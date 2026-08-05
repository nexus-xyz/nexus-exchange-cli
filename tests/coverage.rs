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

/// `.api-version` feeds a raw.githubusercontent URL and a cache key in
/// `spec-drift.yml`, and `sdk-autobump.yml` derives it from the SDK crate. Both
/// validate it, but failing here names the problem sooner.
#[test]
fn api_version_is_a_pinned_semver_tag() {
    let tag = read(".api-version");
    let tag = tag.trim();
    let rest = tag
        .strip_prefix('v')
        .unwrap_or_else(|| panic!(".api-version must start with 'v', got {tag:?}"));
    let parts: Vec<&str> = rest.split('.').collect();
    assert!(
        (1..=3).contains(&parts.len()) && parts.iter().all(|c| c.parse::<u32>().is_ok()),
        ".api-version must be a vX, vX.Y or vX.Y.Z tag, got {tag:?}"
    );
}

const MARK_START: &str = "<!-- api-version-sync:start -->";
const MARK_END: &str = "<!-- api-version-sync:end -->";

/// The text between the sync markers, or a panic naming what's missing. Mirrors
/// `MANAGED_BLOCK_RE` in `scripts/sync_sdk_version.py`.
fn managed_block(readme: &str) -> &str {
    let start = readme
        .find(MARK_START)
        .unwrap_or_else(|| panic!("README.md is missing {MARK_START}"))
        + MARK_START.len();
    let end = readme
        .find(MARK_END)
        .unwrap_or_else(|| panic!("README.md is missing {MARK_END}"));
    assert!(
        end > start,
        "README.md has the api-version-sync markers in the wrong order"
    );
    &readme[start..end]
}

/// `scripts/sync_sdk_version.py --write` owns the marked README block, and fails
/// loudly if the markers are gone — which would kill an autobump run mid-flight.
/// Cheaper to notice here.
#[test]
fn readme_has_exactly_one_managed_api_version_block() {
    let readme = read("README.md");
    assert_eq!(
        readme.matches(MARK_START).count(),
        1,
        "expected exactly one {MARK_START}"
    );
    assert_eq!(
        readme.matches(MARK_END).count(),
        1,
        "expected exactly one {MARK_END}"
    );
    // A second block would rot: the script's regex substitutes with count=1.
    assert!(
        managed_block(&readme).contains("Currently targets Exchange API spec"),
        "the managed block no longer states the targeted spec version; \
         sync_sdk_version.py::render_managed_block defines its shape"
    );
}

/// The pin and the sentence a reader actually sees must agree. Nothing else
/// notices if they diverge — `spec-drift` reads `.api-version` and never looks at
/// the prose, so a stale line is a silently wrong claim about which spec version
/// the CLI targets and sends as `X-Nexus-Api-Version` (ENG-7962).
#[test]
fn readme_managed_line_matches_the_pinned_api_version() {
    let readme = read("README.md");
    let tag = read(".api-version").trim().to_string();
    let block = managed_block(&readme);
    assert!(
        block.contains(&format!("**`{tag}`**")),
        "README managed block says {block:?} but .api-version pins {tag}. Both are \
         derived from the `nexus-exchange` crate, so the fix is \
         `python3 scripts/sync_sdk_version.py --write` — not a hand-edit of either."
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
        ("GET", "/status"),
        ("GET", "/keys"),
        ("GET", "/ws"),
        ("POST", "/ws/token"),
        // `order amend`. Listed here deliberately: this op spent time misfiled as
        // ahead-of-spec under the wrong verb (PUT), which kept a covered operation
        // out of the manifest and out of the coverage count. The spec has had
        // `PATCH /orders/{order_id}` since v0.7.1 (ENG-7962).
        ("PATCH", "/orders/{order_id}"),
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
///
/// Keep this list to ops the pinned spec genuinely lacks. `PUT /orders/{order_id}`
/// used to head it, described as "amend" — but the SDK issues `PATCH` and the spec
/// has defined `PATCH /orders/{order_id}` since v0.7.1, so the entry asserted a
/// false claim and passed only because the verb didn't match either file
/// (ENG-7962). Invariant 3 of the drift check now fails a `CODE_ONLY_OPS` entry
/// whose path the spec defines under any verb, which is the real guard; this list
/// is the cheap offline echo of it.
#[test]
fn ahead_of_spec_ops_are_not_in_endpoints_txt() {
    let ops = endpoints();
    for absent in [
        ("POST", "/account/leverage"),
        ("GET", "/funding-payments"),
        ("POST", "/transfers"),
        ("GET", "/sub-accounts"),
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

/// `POST /account/margin-mode` is not "ahead of the pinned spec" — it has never
/// existed in any spec version and no service routes it. The `account
/// margin-mode` command that targeted it was withdrawn in ENG-7740, so the op
/// must not reappear in *any* of the drift artifacts: not endpoints.txt (it isn't
/// in the spec), and not the drift script's METHOD_OP / CODE_ONLY_OPS tables
/// (nothing calls `set_margin_mode` any more, and listing it there would let the
/// command be re-added silently).
///
/// This is the artifact half of the withdrawal; `src/cli.rs` holds the
/// command-surface half. ENG-7614 must land before either is reversed.
#[test]
fn margin_mode_is_absent_from_every_drift_artifact() {
    let ops = endpoints();
    assert!(
        !ops.contains(&("POST".to_string(), "/account/margin-mode".to_string())),
        "endpoints.txt must not list POST /account/margin-mode — it is in no spec version"
    );

    let drift = read("scripts/check_spec_drift.py");
    // Strip comment lines: the tables must be clean, but the prose explaining
    // *why* the op is absent is expected to name it.
    let code: String = drift
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in ["set_margin_mode", "/account/margin-mode"] {
        assert!(
            !code.contains(needle),
            "scripts/check_spec_drift.py still maps {needle:?} outside a comment; \
             the withdrawn margin-mode op must not be in METHOD_OP or CODE_ONLY_OPS"
        );
    }

    // And no command handler may call the SDK method again.
    for src in ["src/main.rs", "src/cli.rs"] {
        let body = read(src);
        let code: String = body
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("set_margin_mode"),
            "{src} calls set_margin_mode outside a comment; the command is withdrawn (ENG-7740)"
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
