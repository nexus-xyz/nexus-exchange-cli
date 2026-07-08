//! End-to-end smoke test: run the BUILT `nexus` binary against a local mock
//! HTTP server and assert its rendered output for the read-only public
//! commands in both `--output human` and `--output json` modes.
//!
//! This complements the in-crate unit tests (which cover arg parsing and the
//! `output::*` renderers against hand-built fixtures): it exercises the whole
//! path the unit tests can't — process spawn, arg/env resolution, the SDK's
//! HTTP transport, JSON decode, and stdout rendering — wired together exactly
//! as a user invokes them.
//!
//! It mirrors, in spirit, the Rust SDK's wiremock integration tests
//! (`tests/public.rs` over there) and the Python SDK's loopback smoke: a real
//! loopback socket serves canned, spec-shaped responses, so the test is fully
//! offline and deterministic — no live network, no credentials, no clock or
//! port assumptions (wiremock binds an ephemeral port and we pass it via
//! `--base-url`).
//!
//! Scope is deliberately narrow: the three read-only, unauthenticated commands
//! the ticket calls out (`markets`, `ticker`, `health`), each in both output
//! modes. Authenticated commands (which would need request signing) and the
//! `ws` streaming path are out of scope for a smoke test.

use assert_cmd::Command;
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Spec-shaped `GET /markets` body. Mirrors the fixture the SDK's own wiremock
/// test (`tests/public.rs::fetch_markets_parses_string_decimals`) uses: money
/// fields are decimal *strings*, `max_leverage` is a JSON number.
fn markets_body() -> Value {
    json!([{
        "market_id": "BTC-USDX-PERP", "base_asset": "BTC", "quote_asset": "USDX",
        "tick_size": "0.5", "lot_size": "0.001", "min_order_size": "0.001",
        "max_order_size": "100", "initial_margin_rate": "0.05",
        "maintenance_margin_rate": "0.03", "max_leverage": 20
    }])
}

/// Spec-shaped `GET /markets/{id}/ticker` body. Market-data money rides the
/// server's JSON-number `float` adapter; `bid` is null to exercise the
/// present-vs-absent contract end to end.
fn ticker_body() -> Value {
    json!({
        "symbol": "BTC-USDX-PERP", "timestamp": 1776033900000i64,
        "datetime": "2026-04-13T00:00:00Z", "high": 51903.0, "low": 44992.0,
        "bid": null, "bidVolume": null, "ask": 50012.5, "askVolume": 1.2,
        "open": 48062.0, "close": 51903.0, "last": 51903.0, "change": 3841.0,
        "percentage": 7.99, "baseVolume": 27.1, "quoteVolume": 1350000.0,
        "markPrice": 50011.6, "indexPrice": 50010.0, "info": {}
    })
}

/// Spec-shaped `GET /health` body (`HealthStatus`).
fn health_body() -> Value {
    json!({
        "health": "ok", "connected": true,
        "events_received": 7, "fills_total": 3, "uptime_seconds": 42
    })
}

/// Start a mock server serving canned responses for the three public endpoints.
async fn mock_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/markets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&server)
        .await;
    // The per-market ticker migrated to the direct-indexer `/api/v1` surface
    // (ENG-5190): the SDK now routes it to the host root under `/api/v1`, so the
    // mock must serve the prefixed path. `markets` (list-all) and `health` have
    // no `/api/v1` variant yet and stay on the bare gateway paths below.
    Mock::given(method("GET"))
        .and(path("/api/v1/markets/BTC-USDX-PERP/ticker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ticker_body()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(health_body()))
        .mount(&server)
        .await;
    server
}

/// Build a `nexus` command pointed at the mock server. `--base-url` sets the
/// SDK's base host; the SDK then routes each request per its path — migrated
/// endpoints under `/api/v1/...` and the rest on the bare gateway path — all
/// against this same host, so the mock above serves both shapes. `NEXUS_OUTPUT`
/// is cleared so a value in the test runner's environment can't change what we
/// assert.
fn nexus(base_url: &str, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("nexus").expect("`nexus` binary builds");
    cmd.env_remove("NEXUS_OUTPUT")
        .env_remove("NEXUS_API_KEY")
        .env_remove("NEXUS_API_SECRET")
        .env_remove("NEXUS_NETWORK")
        .env_remove("NEXUS_BASE_URL")
        .arg("--base-url")
        .arg(base_url)
        .args(args);
    cmd
}

/// Run `cmd` to completion, assert success, and return stdout as a `String`.
/// Runs on a blocking thread so it doesn't stall the tokio runtime the mock
/// server lives on.
async fn stdout_of(mut cmd: Command) -> String {
    tokio::task::spawn_blocking(move || {
        let out = cmd.assert().success();
        String::from_utf8(out.get_output().stdout.clone()).expect("stdout is utf-8")
    })
    .await
    .expect("command thread joins")
}

#[tokio::test]
async fn markets_human_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["markets"])).await;
    // Header + the one market's row + the count footer.
    assert!(out.contains("MARKET"), "missing table header:\n{out}");
    assert!(out.contains("MAX LEV"), "missing table header:\n{out}");
    assert!(out.contains("BTC-USDX-PERP"), "missing market row:\n{out}");
    assert!(out.contains("0.5"), "missing tick size:\n{out}");
    assert!(out.contains("20x"), "missing leverage:\n{out}");
    assert!(out.contains("1 market(s)."), "missing footer:\n{out}");
}

#[tokio::test]
async fn markets_json_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["--output", "json", "markets"])).await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    let row = &v.as_array().expect("top-level array")[0];
    assert_eq!(row["market_id"], json!("BTC-USDX-PERP"));
    // Money serializes as a decimal string; leverage stays a JSON number.
    assert_eq!(row["tick_size"], json!("0.5"));
    assert_eq!(row["max_leverage"], json!(20));
}

#[tokio::test]
async fn ticker_human_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["ticker", "BTC-USDX-PERP"])).await;
    assert!(out.contains("symbol"), "missing symbol label:\n{out}");
    assert!(
        out.contains("BTC-USDX-PERP"),
        "missing symbol value:\n{out}"
    );
    assert!(out.contains("51903"), "missing last price:\n{out}");
    assert!(out.contains("50012.5"), "missing ask:\n{out}");
    // Absent bid renders as `-` in human mode.
    assert!(out.contains("bid"), "missing bid label:\n{out}");
}

#[tokio::test]
async fn ticker_json_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(
        &server.uri(),
        &["--output", "json", "ticker", "BTC-USDX-PERP"],
    ))
    .await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["symbol"], json!("BTC-USDX-PERP"));
    // Present money -> decimal string; absent -> JSON null.
    assert_eq!(v["last"], json!("51903"));
    assert_eq!(v["ask"], json!("50012.5"));
    assert_eq!(v["bid"], Value::Null);
}

#[tokio::test]
async fn health_human_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["health"])).await;
    assert!(out.contains("health"), "missing health label:\n{out}");
    assert!(out.contains("ok"), "missing health value:\n{out}");
    assert!(out.contains("connected"), "missing connected label:\n{out}");
    assert!(out.contains("true"), "missing connected value:\n{out}");
    assert!(
        out.contains("events received"),
        "missing events label:\n{out}"
    );
    assert!(out.contains('7'), "missing events value:\n{out}");
}

#[tokio::test]
async fn health_json_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["--output", "json", "health"])).await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["health"], json!("ok"));
    assert_eq!(v["connected"], json!(true));
    assert_eq!(v["events_received"], json!(7));
    assert_eq!(v["fills_total"], json!(3));
    assert_eq!(v["uptime_seconds"], json!(42));
}

/// The output mode also resolves from `NEXUS_OUTPUT`, not just the flag — the
/// same env path the README documents. Assert JSON comes back when it is set.
#[tokio::test]
async fn output_mode_resolves_from_env() {
    let server = mock_server().await;
    let mut cmd = Command::cargo_bin("nexus").expect("`nexus` binary builds");
    cmd.env_remove("NEXUS_API_KEY")
        .env_remove("NEXUS_API_SECRET")
        .env_remove("NEXUS_NETWORK")
        .env_remove("NEXUS_BASE_URL")
        .env("NEXUS_OUTPUT", "json")
        .arg("--base-url")
        .arg(server.uri())
        .arg("health");
    let out = stdout_of(cmd).await;
    let v: Value = serde_json::from_str(&out).expect("NEXUS_OUTPUT=json yields JSON");
    assert_eq!(v["health"], json!("ok"));
}
