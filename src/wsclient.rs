//! WebSocket streaming (`GET /ws`), over the SDK's streaming client.
//!
//! Flow: mint a short-lived single-use token over REST (when authenticated),
//! hand the SDK a [`Config`] whose `ws_url` carries that token, and let
//! [`Client::connect`](nexus_exchange::Client::connect) own the socket — the
//! upgrade, subscription replay, automatic reconnect-with-backoff, ping/pong
//! keep-alive, and bounded buffering. This module only builds the subscription
//! frames, renders the [`Event`]s the SDK yields, and stops on Ctrl-C.

use anyhow::{Context, Result};
use nexus_exchange::ws::Event;
use nexus_exchange::{Client, Config};
use serde_json::{json, Value};

use crate::cli::OutputFormat;

/// Channels that carry public per-market data and therefore require a `market`.
pub const PUBLIC_CHANNELS: &[&str] = &["trades", "book", "candles"];
/// Channels scoped to the account that minted the token; `market` is ignored.
pub const ACCOUNT_CHANNELS: &[&str] = &["orders", "fills", "positions", "balances"];

/// One channel subscription requested on the command line.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub channel: String,
    pub market: Option<String>,
    pub since: Option<i64>,
}

impl Subscription {
    /// The `subscribe` envelope sent for this channel.
    fn frame(&self) -> Value {
        let mut msg = json!({ "op": "subscribe", "channel": self.channel });
        if let Some(market) = &self.market {
            msg["market"] = json!(market);
        }
        if let Some(since) = self.since {
            msg["since"] = json!(since);
        }
        msg
    }
}

/// Connect, subscribe, and stream until Ctrl-C is pressed.
///
/// `config` is the resolved SDK config (network / base URL / credentials); a
/// clone with the token-bearing `ws_url` drives the streaming client. The
/// `client` (which holds the same credentials) mints the token.
pub async fn stream(
    client: &Client,
    config: &Config,
    authenticated: bool,
    subs: &[Subscription],
    format: OutputFormat,
) -> Result<()> {
    // The SDK only knows a WebSocket origin for networks that have one.
    let ws_origin = config
        .ws_url()
        .context("the selected network has no WebSocket endpoint")?;

    // Account channels need a signed token; public channels can stream without
    // one. Only mint when we actually have credentials.
    let ws_url = if authenticated {
        let token = client
            .mint_web_socket_token()
            .await
            .context("failed to mint a websocket token")?;
        format!("{}?token={}", ws_origin, encode_token(&token.token))
    } else {
        ws_origin.to_string()
    };

    // Never log the token (it is a bearer credential); show only the host path.
    eprintln!("connecting to {} ...", redacted(&ws_url));

    let frames: Vec<Value> = subs.iter().map(Subscription::frame).collect();
    let ws_client = Client::new(config.clone().with_ws_url(ws_url));
    let mut sub = ws_client.connect(frames);
    eprintln!("streaming events (Ctrl-C to stop)");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nclosing.");
                sub.close().await;
                break;
            }
            event = sub.next() => match event {
                None => {
                    eprintln!("stream ended.");
                    break;
                }
                Some(Event::Message(value)) => render(&value, format),
                Some(Event::Connected) => eprintln!("connected; subscriptions sent."),
                Some(Event::Disconnected(reason)) => {
                    eprintln!("disconnected: {reason} (reconnecting…)");
                }
                Some(Event::Lagged { dropped }) => {
                    eprintln!("warning: fell behind, dropped {dropped} message(s)");
                }
                // `Event` is #[non_exhaustive]; ignore variants added upstream.
                Some(_) => {}
            },
        }
    }

    Ok(())
}

/// Render one server message. In JSON mode the event is emitted as a single
/// compact JSON line (friendly to `jq`/streaming consumers); in human mode the
/// envelope is summarized to a single tidy line.
fn render(value: &Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!("{value}"),
        OutputFormat::Human => println!("{}", humanize(value)),
    }
}

/// Summarize a server envelope for human output, falling back to the compact
/// JSON if it isn't the shape we expect.
fn humanize(v: &Value) -> String {
    let op = v.get("op").and_then(|o| o.as_str()).unwrap_or("?");
    let channel = v.get("channel").and_then(|c| c.as_str()).unwrap_or("");
    let market = v
        .get("market")
        .and_then(|m| m.as_str())
        .map(|m| format!(" {m}"))
        .unwrap_or_default();
    match op {
        "event" => {
            let seq = v.get("seq").and_then(|s| s.as_i64()).unwrap_or(-1);
            let payload = v.get("payload").map(|p| p.to_string()).unwrap_or_default();
            format!("[{channel}{market} #{seq}] {payload}")
        }
        "subscribed" => format!("subscribed: {channel}{market}"),
        "unsubscribed" => format!("unsubscribed: {channel}{market}"),
        "out_of_sync" => {
            let oldest = v.get("oldest_seq").and_then(|s| s.as_i64()).unwrap_or(-1);
            format!("out_of_sync: {channel}{market} — refetch state and resubscribe (oldest_seq={oldest})")
        }
        "error" => {
            let m = v.get("message").and_then(|m| m.as_str()).unwrap_or("");
            format!("error: {m}")
        }
        _ => v.to_string(),
    }
}

/// Percent-encode a token for use in a query value (defensive — tokens are
/// already URL-safe hex).
fn encode_token(token: &str) -> String {
    token
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Strip the token from a URL so it can be logged safely.
///
/// The token rides in the query string because the server's `/ws` upgrade only
/// accepts it there — there is no header-based alternative on this endpoint, and
/// browsers can't set headers on a `WebSocket` handshake anyway. Residual
/// exposure: query strings can surface in proxy/access logs, shell history, and
/// crash dumps more readily than headers. We mitigate by minting a short-lived,
/// single-use token per connection, redacting it here from anything we log, and
/// never persisting it.
fn redacted(url: &str) -> String {
    match url.split_once("?token=") {
        Some((head, _)) => format!("{head}?token=<redacted>"),
        None => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_never_logged() {
        let r = redacted("wss://h/api/exchange/ws?token=supersecret");
        assert!(!r.contains("supersecret"));
        assert!(r.contains("<redacted>"));
    }

    #[test]
    fn encode_token_escapes_non_unreserved() {
        assert_eq!(encode_token("abc123"), "abc123");
        assert_eq!(encode_token("a/b c"), "a%2Fb%20c");
    }

    #[test]
    fn frame_includes_market_and_since_when_set() {
        let sub = Subscription {
            channel: "trades".into(),
            market: Some("BTC-USDX-PERP".into()),
            since: Some(42),
        };
        let f = sub.frame();
        assert_eq!(f["op"], json!("subscribe"));
        assert_eq!(f["channel"], json!("trades"));
        assert_eq!(f["market"], json!("BTC-USDX-PERP"));
        assert_eq!(f["since"], json!(42));

        // Account channel: no market, no since.
        let acct = Subscription {
            channel: "orders".into(),
            market: None,
            since: None,
        };
        let f = acct.frame();
        assert!(f.get("market").is_none());
        assert!(f.get("since").is_none());
    }

    #[test]
    fn humanizes_event_and_passes_through_unknown() {
        let line = humanize(&json!({
            "op": "event", "channel": "trades", "market": "BTC-USDX-PERP",
            "seq": 7, "payload": { "price": 100 }
        }));
        assert!(line.contains("trades"));
        assert!(line.contains("#7"));
        // An unrecognized op falls back to the compact JSON.
        let other = humanize(&json!({ "op": "weird" }));
        assert!(other.contains("weird"));
    }
}
