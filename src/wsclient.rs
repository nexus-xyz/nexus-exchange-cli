//! WebSocket streaming (`GET /ws`), over the SDK's streaming client.
//!
//! Flow: hand the SDK the resolved [`Config`] and let
//! [`Client::connect_ws`](nexus_exchange::Client::connect_ws) (authenticated) or
//! [`Client::connect`](nexus_exchange::Client::connect) (public) own each socket:
//! the token mint, the upgrade, ping/pong keep-alive, and bounded buffering.
//!
//! Resume is the CLI's job, because the SDK's raw client replays the frames it
//! was opened with verbatim. So this module keeps a cursor per channel (the
//! highest `seq` seen, seeded from each `subscribed` frame's `seq_at_join`) and,
//! when a socket drops, closes that SDK stream and opens a new one whose
//! `subscribe` frames carry `since` = the cursor. The user's `--since` is used
//! only for the very first subscribe. On `out_of_sync` the server has ended the
//! subscription, so the cursor is dropped and the channel is re-subscribed from
//! the live edge on the same socket. This mirrors the SDK's typed client
//! (`nexus-exchange-rs` ENG-18685).
//!
//! The upgrade token is minted over REST (`POST /ws/token`) and rides in the
//! query string, because the `/ws` upgrade accepts it nowhere else. It is
//! short-lived and **single-use**, so it authenticates exactly one socket: a
//! token baked into the connect URL is spent by the first connection and the
//! first automatic reconnect replaying it is rejected (ENG-5291). The CLI
//! therefore never mints, formats or holds one — `connect_ws` mints for every
//! connection this module opens, so every attempt presents a fresh token, and
//! the SDK also owns redacting the value out of anything it reports.

use std::collections::HashMap;
use std::future::Future;

use anyhow::{Context, Result};
use nexus_exchange::ws::{self, Backoff, Event};
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
    /// The `subscribe` envelope sent for this channel, resuming after `since`.
    fn frame(&self, since: Option<i64>) -> Value {
        let mut msg = json!({ "op": "subscribe", "channel": self.channel });
        if let Some(market) = &self.market {
            msg["market"] = json!(market);
        }
        if let Some(since) = since {
            msg["since"] = json!(since);
        }
        msg
    }
}

/// Resume cursors, one per channel: the highest `seq` seen.
///
/// ponytail: keyed by channel name alone, because one `nexus ws` invocation has a
/// single `--market`; key by (channel, market) if it ever takes several.
#[derive(Debug, Default)]
struct Cursors(HashMap<String, i64>);

impl Cursors {
    /// Fold one server frame in. Returns the channels to re-subscribe from the
    /// live edge, which is non-empty only for an `out_of_sync`.
    fn observe(&mut self, v: &Value) -> Vec<String> {
        let op = v.get("op").and_then(Value::as_str).unwrap_or("");
        let Some(channel) = v.get("channel").and_then(Value::as_str) else {
            return Vec::new();
        };
        let seq = match op {
            "event" => v.get("seq"),
            "subscribed" => v.get("seq_at_join"),
            "out_of_sync" => {
                // The server has ended this subscription. Its cursor can no longer
                // be satisfied, so drop it; the frame carries no market we need,
                // since every market of the channel is covered (one per run).
                self.0.remove(channel);
                return vec![channel.to_string()];
            }
            _ => None,
        };
        if let Some(seq) = seq.and_then(Value::as_i64) {
            let cursor = self.0.entry(channel.to_string()).or_insert(seq);
            // A duplicate or reordered frame never moves the cursor back.
            *cursor = (*cursor).max(seq);
        }
        Vec::new()
    }

    fn get(&self, channel: &str) -> Option<i64> {
        self.0.get(channel).copied()
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

    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\nclosing.");
    };
    pump(
        client,
        authenticated,
        subs,
        &Backoff::new(),
        |v| render(v, format),
        ctrl_c,
    )
    .await
}

/// Stream `subs`, resuming each channel from its cursor across reconnects, and
/// hand every server frame to `emit` until `shutdown` resolves or the stream
/// ends. Split from [`stream`] so tests can drive it against a local server.
async fn pump(
    client: &Client,
    authenticated: bool,
    subs: &[Subscription],
    backoff: &Backoff,
    mut emit: impl FnMut(&Value),
    shutdown: impl Future<Output = ()>,
) -> Result<()> {
    tokio::pin!(shutdown);
    let mut cursors = Cursors::default();
    let mut delays = backoff.iter();
    let mut first = true;

    loop {
        // The user's `--since` applies to the first subscribe only; after that a
        // channel resumes from its cursor, or from the live edge if it has none.
        let frames: Vec<Value> = subs
            .iter()
            .map(|s| {
                s.frame(
                    cursors
                        .get(&s.channel)
                        .or(if first { s.since } else { None }),
                )
            })
            .collect();

        // Account channels need a signed token; public channels stream without
        // one. `connect_ws` mints a fresh token for each connection (ENG-5291).
        let opened = if authenticated {
            client.connect_ws(frames).await
        } else {
            Ok(client.connect(frames))
        };
        let mut sub = match opened {
            Ok(sub) => sub,
            Err(err) if first => {
                return Err(err).context("failed to open an authenticated websocket stream")
            }
            Err(err) => {
                eprintln!("reconnect failed: {err}");
                if wait(&mut delays, &mut shutdown).await {
                    return Ok(());
                }
                continue;
            }
        };
        if first {
            eprintln!("streaming events (Ctrl-C to stop)");
        }
        first = false;

        let mut delivered = false;
        let stopped = loop {
            tokio::select! {
                _ = &mut shutdown => break true,
                event = sub.next() => match event {
                    None => {
                        eprintln!("stream ended.");
                        sub.close().await;
                        return Ok(());
                    }
                    Some(Event::Message(value)) => {
                        delivered = true;
                        let before = value
                            .get("channel")
                            .and_then(Value::as_str)
                            .and_then(|c| cursors.get(c));
                        let resync = cursors.observe(&value);
                        emit(&value);
                        for channel in resync {
                            report_gap(&value, before);
                            for s in subs.iter().filter(|s| s.channel == channel) {
                                // Same socket; a failed send surfaces as a
                                // disconnect and the reconnect resubscribes.
                                let _ = sub.subscribe(s.frame(None)).await;
                            }
                        }
                    }
                    Some(Event::Connected) => eprintln!("connected; subscriptions sent."),
                    Some(Event::Disconnected(reason)) => {
                        eprintln!("disconnected: {reason} (reconnecting…)");
                        break false;
                    }
                    Some(Event::Lagged { dropped }) => {
                        eprintln!("warning: fell behind, dropped {dropped} message(s)");
                    }
                    // `Event` is #[non_exhaustive]; ignore variants added upstream.
                    Some(_) => {}
                },
            }
        };

        // Close this SDK stream before it replays its original frames, and open a
        // new one carrying the cursors.
        sub.close().await;
        if stopped {
            return Ok(());
        }
        // Reset the backoff only once a connection has actually delivered.
        if delivered {
            delays = backoff.iter();
        }
        if wait(&mut delays, &mut shutdown).await {
            return Ok(());
        }
    }
}

/// Sleep one backoff delay. Returns `true` if `shutdown` fired meanwhile.
async fn wait(
    delays: &mut ws::BackoffIter,
    shutdown: &mut std::pin::Pin<&mut impl Future<Output = ()>>,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delays.next_delay()) => false,
        _ = shutdown => true,
    }
}

/// Tell the user which events an `out_of_sync` lost. The frame itself is already
/// on stdout; this goes to stderr with the other connection notices.
fn report_gap(v: &Value, before: Option<i64>) {
    let channel = v.get("channel").and_then(Value::as_str).unwrap_or("");
    let oldest = v.get("oldest_seq").and_then(Value::as_i64);
    let gap = match (before, oldest) {
        (Some(from), Some(to)) => format!("events after #{from} up to #{to} were missed"),
        (Some(from), None) => format!("events after #{from} were missed"),
        (None, Some(to)) => format!("events before #{to} were missed"),
        (None, None) => "some events were missed".to_string(),
    };
    eprintln!("{channel}: {gap}; resubscribed from the live edge.");
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
            format!("out_of_sync: {channel}{market}, resubscribing from the live edge (oldest_seq={oldest})")
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
        let f = sub.frame(sub.since);
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
        let f = acct.frame(None);
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

    #[test]
    fn cursor_takes_the_max_seq_and_out_of_sync_drops_it() {
        let mut c = Cursors::default();
        c.observe(&json!({ "op": "subscribed", "channel": "trades", "seq_at_join": 7 }));
        assert_eq!(c.get("trades"), Some(7));
        c.observe(&json!({ "op": "event", "channel": "trades", "seq": 9 }));
        // A reordered frame never rewinds the cursor.
        c.observe(&json!({ "op": "event", "channel": "trades", "seq": 8 }));
        assert_eq!(c.get("trades"), Some(9));
        // seq 0 is a real cursor.
        c.observe(&json!({ "op": "subscribed", "channel": "fills", "seq_at_join": 0 }));
        assert_eq!(c.get("fills"), Some(0));

        let resync =
            c.observe(&json!({ "op": "out_of_sync", "channel": "trades", "market": null }));
        assert_eq!(resync, vec!["trades".to_string()]);
        assert_eq!(c.get("trades"), None);
    }

    mod fake_server {
        use super::super::*;
        use futures_util::{SinkExt, StreamExt};
        use nexus_exchange::Network;
        use std::time::Duration;
        use tokio::net::TcpListener;
        use tokio::sync::{mpsc, oneshot};
        use tokio_tungstenite::tungstenite::Message;
        use tokio_tungstenite::WebSocketStream;

        type Ws = WebSocketStream<tokio::net::TcpStream>;

        fn fast() -> Backoff {
            Backoff::new()
                .with_initial(Duration::from_millis(10))
                .with_max(Duration::from_millis(20))
        }

        async fn accept(listener: &TcpListener) -> Ws {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio_tungstenite::accept_async(tcp).await.unwrap()
        }

        async fn recv(ws: &mut Ws) -> Value {
            loop {
                if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
                    return serde_json::from_str(&t).unwrap();
                }
            }
        }

        async fn send(ws: &mut Ws, v: Value) {
            ws.send(Message::Text(v.to_string().into())).await.unwrap();
        }

        fn trades(since: Option<i64>) -> Vec<Subscription> {
            vec![Subscription {
                channel: "trades".into(),
                market: Some("BTC-USDX-PERP".into()),
                since,
            }]
        }

        /// Run `pump` until an event with `until_seq` is emitted.
        async fn run_until(client: &Client, subs: &[Subscription], until_seq: i64) {
            let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let watcher = async move {
                while let Some(v) = rx.recv().await {
                    if v["op"] == "event" && v["seq"] == json!(until_seq) {
                        let _ = stop_tx.send(());
                        return;
                    }
                }
            };
            let backoff = fast();
            let run = pump(
                client,
                false,
                subs,
                &backoff,
                move |v| {
                    let _ = tx.send(v.clone());
                },
                async {
                    let _ = stop_rx.await;
                },
            );
            let (res, ()) = tokio::time::timeout(Duration::from_secs(10), async {
                tokio::join!(run, watcher)
            })
            .await
            .expect("stream did not reach the expected event");
            res.unwrap();
        }

        async fn setup() -> (TcpListener, Client) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let client = Client::new(
                Config::new(Network::Local).with_ws_url(format!("ws://127.0.0.1:{port}")),
            );
            (listener, client)
        }

        #[tokio::test]
        async fn reconnect_resumes_from_the_last_seen_seq_not_the_original_since() {
            let (listener, client) = setup().await;
            let server = tokio::spawn(async move {
                let mut ws = accept(&listener).await;
                let first = recv(&mut ws).await;
                send(&mut ws, json!({ "op": "subscribed", "channel": "trades", "market": "BTC-USDX-PERP", "seq_at_join": 7 })).await;
                send(&mut ws, json!({ "op": "event", "channel": "trades", "market": "BTC-USDX-PERP", "seq": 8, "payload": {} })).await;
                ws.close(None).await.unwrap();

                let mut ws = accept(&listener).await;
                let second = recv(&mut ws).await;
                send(&mut ws, json!({ "op": "event", "channel": "trades", "market": "BTC-USDX-PERP", "seq": 9, "payload": {} })).await;
                (first, second, ws)
            });

            run_until(&client, &trades(Some(5)), 9).await;
            let (first, second, _ws) = server.await.unwrap();
            assert_eq!(
                first["since"],
                json!(5),
                "the first subscribe honours --since"
            );
            assert_eq!(
                second["since"],
                json!(8),
                "the reconnect resumes after the last seq seen"
            );
        }

        #[tokio::test]
        async fn out_of_sync_resubscribes_on_the_same_socket() {
            let (listener, client) = setup().await;
            let server = tokio::spawn(async move {
                let mut ws = accept(&listener).await;
                let first = recv(&mut ws).await;
                send(&mut ws, json!({ "op": "subscribed", "channel": "trades", "market": "BTC-USDX-PERP", "seq_at_join": 3 })).await;
                send(&mut ws, json!({ "op": "out_of_sync", "channel": "trades", "market": null, "oldest_seq": 10 })).await;
                let again = recv(&mut ws).await;
                send(&mut ws, json!({ "op": "subscribed", "channel": "trades", "market": "BTC-USDX-PERP", "seq_at_join": 12 })).await;
                send(&mut ws, json!({ "op": "event", "channel": "trades", "market": "BTC-USDX-PERP", "seq": 13, "payload": {} })).await;
                (first, again, listener, ws)
            });

            run_until(&client, &trades(None), 13).await;
            let (first, again, listener, _ws) = server.await.unwrap();
            assert!(first.get("since").is_none());
            assert_eq!(again["op"], json!("subscribe"));
            assert!(
                again.get("since").is_none(),
                "resubscribes from the live edge"
            );
            // Recovered without a second connection.
            let second = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
            assert!(second.is_err(), "no reconnect was needed");
        }
    }
}
