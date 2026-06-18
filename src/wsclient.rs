//! WebSocket streaming (`GET /ws`).
//!
//! Flow: mint a short-lived single-use token over REST, upgrade to `wss://`,
//! send one `subscribe` envelope per channel, then stream `event` messages to
//! stdout until the server closes or the user hits Ctrl-C.
//!
//! Concurrency: the socket is `split()` into a read half and a write half so the
//! select loop can race "next inbound message" against "Ctrl-C" while still
//! writing pongs/close frames — all from a single task, so there are no shared
//! locks and therefore no possibility of deadlock. Inbound pings are answered
//! with pongs to keep the connection alive.

use anyhow::{anyhow, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::api::ApiClient;
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

/// Connect, subscribe, and stream until the server closes or Ctrl-C is pressed.
pub async fn stream(api: &ApiClient, subs: &[Subscription], format: OutputFormat) -> Result<()> {
    let token = api
        .mint_ws_token()
        .await
        .context("failed to mint a websocket token")?;
    let url = ws_url(api.base_url(), &token.token);

    // Never log the token (it is a bearer credential); show only the host path.
    eprintln!("connecting to {} ...", redacted(&url));
    let (socket, _resp) = connect_async(url)
        .await
        .context("websocket connection failed")?;
    let (mut write, mut read) = socket.split();

    for sub in subs {
        let mut msg = json!({ "op": "subscribe", "channel": sub.channel });
        if let Some(market) = &sub.market {
            msg["market"] = json!(market);
        }
        if let Some(since) = sub.since {
            msg["since"] = json!(since);
        }
        write
            .send(Message::Text(msg.to_string().into()))
            .await
            .with_context(|| format!("failed to subscribe to '{}'", sub.channel))?;
    }
    eprintln!("subscribed; streaming events (Ctrl-C to stop)");

    loop {
        tokio::select! {
            // Graceful shutdown: tell the server we're going away, then stop.
            _ = tokio::signal::ctrl_c() => {
                let _ = write.send(Message::Close(None)).await;
                eprintln!("\nclosing.");
                break;
            }
            item = read.next() => {
                match item {
                    None => {
                        eprintln!("connection closed by server.");
                        break;
                    }
                    Some(Err(e)) => return Err(anyhow!("websocket error: {e}")),
                    Some(Ok(msg)) => match msg {
                        Message::Text(text) => render(text.as_str(), format),
                        // Keep-alive: answer pings so the server doesn't drop us.
                        Message::Ping(payload) => {
                            let _ = write.send(Message::Pong(payload)).await;
                        }
                        Message::Close(frame) => {
                            match frame {
                                Some(f) => eprintln!("server closed connection: {} {}", f.code, f.reason),
                                None => eprintln!("server closed connection."),
                            }
                            break;
                        }
                        Message::Binary(_) | Message::Pong(_) | Message::Frame(_) => {}
                    },
                }
            }
        }
    }

    Ok(())
}

/// Render one server message. In JSON mode the line is already JSON, so it is
/// emitted verbatim (one event per line — friendly to `jq`/streaming consumers).
/// In human mode the envelope is summarized to a single tidy line.
fn render(text: &str, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!("{text}"),
        OutputFormat::Human => println!("{}", humanize(text)),
    }
}

/// Summarize a server envelope for human output, falling back to the raw text
/// if it isn't the shape we expect.
fn humanize(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
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
        _ => text.to_string(),
    }
}

/// Build the `wss://…/ws?token=…` URL from the REST base URL.
fn ws_url(base_url: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/ws?token={}", encode_token(token))
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
    fn derives_wss_url_and_keeps_base_path() {
        assert_eq!(
            ws_url("https://exchange.nexus.xyz/api/exchange", "abc123"),
            "wss://exchange.nexus.xyz/api/exchange/ws?token=abc123"
        );
        assert_eq!(
            ws_url("http://localhost:9090/", "t"),
            "ws://localhost:9090/ws?token=t"
        );
    }

    #[test]
    fn token_is_never_logged() {
        let r = redacted("wss://h/api/exchange/ws?token=supersecret");
        assert!(!r.contains("supersecret"));
        assert!(r.contains("<redacted>"));
    }

    #[test]
    fn humanizes_event_and_passes_through_unknown() {
        let line = humanize(
            r#"{"op":"event","channel":"trades","market":"BTC-USDX-PERP","seq":7,"payload":{"price":100}}"#,
        );
        assert!(line.contains("trades"));
        assert!(line.contains("#7"));
        // Non-JSON falls back to the raw text.
        assert_eq!(humanize("not json"), "not json");
    }
}
