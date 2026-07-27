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

/// Spec-shaped `GET /status` body (`HealthStatus`, v0.7.1): a worst-of
/// `status`, the snapshot `timestamp_ms`, and an opaque, evolving `services`
/// object of per-component detail.
fn health_body() -> Value {
    json!({
        "status": "ok",
        "timestamp_ms": 1776033900000i64,
        "services": {"indexer": "ok", "engine": "ok"}
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
    // mock must serve the prefixed path. `markets` (list-all) and the `/status`
    // health snapshot have no `/api/v1` variant and stay on the bare gateway
    // paths below.
    Mock::given(method("GET"))
        .and(path("/api/v1/markets/BTC-USDX-PERP/ticker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ticker_body()))
        .mount(&server)
        .await;
    // v0.7.1 removed the old `/health` probe; `health_check` now reads `/status`.
    Mock::given(method("GET"))
        .and(path("/status"))
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
    assert!(out.contains("status"), "missing status label:\n{out}");
    assert!(out.contains("ok"), "missing status value:\n{out}");
    assert!(
        out.contains("timestamp (ms)"),
        "missing timestamp label:\n{out}"
    );
    assert!(out.contains("1776033900000"), "missing timestamp:\n{out}");
    assert!(out.contains("services"), "missing services label:\n{out}");
    assert!(out.contains("indexer"), "missing services detail:\n{out}");
}

#[tokio::test]
async fn health_json_output() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["--output", "json", "health"])).await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["status"], json!("ok"));
    assert_eq!(v["timestamp_ms"], json!(1776033900000i64));
    assert_eq!(v["services"]["indexer"], json!("ok"));
    assert_eq!(v["services"]["engine"], json!("ok"));
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
    assert_eq!(v["status"], json!("ok"));
}

/// ENG-6039 / ENG-5958: the CLI carries no transport of its own — every request
/// rides the rs SDK's `Client`, which sends `X-Nexus-Api-Version` (the pinned
/// spec tag) on every request plus a normalized `User-Agent`. The CLI overrides
/// the UA to identify itself as `nexus-cli/<version>` (so edge metering can
/// segment CLI traffic) and must NOT strip or clobber the inherited spec-version
/// header. Capture a real request off the wire and assert both headers, rather
/// than trusting inheritance.
#[tokio::test]
async fn emits_api_version_and_user_agent_headers() {
    let server = mock_server().await;
    // Any request exercises the shared transport; `markets` is public and needs
    // no credentials, so this stays offline and deterministic.
    let _ = stdout_of(nexus(&server.uri(), &["markets"])).await;

    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests by default");
    let req = requests.first().expect("the CLI issued a request");

    // Spec tag: inherited from the SDK, which pins the same spec the CLI is
    // compiled against. Assert it equals our own `.api-version` so a future SDK
    // whose pinned tag drifts from the CLI's would trip this test.
    let spec_tag = include_str!("../.api-version").trim();
    let sent_tag = req
        .headers
        .get("x-nexus-api-version")
        .expect("X-Nexus-Api-Version present (inherited from the SDK)")
        .to_str()
        .expect("header value is valid UTF-8");
    assert_eq!(
        sent_tag, spec_tag,
        "emitted spec tag must match the compiled `.api-version` pin"
    );

    // User-Agent: the CLI's own identifier, neither dropped nor left as the SDK
    // default. Mirrors the crate version baked into `nexus --version`.
    let expected_ua = concat!("nexus-cli/", env!("CARGO_PKG_VERSION"));
    let sent_ua = req
        .headers
        .get("user-agent")
        .expect("User-Agent present")
        .to_str()
        .expect("header value is valid UTF-8");
    assert_eq!(
        sent_ua, expected_ua,
        "the CLI must identify itself in the User-Agent"
    );
}

/// ENG-6039 acceptance: `nexus --version` surfaces the compiled-against spec tag
/// (the same tag emitted as `X-Nexus-Api-Version`) alongside the crate and SDK
/// versions. `--version` short-circuits before any request, so no server needed.
#[test]
fn version_reports_spec_tag_and_sdk() {
    let output = Command::cargo_bin("nexus")
        .expect("`nexus` binary builds")
        .arg("--version")
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).expect("stdout is utf-8");
    let spec_tag = include_str!("../.api-version").trim();
    assert!(
        stdout.contains(spec_tag),
        "version output missing spec tag {spec_tag}:\n{stdout}"
    );
    assert!(
        stdout.contains("nexus-exchange"),
        "version output missing SDK version:\n{stdout}"
    );
}
