//! Signed REST client for the endpoints the SDK does not yet expose.
//!
//! Layered over the SDK's [`Config`]/base URL: we reuse the SDK for the public
//! market-data methods it already ships, and this client carries everything
//! else (the rest of market data, the authenticated account, and trading) until
//! those land in the SDK. Requests are HMAC-signed when credentials are
//! configured; the public endpoints work signed or not.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;

use crate::auth::Signer;
use crate::wire::{
    Balance, Fill, NewOrder, Order, OrderBook, OrderResult, Position, Trade, WsToken,
};

/// `{ code, message }` error envelope the API returns on failures.
#[derive(serde::Deserialize)]
struct ApiErrorBody {
    code: String,
    message: Option<String>,
}

/// HTTP client that signs requests when a [`Signer`] is present.
#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base_url: String,
    signer: Option<Signer>,
}

impl ApiClient {
    /// Build a client for `base_url` (the SDK's resolved base, including any
    /// `/api/exchange` prefix). `signer` is `None` for unauthenticated use.
    pub fn new(base_url: impl Into<String>, signer: Option<Signer>) -> Result<Self> {
        let http = reqwest::Client::builder()
            // Bound every request so a hung connection can't wedge the CLI.
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            signer,
        })
    }

    /// Whether requests will be signed.
    pub fn is_authenticated(&self) -> bool {
        self.signer.is_some()
    }

    /// The configured base URL (used to derive the WebSocket URL).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // ───────────────────────── market data ─────────────────────────

    pub async fn fetch_orderbook(&self, market_id: &str) -> Result<OrderBook> {
        let path = format!("/markets/{}/orderbook", path_segment(market_id)?);
        self.send(Method::GET, &path, &[], None).await
    }

    pub async fn fetch_trades(&self, market_id: &str, limit: u32) -> Result<Vec<Trade>> {
        let path = format!("/markets/{}/trades", path_segment(market_id)?);
        self.send(Method::GET, &path, &[("limit", limit.to_string())], None)
            .await
    }

    pub async fn fetch_candles(
        &self,
        market_id: &str,
        timeframe: &str,
        limit: u32,
    ) -> Result<Vec<crate::wire::Candle>> {
        let path = format!("/markets/{}/candles", path_segment(market_id)?);
        let query = [
            ("limit", limit.to_string()),
            ("timeframe", timeframe.to_string()),
        ];
        self.send(Method::GET, &path, &query, None).await
    }

    // ───────────────────────── account ─────────────────────────

    pub async fn fetch_balance(&self) -> Result<Balance> {
        self.send(Method::GET, "/account", &[], None).await
    }

    pub async fn fetch_positions(&self) -> Result<Vec<Position>> {
        self.send(Method::GET, "/positions", &[], None).await
    }

    pub async fn fetch_fills(&self, limit: u32) -> Result<Vec<Fill>> {
        self.send(Method::GET, "/fills", &[("limit", limit.to_string())], None)
            .await
    }

    pub async fn fetch_open_orders(&self) -> Result<Vec<Order>> {
        // Note: the live API exposes no single-order GET (`GET /orders/{id}`
        // returns 405), so there is deliberately no `fetch_order` here.
        self.send(Method::GET, "/orders", &[], None).await
    }

    // ───────────────────────── trading ─────────────────────────

    pub async fn place_order(&self, order: &NewOrder) -> Result<OrderResult> {
        // Serialize once and sign exactly these bytes, so the signature always
        // covers the body that goes on the wire.
        let body = serde_json::to_vec(order).context("failed to encode order")?;
        self.send(Method::POST, "/orders", &[], Some(&body)).await
    }

    /// Cancel one order. The live API requires the order's `market_id` as a
    /// query parameter (a 400 results without it), so it is mandatory here.
    pub async fn cancel_order(&self, order_id: &str, market_id: &str) -> Result<serde_json::Value> {
        let path = format!("/orders/{}", path_segment(order_id)?);
        let query = [("market_id", validated(market_id)?.to_string())];
        self.send(Method::DELETE, &path, &query, None).await
    }

    pub async fn cancel_all(&self, market_id: Option<&str>) -> Result<serde_json::Value> {
        let query: Vec<(&str, String)> = match market_id {
            Some(m) => vec![("market_id", validated(m)?.to_string())],
            None => vec![],
        };
        self.send(Method::DELETE, "/orders", &query, None).await
    }

    // ───────────────────────── websocket token ─────────────────────────

    pub async fn mint_ws_token(&self) -> Result<WsToken> {
        // Body is an empty JSON object; sign and send the same bytes.
        self.send(Method::POST, "/ws/token", &[], Some(b"")).await
    }

    // ───────────────────────── transport ─────────────────────────

    /// Issue one request, signing it if a [`Signer`] is configured, and decode
    /// the JSON response (or the `{ code, message }` error envelope).
    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&[u8]>,
    ) -> Result<T> {
        let canonical_query = canonical_query(query);
        let mut url = format!("{}{}", self.base_url, path);
        if !canonical_query.is_empty() {
            url.push('?');
            url.push_str(&canonical_query);
        }

        let mut req = self.http.request(method.clone(), &url);
        if let Some(bytes) = body {
            req = req
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes.to_vec());
        }

        if let Some(signer) = &self.signer {
            let ts = now_ms()?;
            let sig = signer.sign(
                ts,
                method.as_str(),
                path,
                &canonical_query,
                body.unwrap_or(b""),
            );
            req = req
                .header("X-API-Key", sig.api_key)
                .header("X-Timestamp", sig.timestamp)
                .header("X-Signature", sig.signature);
        }

        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        // Surface rate-limit retry guidance before consuming the body.
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let bytes = resp.bytes().await.context("failed to read response body")?;

        if status.is_success() {
            // A 204 / empty body for a unit-ish response still needs to decode;
            // treat an empty body as JSON null so `Value`/optionals work.
            if bytes.is_empty() {
                return serde_json::from_slice(b"null").context("failed to decode empty response");
            }
            serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "failed to decode response body: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        } else {
            Err(api_error(status, &bytes, retry_after))
        }
    }
}

/// Map a non-2xx response to a descriptive error, decoding the `{ code,
/// message }` envelope when present.
fn api_error(status: StatusCode, bytes: &[u8], retry_after: Option<String>) -> anyhow::Error {
    if status == StatusCode::TOO_MANY_REQUESTS {
        let hint = retry_after
            .map(|s| format!(" (retry after {s}s)"))
            .unwrap_or_default();
        return anyhow!("rate limited{hint}: slow down and try again");
    }
    if status == StatusCode::UNAUTHORIZED {
        return anyhow!(
            "authentication failed (401) — check your API key/secret (run `nexus setup`)"
        );
    }
    if let Ok(env) = serde_json::from_slice::<ApiErrorBody>(bytes) {
        anyhow!(
            "api error [{}]: {}",
            env.code,
            env.message.unwrap_or_default()
        )
    } else {
        anyhow!(
            "api error [{}]: {}",
            status.as_str(),
            String::from_utf8_lossy(bytes)
        )
    }
}

/// Current unix time in milliseconds, for the signature timestamp.
fn now_ms() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis())
}

/// Build the canonical query string: pairs sorted by key (then value),
/// percent-encoded, joined with `&`, with no leading `?`. Empty when no pairs.
/// The exact string returned here is both signed and sent, so they can never
/// disagree.
fn canonical_query(pairs: &[(&str, String)]) -> String {
    let mut pairs: Vec<&(&str, String)> = pairs.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode per RFC 3986, leaving unreserved characters untouched. The
/// identifiers and values this CLI sends are already unreserved, so this is a
/// defensive no-op for normal input rather than a behavior change.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Validate a value destined for a URL path segment or query value. Rejects
/// anything that could break out of the segment (slashes, query/fragment
/// delimiters, whitespace, control characters) so a crafted `market_id` cannot
/// alter the request target or desynchronize the signed path from the wire.
fn validated(id: &str) -> Result<&str> {
    if id.is_empty() {
        return Err(anyhow!("identifier must not be empty"));
    }
    let ok = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if ok {
        Ok(id)
    } else {
        Err(anyhow!(
            "invalid identifier {id:?}: only letters, digits, '-', '.', '_' are allowed"
        ))
    }
}

/// Validate and return an identifier for use as a path segment.
fn path_segment(id: &str) -> Result<&str> {
    validated(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_query_is_sorted_and_stable() {
        let q = canonical_query(&[
            ("timeframe", "1m".to_string()),
            ("limit", "100".to_string()),
        ]);
        assert_eq!(q, "limit=100&timeframe=1m");
        assert_eq!(canonical_query(&[]), "");
    }

    #[test]
    fn encode_leaves_market_ids_untouched_but_escapes_specials() {
        assert_eq!(encode("BTC-USDX-PERP"), "BTC-USDX-PERP");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("a/b?c"), "a%2Fb%3Fc");
    }

    #[test]
    fn validated_rejects_injection_attempts() {
        assert!(validated("BTC-USDX-PERP").is_ok());
        assert!(validated("3fa85f64-5717-4562-b3fc-2c963f66afa6").is_ok());
        assert!(validated("").is_err());
        assert!(validated("../admin").is_err());
        assert!(validated("BTC/orderbook?x=1").is_err());
        assert!(validated("a b").is_err());
    }
}
