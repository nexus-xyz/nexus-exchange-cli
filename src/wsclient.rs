//! WebSocket streaming (`GET /ws`), over the SDK's streaming client.
//!
//! Flow: hand the SDK the resolved [`Config`] and let
//! [`Client::connect_ws`](nexus_exchange::Client::connect_ws) (authenticated) or
//! [`Client::connect`](nexus_exchange::Client::connect) (public) own the socket —
//! the token mint, the upgrade, subscription replay, automatic
//! reconnect-with-backoff, ping/pong keep-alive, and bounded buffering. This
//! module only builds the subscription frames, renders the [`Event`]s the SDK
//! yields, and stops on Ctrl-C.
//!
//! The upgrade token is minted over REST (`POST /ws/token`) and rides in the
//! query string, because the `/ws` upgrade accepts it nowhere else. It is
//! short-lived and **single-use**, so it authenticates exactly one socket: a
//! token baked into the connect URL is spent by the first connection and the
//! first automatic reconnect replaying it is rejected (ENG-5291). The CLI
//! therefore never mints, formats or holds one — `connect_ws` mints inside the
//! reconnect loop, so every attempt presents a fresh token, and the SDK also
//! owns redacting the value out of anything it reports.

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
/// `config` is the resolved SDK config (network / base URL / credentials) and
/// `client` is built from it, so the WebSocket origin read here is the one the
/// client streams against. Reading it up front turns "this network has no
/// WebSocket endpoint" into a clean error before anything is sent, rather than a
/// background disconnect event.
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

    // Safe to log as-is: the origin carries no token — the SDK appends one per
    // connection attempt and never hands it back, so there is nothing here to
    // redact. This call site deliberately does NOT run through a `redacted()`
    // helper the way the old token-bearing URL did (review on #76).
    //
    // The one input that could put a secret in this string is a custom network
    // whose configured `ws_url` already contains `?token=`. That is not a leak
    // this can prevent: the value is the user's own, sitting in plaintext in
    // their config file, and the SDK would append a second `token` parameter
    // and produce a broken URL regardless. Re-adding a redactor here would
    // reintroduce the local helper this change removed, to hide a string the
    // user typed themselves.
    eprintln!("connecting to {ws_origin} ...");

    let frames: Vec<Value> = subs.iter().map(Subscription::frame).collect();

    // Account channels need a signed token; public channels stream without one.
    // `connect_ws` mints the first token and re-mints before every reconnect,
    // which is why the CLI hands the socket over instead of baking a token into
    // the URL itself — see the module docs (ENG-5291).
    let mut sub = if authenticated {
        client
            .connect_ws(frames)
            .await
            .context("failed to open an authenticated websocket stream")?
    } else {
        client.connect(frames)
    };
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

#[cfg(test)]
mod tests {
    use super::*;

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
