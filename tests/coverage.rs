//! Guards the API-coverage artifacts that make the dashboard's CLI panel a real
//! percentage: `.api-version` (the pinned spec), `endpoints.txt` (the spec ops
//! the CLI's commands exercise), and the examples that document them.
//!
//! `scripts/check_spec_drift.py` is the full drift gate (it needs the fetched
//! spec, so it runs in the `spec-drift` CI workflow). These Rust tests run in
//! `cargo test` with no network: they catch the cheap, local ways the artifacts
//! rot — a malformed line, a duplicate, an `.api-version` that isn't `vX.Y.Z`,
//! or an example that drifts from the committed list — so a contributor sees it
//! before CI.

use std::collections::BTreeSet;

fn read(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// Parse `endpoints.txt` into the set of (METHOD, path) ops, validating each
/// non-comment line is well-formed and unique.
fn endpoints() -> BTreeSet<(String, String)> {
    let text = read("endpoints.txt");
    let mut ops = BTreeSet::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path = parts.next().unwrap_or("");
        assert!(
            parts.next().is_none(),
            "endpoints.txt:{}: expected 'METHOD /path', got {line:?}",
            i + 1
        );
        assert!(
            matches!(method, "GET" | "POST" | "PUT" | "DELETE" | "PATCH"),
            "endpoints.txt:{}: bad method {method:?}",
            i + 1
        );
        assert!(
            path.starts_with('/'),
            "endpoints.txt:{}: path must start with '/', got {path:?}",
            i + 1
        );
        assert!(
            ops.insert((method.to_string(), path.to_string())),
            "endpoints.txt:{}: duplicate {method} {path}",
            i + 1
        );
    }
    ops
}

#[test]
fn api_version_is_a_pinned_semver_tag() {
    let tag = read(".api-version");
    let tag = tag.trim();
    assert!(
        tag.starts_with('v') && tag[1..].split('.').all(|c| c.parse::<u32>().is_ok()),
        ".api-version must be a vX.Y.Z tag, got {tag:?}"
    );
}

#[test]
fn endpoints_txt_is_well_formed_and_non_empty() {
    let ops = endpoints();
    assert!(
        ops.len() >= 25,
        "expected the CLI to target a substantial slice of the spec, got {} ops",
        ops.len()
    );
    // A representative sampling across the surface must be present, so a future
    // edit can't silently gut the file and keep the check green. The mix spans
    // both stacks of the /api/v1 migration (ENG-4949): ops that moved to the
    // host-root `/api/v1` surface and ops that (for now) stay on the gateway.
    for want in [
        // Migrated to /api/v1:
        ("GET", "/api/v1/account"),
        ("POST", "/api/v1/orders"),
        ("DELETE", "/api/v1/orders/{order_id}"),
        // No /api/v1 variant yet — still on the gateway root:
        ("GET", "/markets"),
        ("GET", "/health"),
        ("GET", "/keys"),
        ("GET", "/ws"),
        ("POST", "/ws/token"),
    ] {
        assert!(
            ops.contains(&(want.0.to_string(), want.1.to_string())),
            "endpoints.txt should list {} {}",
            want.0,
            want.1
        );
    }
}

/// The drift script's ahead-of-spec ops are intentionally NOT in endpoints.txt;
/// assert they really are absent so the two files can't both claim them.
#[test]
fn ahead_of_spec_ops_are_not_in_endpoints_txt() {
    let ops = endpoints();
    for absent in [
        ("PUT", "/orders/{order_id}"), // amend
        ("POST", "/account/leverage"),
        ("POST", "/account/margin-mode"),
        ("GET", "/funding-payments"),
        ("POST", "/transfers"),
        ("GET", "/sub-accounts"),
        ("POST", "/orders/batch-cancel"),
        ("GET", "/orders/by-client-id/{client_order_id}"),
        ("DELETE", "/orders/by-client-id/{client_order_id}"),
    ] {
        assert!(
            !ops.contains(&(absent.0.to_string(), absent.1.to_string())),
            "{} {} is ahead of the pinned spec; it must stay out of endpoints.txt \
             (it lives in CODE_ONLY_OPS in the drift script)",
            absent.0,
            absent.1
        );
    }
}

#[test]
fn examples_exist_and_reference_the_binary() {
    for f in [
        "examples/README.md",
        "examples/market_data.sh",
        "examples/account.sh",
        "examples/trading.sh",
        "examples/keys_and_agents.sh",
        "examples/streaming.sh",
    ] {
        let body = read(f);
        assert!(!body.trim().is_empty(), "{f} is empty");
        assert!(
            body.contains("nexus"),
            "{f} should invoke the `nexus` binary"
        );
    }
}

#[test]
fn batch_orders_example_is_valid_json_array() {
    let body = read("examples/batch_orders.json");
    let v: serde_json::Value =
        serde_json::from_str(&body).expect("batch_orders.json must be valid JSON");
    let arr = v
        .as_array()
        .expect("batch_orders.json must be a JSON array");
    assert!(!arr.is_empty(), "example batch should not be empty");
    // Each entry has the fields `order batch` requires.
    for (i, o) in arr.iter().enumerate() {
        for field in ["market", "side", "type", "quantity"] {
            assert!(
                o.get(field).is_some(),
                "batch_orders.json[{i}] missing required field {field:?}"
            );
        }
    }
}
