//! End-to-end smoke test for the portfolio-parity commands (ENG-6460): run the
//! BUILT `nexus` binary, with credentials, against a local mock HTTP server and
//! assert what it renders for `account summary`, `account state`, `account
//! fees`, `account portfolio-history`, and the enriched `positions`.
//!
//! Same shape as `tests/smoke.rs` — a real loopback socket serving canned,
//! spec-shaped responses, so the test is offline and deterministic — but pointed
//! at the *authenticated* surface. The mock does not verify the HMAC signature
//! (the SDK owns signing and has its own tests for it); what these cover is the
//! part the CLI owns: dispatch, query forwarding, and rendering.
//!
//! Two behaviours here are safety properties, not cosmetics, and are asserted
//! deliberately:
//!
//!   * a field the server could not derive renders as `-` / `null` **with its
//!     reason**, never as `0`; and
//!   * a fail-closed `502 authoritative_margin_unavailable` exits non-zero and
//!     says the balance is unknown — it must never look like an empty account.

use assert_cmd::Command;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Spec-shaped `GET /api/v1/account/summary` body. Money rides the exact `str`
/// decimal adapter; the counts are JSON numbers.
fn summary_body() -> Value {
    json!({
        "collateral": "1000", "total_equity": "1050",
        "total_unrealized_pnl": "50", "total_realized_pnl_24h": "10",
        "total_volume_24h": "5000", "open_positions_count": 1,
        "open_orders_count": 2, "margin_used": "200",
        "available_margin": "800", "withdrawable": "800",
        "early_access_allowed": true
    })
}

/// Spec-shaped position with the enriched risk detail. `leverage` is null with a
/// reason in `leverage_error` — the server's way of saying "not computable",
/// which must never be read as zero.
fn position_body() -> Value {
    json!({
        "market_id": "BTC-USDX-PERP", "side": "Buy", "size": "0.5",
        "entry_price": "80000", "unrealized_pnl": "50", "realized_pnl": "0",
        "liquidation_price": "60000",
        "leverage": null, "leverage_error": "margin_state_not_mirrored",
        "notional_value": "40050", "notional_value_error": null,
        "roe": "0.025", "roe_error": null,
        "margin_used": "2002.5", "margin_used_error": null,
        "max_leverage": 20, "max_leverage_error": null,
        "funding_paid": "1.25"
    })
}

fn fees_body() -> Value {
    json!({
        "maker_fee_bps": -2, "taker_fee_bps": 5, "tier": "base",
        "schedule": "standard", "volume_30d": "123456.78",
        "volume_30d_estimated": false, "discounts": []
    })
}

fn history_body() -> Value {
    json!({
        "window": "week",
        "cadence_ms": 3600000i64,
        "points": [
            {"timestamp_ms": 1_700_000_000_000i64, "equity": "1000",
             "pnl": "0", "volume": "0"},
            {"timestamp_ms": 1_700_003_600_000i64, "equity": "1050",
             "pnl": "50", "volume": "12000"}
        ]
    })
}

/// Start a mock server for the authenticated portfolio surface.
async fn mock_server() -> MockServer {
    let server = MockServer::start().await;
    for (p, body) in [
        ("/api/v1/account/summary", summary_body()),
        (
            "/api/v1/account/state",
            json!({"summary": summary_body(), "positions": [position_body()]}),
        ),
        ("/api/v1/account/fees", fees_body()),
        ("/api/v1/account/portfolio-history", history_body()),
        ("/api/v1/positions", json!([position_body()])),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
    }
    server
}

/// Build a credentialed `nexus` command pointed at the mock server. Env vars are
/// cleared so the developer's own configuration can't change what we assert.
fn nexus(base_url: &str, args: &[&str]) -> Command {
    let mut cmd = Command::cargo_bin("nexus").expect("`nexus` binary builds");
    cmd.env(
        "XDG_CONFIG_HOME",
        std::env::temp_dir().join("nexus-cli-portfolio-empty"),
    )
    .env_remove("NEXUS_OUTPUT")
    .env_remove("NEXUS_NETWORK")
    .env_remove("NEXUS_BASE_URL")
    .env_remove("NEXUS_SESSION_TOKEN")
    // The SDK requires a hex secret (it is the raw HMAC key). The value is
    // arbitrary — the mock does not verify signatures — but it must decode.
    .env("NEXUS_API_KEY", "nx_test")
    .env("NEXUS_API_SECRET", "00112233445566778899aabbccddeeff")
    .arg("--base-url")
    .arg(base_url)
    .args(args);
    cmd
}

async fn stdout_of(mut cmd: Command) -> String {
    tokio::task::spawn_blocking(move || {
        let out = cmd.assert().success();
        String::from_utf8(out.get_output().stdout.clone()).expect("stdout is utf-8")
    })
    .await
    .expect("command thread joins")
}

#[tokio::test]
async fn account_summary_surfaces_withdrawable() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["account", "summary"])).await;
    assert!(out.contains("withdrawable"), "missing label:\n{out}");
    assert!(out.contains("800"), "missing withdrawable value:\n{out}");
    assert!(
        out.contains("total equity") && out.contains("1050"),
        "{out}"
    );

    let out = stdout_of(nexus(
        &server.uri(),
        &["--output", "json", "account", "summary"],
    ))
    .await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["withdrawable"], json!("800"));
    assert_eq!(v["open_positions_count"], json!(1));
}

#[tokio::test]
async fn account_state_is_one_read_of_summary_and_positions() {
    let server = mock_server().await;
    let out = stdout_of(nexus(
        &server.uri(),
        &["--output", "json", "account", "state"],
    ))
    .await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["summary"]["withdrawable"], json!("800"));
    assert_eq!(v["positions"][0]["market_id"], json!("BTC-USDX-PERP"));

    // Exactly one request went out — the whole point of `account state` is that
    // the two halves cannot tear against each other.
    let requests = server
        .received_requests()
        .await
        .expect("wiremock records requests");
    assert_eq!(
        requests.len(),
        1,
        "account state must be a single request, got {}",
        requests.len()
    );
    assert_eq!(requests[0].url.path(), "/api/v1/account/state");
}

#[tokio::test]
async fn positions_surface_enriched_risk_fields() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["positions"])).await;
    assert!(out.contains("NOTIONAL") && out.contains("40050"), "{out}");
    assert!(
        out.contains("MARGIN USED") && out.contains("2002.5"),
        "{out}"
    );
    assert!(out.contains("ROE") && out.contains("0.025"), "{out}");
    assert!(out.contains("MAX LEV") && out.contains("20x"), "{out}");
    assert!(
        out.contains("FUNDING PAID") && out.contains("1.25"),
        "{out}"
    );
    // The one underivable field is explained rather than shown as a zero.
    assert!(
        out.contains("leverage (margin_state_not_mirrored): BTC-USDX-PERP"),
        "missing the reason a value is blank:\n{out}"
    );

    let out = stdout_of(nexus(&server.uri(), &["--output", "json", "positions"])).await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    let row = &v.as_array().expect("top-level array")[0];
    assert_eq!(row["notional_value"], json!("40050"));
    assert_eq!(row["max_leverage"], json!(20));
    assert_eq!(row["leverage"], Value::Null);
    assert_eq!(row["leverage_error"], json!("margin_state_not_mirrored"));
}

#[tokio::test]
async fn account_fees_labels_the_maker_rebate() {
    let server = mock_server().await;
    let out = stdout_of(nexus(&server.uri(), &["account", "fees"])).await;
    assert!(out.contains("-2 bps (rebate paid to you)"), "{out}");

    let out = stdout_of(nexus(
        &server.uri(),
        &["--output", "json", "account", "fees"],
    ))
    .await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["maker_fee_bps"], json!(-2));
    assert_eq!(v["volume_30d"], json!("123456.78"));
}

#[tokio::test]
async fn portfolio_history_forwards_window_and_limit() {
    let server = MockServer::start().await;
    // Matching on the query pins that the CLI forwards both parameters as the
    // API expects them — the request 404s (and the assertion fails) otherwise.
    Mock::given(method("GET"))
        .and(path("/api/v1/account/portfolio-history"))
        .and(query_param("window", "week"))
        .and(query_param("limit", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(history_body()))
        .mount(&server)
        .await;

    let out = stdout_of(nexus(
        &server.uri(),
        &[
            "account",
            "portfolio-history",
            "--window",
            "week",
            "--limit",
            "2",
        ],
    ))
    .await;
    assert!(out.contains("window          week"), "{out}");
    assert!(out.contains("cadence         3600000 ms"), "{out}");
    assert!(out.contains("2023-11-14T22:13:20Z"), "{out}");
    assert!(out.contains("2 point(s), oldest first."), "{out}");

    let out = stdout_of(nexus(
        &server.uri(),
        &[
            "--output",
            "json",
            "account",
            "portfolio-history",
            "--window",
            "week",
            "--limit",
            "2",
        ],
    ))
    .await;
    let v: Value = serde_json::from_str(&out).expect("stdout is valid JSON");
    assert_eq!(v["window"], json!("week"));
    assert_eq!(v["cadence_ms"], json!(3600000i64));
    assert_eq!(v["points"][1]["equity"], json!("1050"));
    assert_eq!(v["points"][1]["volume"], json!("12000"));
}

/// The fail-closed path. `/account/summary` and `/account/state` derive
/// `withdrawable` from the engine-authoritative margin view and return `502
/// authoritative_margin_unavailable` when that view is down, rather than serving
/// a local estimate. The CLI must exit non-zero and say the balance is unknown:
/// a script that read this as "0 withdrawable" would draw exactly the wrong
/// conclusion about an account that may be fully funded.
#[tokio::test]
async fn authoritative_margin_unavailable_is_not_an_empty_account() {
    let server = MockServer::start().await;
    for p in ["/api/v1/account/summary", "/api/v1/account/state"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(502).set_body_json(json!({
                "code": "authoritative_margin_unavailable"
            })))
            .mount(&server)
            .await;
    }

    for args in [
        &["account", "summary"][..],
        &["account", "state"],
        // The JSON mode must not print a document either — an empty object on
        // stdout is exactly the shape a script would misread.
        &["--output", "json", "account", "summary"],
    ] {
        let uri = server.uri();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let out = tokio::task::spawn_blocking(move || {
            let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
            nexus(&uri, &refs).output().expect("command runs")
        })
        .await
        .expect("command thread joins");

        assert!(!out.status.success(), "`{args:?}` must exit non-zero");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("UNKNOWN, not zero"),
            "`{args:?}` must say the balance is unknown, got: {stderr}"
        );
        assert!(
            stderr.contains("authoritative_margin_unavailable"),
            "`{args:?}` should surface the server's code, got: {stderr}"
        );
        assert!(
            out.stdout.is_empty(),
            "`{args:?}` must print no account data on failure, got: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
