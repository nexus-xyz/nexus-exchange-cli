//! End-to-end tests that run the compiled `nexus` binary and assert on its
//! behavior. These exercise the `main.rs` dispatch path — argument parsing,
//! credential resolution, the authenticated-command gate, and the local-only
//! commands — without reaching a real server.
//!
//! Network-bound commands are pointed at an unroutable `--base-url` and we only
//! assert that the request was *attempted* (a connection/transport failure),
//! which proves dispatch routed to the SDK without depending on a live API.

use std::process::{Command, Stdio};

/// Path to the binary under test, provided by Cargo for integration tests.
fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nexus"));
    // Keep tests hermetic: never read a developer's real config file.
    cmd.env(
        "XDG_CONFIG_HOME",
        std::env::temp_dir().join("nexus-cli-it-empty"),
    );
    cmd.env_remove("NEXUS_API_KEY");
    cmd.env_remove("NEXUS_API_SECRET");
    cmd.env_remove("NEXUS_NETWORK");
    cmd.env_remove("NEXUS_BASE_URL");
    cmd.env_remove("NEXUS_OUTPUT");
    cmd.stdin(Stdio::null());
    cmd
}

struct Output {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> Output {
    let out = bin().args(args).output().expect("failed to run binary");
    Output {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn help_lists_commands_and_exits_zero() {
    let out = run(&["--help"]);
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains("markets"));
    assert!(out.stdout.contains("order"));
    assert!(out.stdout.contains("Usage:"));
}

/// `--base-url` is marked deprecated in the help, and the help names the
/// replacement (ENG-10956).
///
/// Asserted on both `-h` and `--help`: clap renders the first line as short help
/// and the whole comment as long help, so a marker written only into the body
/// would be invisible to `-h` — which is the one most people type.
#[test]
fn help_marks_base_url_deprecated_and_names_the_replacement() {
    for flag in ["-h", "--help"] {
        let out = run(&[flag]);
        assert_eq!(out.code, Some(0), "`{flag}` should exit 0");

        let line = out
            .stdout
            .lines()
            .skip_while(|l| !l.contains("--base-url"))
            .take(3)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            line.contains("[deprecated]"),
            "`{flag}` must mark --base-url deprecated, got: {line}"
        );
        assert!(
            line.contains("--network"),
            "`{flag}` must name the replacement, got: {line}"
        );
    }
}

/// The deprecation notice goes to stderr and never to stdout — including under
/// `--output json`, where stdout is a document something else parses.
///
/// The notice is deliberately *not* suppressed for JSON callers: they are the
/// ones with a pinned invocation to migrate. Keeping it off stdout is what makes
/// that safe, so this asserts the split rather than the presence alone.
#[test]
fn a_base_url_override_warns_on_stderr_and_never_on_stdout() {
    // Port 1 is unroutable, so the command fails after the notice is printed.
    // What is under test is the notice and the stream it lands on, not the fetch.
    let out = run(&[
        "--base-url",
        "http://127.0.0.1:1",
        "--output",
        "json",
        "markets",
    ]);

    assert!(
        out.stderr.contains("deprecated"),
        "stderr should carry the notice, got: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains("--base-url"),
        "the notice should name the flag that was passed, got: {}",
        out.stderr
    );
    assert!(
        !out.stdout.contains("deprecated"),
        "the notice must never reach stdout, got: {}",
        out.stdout
    );
}

/// The ordinary path stays silent. A deprecation notice on every invocation is
/// how warnings stop being read.
#[test]
fn no_base_url_override_emits_no_deprecation_notice() {
    let out = run(&["--network", "testnet", "--help"]);
    assert!(
        !out.stderr.contains("ENG-10956"),
        "nothing should warn without an override, got: {}",
        out.stderr
    );
}

#[test]
fn version_flag_works() {
    let out = run(&["--version"]);
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains("nexus"));
}

#[test]
fn completions_emit_a_script_and_exit_zero() {
    // `completions` short-circuits before any network/config work.
    let out = run(&["completions", "bash"]);
    assert_eq!(out.code, Some(0));
    // The bash completion script references the binary name.
    assert!(out.stdout.contains("nexus"));
    assert!(!out.stdout.is_empty());
}

#[test]
fn unknown_command_is_a_usage_error() {
    let out = run(&["definitely-not-a-command"]);
    assert_eq!(out.code, Some(2)); // clap usage error
    assert!(out.stderr.contains("unrecognized") || out.stderr.contains("error"));
}

/// `account margin-mode` is withdrawn (ENG-7740). The binary must reject it as an
/// unknown subcommand — with credentials supplied, so this proves the rejection
/// happens at argument parsing and NOT that it merely stopped at the auth gate.
/// A pinned `--market` + mode pair is passed to make sure clap isn't silently
/// accepting them as positionals for some other command.
///
/// The old behaviour was worse than an error: the command parsed, passed the auth
/// gate, and dispatched `POST /account/margin-mode` — a path no spec defines and
/// no service routes — so the user got an opaque transport/HTTP failure from a
/// command that `--help` advertised as working.
#[test]
fn withdrawn_margin_mode_command_is_rejected() {
    let out = bin()
        .args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "--base-url",
            "http://127.0.0.1:1",
            "account",
            "margin-mode",
            "BTC-USDX-PERP",
            "isolated",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "should be a clap usage error, got stderr: {stderr}"
    );
    assert!(
        stderr.contains("unrecognized subcommand") || stderr.contains("margin-mode"),
        "the error should name the unrecognized subcommand, got: {stderr}"
    );
    // It must not have reached the network: no request may be attempted for a
    // command that cannot succeed.
    assert!(
        !stderr.contains("failed to set margin mode"),
        "must not dispatch a margin-mode request, got: {stderr}"
    );
}

/// The nine phantom commands are withdrawn (ENG-12369, under the fleet policy in
/// ENG-8616). Each targeted an operation no published spec version defines, and
/// each is checked here the way `margin-mode` is: with credentials supplied, so a
/// pass proves rejection at argument parsing rather than a stop at the auth gate,
/// and with the arguments the command used to accept so clap cannot be quietly
/// taking them as positionals for something else.
///
/// This is the revert-with-the-test-kept pin. Re-adding any of these commands
/// turns this test red on purpose. They come back when a PUBLISHED spec version
/// defines the operation — implemented against that version, listed in
/// endpoints.txt, and covered rather than exempted.
#[test]
fn withdrawn_phantom_commands_are_rejected() {
    let cases: &[(&[&str], &str)] = &[
        // POST /account/leverage — in no spec version; the venue serves
        // POST /leverage, so this path routed nowhere (ENG-7318).
        (&["account", "leverage", "BTC-USDX-PERP", "10"], "leverage"),
        // GET /funding-payments — in no spec version (ENG-3817).
        (&["funding-payments"], "funding-payments"),
        // /transfers, /sub-accounts — 404 live where documented routes 401, and in
        // no spec version (ENG-7800, ENG-8123).
        (&["transfers", "list"], "transfers"),
        (
            &[
                "transfers",
                "create",
                "--from",
                "a",
                "--to",
                "b",
                "--amount",
                "5",
            ],
            "transfers",
        ),
        (&["sub-accounts", "list"], "sub-accounts"),
        (&["sub-accounts", "create", "desk-1"], "sub-accounts"),
        // The ENG-5487 three — added on an "ahead of spec" claim that was never
        // true for any of them.
        (
            &["order", "cancel-batch", "o1", "o2", "--yes"],
            "cancel-batch",
        ),
        (
            &["order", "get-by-client-id", "ladder-1"],
            "get-by-client-id",
        ),
        (
            &["order", "cancel-by-client-id", "ladder-1", "--yes"],
            "cancel-by-client-id",
        ),
    ];
    for (args, name) in cases {
        let mut cmd = bin();
        cmd.args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "--base-url",
            "http://127.0.0.1:1",
        ]);
        cmd.args(*args);
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(2),
            "`{args:?}` should be a clap usage error, got stderr: {stderr}"
        );
        assert!(
            stderr.contains("unrecognized subcommand") || stderr.contains(name),
            "`{args:?}` should name the unrecognized subcommand, got: {stderr}"
        );
        // Nothing may reach the network: the whole point is that these cannot
        // succeed, so a dispatch attempt is the bug, not the error message.
        assert!(
            !stderr.contains("failed to"),
            "`{args:?}` must not dispatch a request, got: {stderr}"
        );
    }
}

/// `account help` must not advertise the withdrawn command, while still listing
/// the account commands that do work.
#[test]
fn account_help_omits_margin_mode() {
    let out = run(&["account", "--help"]);
    assert_eq!(out.code, Some(0));
    assert!(
        out.stdout.contains("deposit") && out.stdout.contains("rate-limit"),
        "sanity: working account commands should still be listed: {}",
        out.stdout
    );
    assert!(
        !out.stdout.contains("margin-mode"),
        "help must not offer the withdrawn margin-mode command: {}",
        out.stdout
    );
    // `leverage` was withdrawn alongside it in ENG-12369: POST /account/leverage
    // is in no spec version and routes nowhere (ENG-7318 documents the served
    // POST /leverage). Same rule, same assertion.
    assert!(
        !out.stdout.contains("leverage"),
        "help must not offer the withdrawn leverage command: {}",
        out.stdout
    );
}

#[test]
fn authenticated_command_without_credentials_is_refused() {
    // `balance` requires credentials; with none configured it must fail fast
    // with a clear message and a non-zero exit, never attempting an unsigned
    // request.
    let out = run(&["balance"]);
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("authenticated command") || out.stderr.contains("credentials"),
        "stderr: {}",
        out.stderr
    );
}

/// Every portfolio-parity read (ENG-6460) is account-scoped, so each must refuse
/// without credentials rather than send an unsigned request that comes back as an
/// opaque 401.
#[test]
fn portfolio_commands_without_credentials_are_refused() {
    for args in [
        &["account", "summary"][..],
        &["account", "state"],
        &["account", "fees"],
        &["account", "portfolio-history"],
    ] {
        let out = run(args);
        assert_ne!(out.code, Some(0), "`{args:?}` should be refused");
        assert!(
            out.stderr.contains("authenticated command"),
            "`{args:?}` stderr: {}",
            out.stderr
        );
    }
}

/// `--limit` is bounded before anything is signed: clap rejects an out-of-range
/// value as a usage error (exit 2), so it never reaches the network.
#[test]
fn portfolio_history_limit_out_of_range_is_a_usage_error() {
    for bad in ["0", "367"] {
        let out = bin()
            .args([
                "--api-key",
                "k",
                "--api-secret",
                "s",
                "account",
                "portfolio-history",
                "--limit",
                bad,
            ])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "--limit {bad} should be a usage error"
        );
    }
}

#[test]
fn order_place_without_credentials_is_refused_before_network() {
    let out = run(&[
        "order",
        "place",
        "--market",
        "BTC-USDX-PERP",
        "--side",
        "buy",
        "--type",
        "market",
        "--quantity",
        "0.01",
        "--yes",
    ]);
    assert_ne!(out.code, Some(0));
    assert!(out.stderr.contains("credentials") || out.stderr.contains("authenticated"));
}

#[test]
fn limit_order_requires_a_price() {
    // Provide credentials so we pass the auth gate and reach the price check.
    let out = bin()
        .args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "limit",
            "--quantity",
            "0.01",
            "--yes",
        ])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--price is required"), "stderr: {stderr}");
}

#[test]
fn bad_quantity_is_rejected_with_a_clear_message() {
    let out = bin()
        .args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "market",
            // `0` is non-positive; `=` form avoids clap reading a leading-dash
            // value as a flag, but zero needs no escaping.
            "--quantity",
            "0",
            "--yes",
        ])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("positive number"), "stderr: {stderr}");
}

#[test]
fn ws_rejects_an_unknown_channel() {
    let out = run(&["ws", "bogus-channel"]);
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("unknown channel"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn ws_public_channel_requires_market() {
    let out = run(&["ws", "trades"]);
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("requires --market"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn public_command_attempts_the_request_against_a_dead_endpoint() {
    // Point at a closed local port: dispatch must route to the SDK and surface
    // a transport failure (not a parse/auth error), proving the command wired
    // through to a real fetch.
    let out = run(&["--base-url", "http://127.0.0.1:1", "markets"]);
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("failed to fetch markets"),
        "stderr: {}",
        out.stderr
    );
}

#[test]
fn setup_refuses_without_a_terminal() {
    // stdin is null (not a tty), so interactive setup must refuse rather than
    // block or write an empty config.
    let out = run(&["setup"]);
    assert_ne!(out.code, Some(0));
    assert!(out.stderr.contains("interactive"), "stderr: {}", out.stderr);
}

#[test]
fn json_output_flag_is_accepted() {
    // Even when the fetch fails, `--output json` must parse and route.
    let out = run(&[
        "--output",
        "json",
        "--base-url",
        "http://127.0.0.1:1",
        "health",
    ]);
    assert_ne!(out.code, Some(0));
    assert!(out.stderr.contains("failed to fetch health"));
}

/// Every public market-data command must route through to the SDK and surface a
/// transport failure (proving the command is wired to a real fetch, not stubbed
/// or mis-dispatched). Each case is a command + the `context` string main.rs
/// attaches to the request, so a renamed/dropped handler is caught here.
///
/// This is the runtime counterpart to the static `endpoints.txt` <-> command
/// drift check (scripts/check_spec_drift.py): the drift check proves the mapping
/// is complete; these prove the mapped commands actually fire a request.
#[test]
fn public_commands_route_to_a_fetch() {
    let dead = "http://127.0.0.1:1";
    let cases: &[(&[&str], &str)] = &[
        (&["markets"], "failed to fetch markets"),
        (&["summaries"], "failed to fetch market summaries"),
        (&["tickers"], "failed to fetch tickers"),
        (&["ticker", "BTC-USDX-PERP"], "failed to fetch ticker"),
        (
            &["orderbook", "BTC-USDX-PERP"],
            "failed to fetch order book",
        ),
        (&["trades", "BTC-USDX-PERP"], "failed to fetch trades"),
        (&["candles", "BTC-USDX-PERP"], "failed to fetch candles"),
        (
            &["funding-rates", "BTC-USDX-PERP"],
            "failed to fetch funding rates",
        ),
        (
            &["mark-price", "BTC-USDX-PERP"],
            "failed to fetch mark price",
        ),
        (
            &["market-status", "BTC-USDX-PERP"],
            "failed to fetch market status",
        ),
        (&["health"], "failed to fetch health"),
    ];
    for (args, want) in cases {
        let mut full = vec!["--base-url", dead];
        full.extend_from_slice(args);
        let out = run(&full);
        assert_ne!(
            out.code,
            Some(0),
            "`{args:?}` should fail against a dead port"
        );
        assert!(
            out.stderr.contains(want),
            "`{args:?}` stderr should contain {want:?}, got: {}",
            out.stderr
        );
    }
}

/// Authenticated read commands, given credentials, must pass the auth gate and
/// route to a fetch (transport failure against the dead port), not refuse.
#[test]
fn authenticated_read_commands_route_to_a_fetch_when_credentialed() {
    let cases: &[(&[&str], &str)] = &[
        (&["balance"], "failed to fetch account balance"),
        (&["positions"], "failed to fetch positions"),
        (&["fills"], "failed to fetch fills"),
        (&["orders"], "failed to fetch open orders"),
        (&["withdrawals"], "failed to fetch withdrawals"),
        (&["account", "rate-limit"], "failed to fetch rate-limit"),
        (&["account", "summary"], "failed to fetch account summary"),
        (&["account", "state"], "failed to fetch account state"),
        (&["account", "fees"], "failed to fetch account fees"),
        (
            &["account", "portfolio-history"],
            "failed to fetch portfolio history",
        ),
        (
            &["account", "portfolio-history", "--window", "week"],
            "failed to fetch portfolio history",
        ),
        (&["keys", "list"], "failed to fetch API keys"),
        (&["agents", "list"], "failed to fetch agents"),
        (
            &["market", "adl-events", "BTC-USDX-PERP"],
            "failed to fetch ADL events",
        ),
        (
            &[
                "account",
                "adl-history",
                "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
            ],
            "failed to fetch ADL history",
        ),
    ];
    for (args, want) in cases {
        let mut cmd = bin();
        cmd.args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "--base-url",
            "http://127.0.0.1:1",
        ]);
        cmd.args(*args);
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_ne!(
            out.status.code(),
            Some(0),
            "`{args:?}` should fail (dead port)"
        );
        assert!(
            stderr.contains(want),
            "`{args:?}` should reach a fetch ({want:?}), got: {stderr}"
        );
        // It must NOT have stopped at the auth gate — credentials were supplied.
        assert!(
            !stderr.contains("authenticated command"),
            "`{args:?}` should pass the auth gate with credentials, got: {stderr}"
        );
    }
}

/// The `--market` flatten, given credentials and `--yes`, must route through the
/// SDK to a network attempt — proving the dispatch and confirmation wiring,
/// without a live server. The batch and by-client-id variants were withdrawn in
/// ENG-12369; `phantom_order_subcommands_are_withdrawn` pins their absence.
#[test]
fn cancel_variants_route_to_the_sdk_when_credentialed() {
    let cases: &[(&[&str], &str)] = &[(
        &["order", "cancel", "--market", "BTC-USDX-PERP", "--yes"],
        "failed to cancel orders in BTC-USDX-PERP",
    )];
    for (args, want) in cases {
        let mut cmd = bin();
        cmd.args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "--base-url",
            "http://127.0.0.1:1",
        ]);
        cmd.args(*args);
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_ne!(
            out.status.code(),
            Some(0),
            "`{args:?}` should fail (dead port)"
        );
        assert!(
            stderr.contains(want),
            "`{args:?}` should reach the SDK call ({want:?}), got: {stderr}"
        );
    }
}

/// Without credentials, the new authenticated commands are refused at the auth
/// gate — before any confirmation prompt or network attempt.
#[test]
fn new_authenticated_commands_are_gated_without_credentials() {
    for args in [
        ["market", "adl-events", "BTC-USDX-PERP"].as_slice(),
        ["account", "adl-history", "0xabc"].as_slice(),
    ] {
        let out = run(args);
        assert_ne!(out.code, Some(0), "`{args:?}` should be refused");
        assert!(
            out.stderr.contains("authenticated command") || out.stderr.contains("credentials"),
            "`{args:?}` stderr: {}",
            out.stderr
        );
    }
}

/// The `examples/batch_orders.json` recipe must parse as a valid order batch:
/// with credentials it gets past the file read/parse and the confirmation
/// (`--yes`) all the way to the network attempt, proving the example is current.
#[test]
fn batch_orders_example_parses_and_routes() {
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/batch_orders.json");
    let out = bin()
        .args([
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "--base-url",
            "http://127.0.0.1:1",
            "order",
            "batch",
            example,
            "--yes",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0));
    // Reached the SDK submit (transport failure), not a parse error.
    assert!(
        stderr.contains("failed to submit order batch"),
        "example should parse and route to the batch submit, got: {stderr}"
    );
}

/// `--tif post-only` must be accepted.
///
/// Post-only is a *time-in-force* on this API (`TimeInForce::PostOnly`), not a
/// boolean flag, and the SDK has carried it for a while — but `TifArg` listed
/// only gtc/ioc/fok, so the CLI was the one client that could not express it.
/// That reads as "the venue has no post-only" from the command line, which is
/// wrong: the spec's `OrderRequest.time_in_force` enum and the engine's
/// `TimeInForce` both include it.
///
/// Asserted through argument parsing rather than a live order: reaching the
/// credentials gate proves clap accepted the value and dispatch routed on it.
#[test]
fn tif_accepts_post_only() {
    let out = bin()
        .args([
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "sell",
            "--type",
            "limit",
            "--price",
            "84000",
            "--quantity",
            "0.01",
            "--tif",
            "post-only",
            "--yes",
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("invalid value"),
        "`--tif post-only` must parse; got: {stderr}"
    );
    // Hermetic env means no credentials, so the authenticated-command gate is
    // the expected stopping point — it proves parsing succeeded.
    assert!(
        stderr.contains("no credentials are configured"),
        "expected the credentials gate after successful parse; got: {stderr}"
    );
}

/// The full accepted set, so dropping a variant is a test failure rather than a
/// silently narrower CLI.
#[test]
fn tif_rejects_an_unknown_value_and_lists_every_supported_one() {
    let out = bin()
        .args([
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "sell",
            "--type",
            "limit",
            "--price",
            "84000",
            "--quantity",
            "0.01",
            "--tif",
            "not-a-tif",
            "--yes",
        ])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid value 'not-a-tif'"),
        "got: {stderr}"
    );
    for expected in ["gtc", "ioc", "fok", "post-only"] {
        assert!(
            stderr.contains(expected),
            "the error should list `{expected}` as supported; got: {stderr}"
        );
    }
}

// ───────────────── real-funds guardrails (ENG-6462) ─────────────────
//
// These run the real binary with no network access, which is the point: every
// guardrail below has to fire locally, before a request is built. The pinned
// SDK also refuses mainnet outright today, so a test asserting on a *response*
// could not distinguish a working guardrail from that blanket refusal.

/// Run with a config file written into a private `XDG_CONFIG_HOME`, so the
/// file layer — the one that is namespaced — can be exercised end to end.
fn run_with_config(config_json: &str, tag: &str, args: &[&str]) -> Output {
    let home = std::env::temp_dir().join(format!(
        "nexus-cli-it-{}-{}-{tag}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let dir = home.join("nexus");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("config.json"), config_json).expect("write config");

    let mut cmd = bin();
    cmd.env("XDG_CONFIG_HOME", &home);
    let out = cmd.args(args).output().expect("failed to run binary");
    let _ = std::fs::remove_dir_all(&home);
    Output {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn mainnet_announces_itself_on_stderr() {
    let out = run(&["--network", "mainnet", "markets"]);
    assert!(
        out.stderr.contains("REAL FUNDS"),
        "a real-funds network must announce itself; got: {}",
        out.stderr
    );
}

/// The default is play funds and says nothing. A banner on every invocation is
/// how a warning stops being read.
#[test]
fn play_funds_networks_stay_quiet() {
    for args in [
        vec!["--base-url", "http://127.0.0.1:1", "markets"],
        vec!["--network", "local", "markets"],
    ] {
        let out = run(&args);
        assert!(
            !out.stderr.contains("REAL FUNDS"),
            "{args:?} is play funds and must not warn; got: {}",
            out.stderr
        );
    }
}

/// The banner is a warning, not data: `--output json` has to stay parseable.
#[test]
fn the_mainnet_banner_never_touches_stdout() {
    let out = run(&["--output", "json", "--network", "mainnet", "markets"]);
    assert!(
        !out.stdout.contains("REAL FUNDS"),
        "the banner leaked into stdout: {}",
        out.stdout
    );
    assert!(out.stderr.contains("REAL FUNDS"));
}

#[test]
fn the_faucet_is_refused_on_mainnet() {
    let out = run(&["--network", "mainnet", "account", "credit"]);
    assert_ne!(out.code, Some(0), "the faucet must fail on mainnet");
    assert!(
        out.stderr.contains("account deposit"),
        "the refusal must name the real-funds alternative; got: {}",
        out.stderr
    );
    // Refused on the network axis, not deflected into a credentials error.
    assert!(
        !out.stderr.contains("no credentials are configured"),
        "the faucet refusal should not be masked by the auth gate; got: {}",
        out.stderr
    );
}

#[test]
fn the_faucet_still_works_on_play_funds() {
    // No credentials configured, so this stops at the auth gate — which proves
    // the faucet guard let it through rather than refusing on the network.
    let out = run(&["--network", "testnet", "account", "credit"]);
    assert!(
        out.stderr.contains("no credentials are configured"),
        "testnet should reach the auth gate, not a faucet refusal; got: {}",
        out.stderr
    );
}

/// A first mainnet trade with no terminal and no `--yes` must stop, rather than
/// silently trading because nobody was there to answer.
#[test]
fn a_first_mainnet_trade_refuses_without_an_acknowledgement() {
    let out = run(&[
        "--network",
        "mainnet",
        "--api-key",
        "k",
        "--api-secret",
        "s",
        "order",
        "place",
        "--market",
        "BTC-USDX-PERP",
        "--side",
        "buy",
        "--type",
        "market",
        "--quantity",
        "0.01",
    ]);
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("REAL FUNDS") && out.stderr.contains("--yes"),
        "the refusal should explain itself and name the escape hatch; got: {}",
        out.stderr
    );
}

/// The same trade on testnet must not acquire a real-funds prompt.
#[test]
fn a_testnet_trade_is_not_gated_by_the_acknowledgement() {
    let out = run(&[
        "--network",
        "testnet",
        "--api-key",
        "k",
        "--api-secret",
        "s",
        "order",
        "place",
        "--market",
        "BTC-USDX-PERP",
        "--side",
        "buy",
        "--type",
        "market",
        "--quantity",
        "0.01",
    ]);
    assert!(
        !out.stderr.contains("REAL FUNDS"),
        "play funds must not raise a real-funds prompt; got: {}",
        out.stderr
    );
}

/// ENG-6462's core guarantee, end to end: a config holding only testnet
/// credentials leaves a mainnet invocation unauthenticated instead of
/// presenting a key that is invalid there by construction.
#[test]
fn a_testnet_config_does_not_authenticate_a_mainnet_command() {
    let config = r#"{
        "network": "testnet",
        "networks": {
            "testnet": { "api_key": "nx_testnet", "api_secret": "shh" }
        }
    }"#;

    let out = run_with_config(config, "cross", &["--network", "mainnet", "balance"]);
    assert!(
        out.stderr.contains("no credentials are configured"),
        "the testnet key must not be offered to mainnet; got: {}",
        out.stderr
    );

    // ...while the network it was minted for still authenticates and gets as far
    // as the request itself.
    let out = run_with_config(config, "same", &["--network", "testnet", "balance"]);
    assert!(
        !out.stderr.contains("no credentials are configured"),
        "the testnet key must still work on testnet; got: {}",
        out.stderr
    );
}

/// A config written before namespacing keeps working, and its key stays on the
/// network that file names.
#[test]
fn a_pre_namespacing_config_still_authenticates() {
    let config = r#"{"network": "testnet", "api_key": "nx_old", "api_secret": "old"}"#;

    let out = run_with_config(config, "legacy-same", &["balance"]);
    assert!(
        !out.stderr.contains("no credentials are configured"),
        "a flat config must keep authenticating; got: {}",
        out.stderr
    );

    let out = run_with_config(config, "legacy-cross", &["--network", "mainnet", "balance"]);
    assert!(
        out.stderr.contains("no credentials are configured"),
        "a flat testnet key must not be promoted to mainnet; got: {}",
        out.stderr
    );
}

// ───────────────── custom networks (ENG-9827) ─────────────────
//
// End to end, through the real binary and a real config file, because the whole
// point of a custom network is what the *file* can express — a unit test that
// builds the struct in memory would skip the deserialization these entries have
// to survive. `example.com` / `example.invalid` are RFC 2606 reserved: no
// deployment of ours is named here, which is the reason the variant exists.

/// A config declaring one play-funds stage with a faucet, plus its credentials.
const DEV_STAGE: &str = r#"{
    "custom_networks": {
        "dev": {
            "base_url": "http://127.0.0.1:1",
            "funds": "play",
            "faucet": true
        }
    },
    "networks": {
        "dev": { "api_key": "nx_dev", "api_secret": "shh" }
    }
}"#;

/// The headline: a stage the CLI ships no knowledge of is selectable by label,
/// and authenticates with the credentials stored under that label.
#[test]
fn a_declared_stage_is_selectable_and_authenticates() {
    let out = run_with_config(DEV_STAGE, "custom-select", &["--network", "dev", "balance"]);
    assert!(
        !out.stderr.contains("no credentials are configured"),
        "the stage's own key must authenticate it; got: {}",
        out.stderr
    );
    // It is play funds, so nothing real-funds fires.
    assert!(
        !out.stderr.contains("REAL FUNDS"),
        "a play-funds stage must stay quiet; got: {}",
        out.stderr
    );
}

/// The credential-namespace guarantee, from the outside: a key stored for one
/// label is not offered to another, even when both point at the same host. This
/// is the collision the ticket is about — with the URL as the key, these two
/// would share a slot, and they have different funds semantics.
#[test]
fn two_stages_on_one_host_do_not_share_credentials() {
    let config = r#"{
        "custom_networks": {
            "one": { "base_url": "http://127.0.0.1:1", "funds": "play" },
            "two": { "base_url": "http://127.0.0.1:1", "funds": "play" }
        },
        "networks": {
            "one": { "api_key": "nx_one", "api_secret": "shh" }
        }
    }"#;

    let out = run_with_config(config, "ns-one", &["--network", "one", "balance"]);
    assert!(
        !out.stderr.contains("no credentials are configured"),
        "`one`'s key must authenticate `one`; got: {}",
        out.stderr
    );

    let out = run_with_config(config, "ns-two", &["--network", "two", "balance"]);
    assert!(
        out.stderr.contains("no credentials are configured"),
        "`one`'s key must not authenticate `two`, same host or not; got: {}",
        out.stderr
    );
}

/// A stage that declares real funds gets the full real-funds treatment under its
/// own name — the banner and the first-trade acknowledgement — even though the
/// CLI has never heard of it. This is what `is_mainnet()` could not express.
#[test]
fn a_real_funds_stage_is_guarded_like_mainnet() {
    let config = r#"{
        "custom_networks": {
            "dev": { "base_url": "http://127.0.0.1:1", "funds": "real" }
        }
    }"#;

    let out = run_with_config(config, "custom-real", &["--network", "dev", "markets"]);
    assert!(
        out.stderr.contains("REAL FUNDS") && out.stderr.contains("dev"),
        "a real-funds stage must announce itself under its own label; got: {}",
        out.stderr
    );

    let out = run_with_config(
        config,
        "custom-real-trade",
        &[
            "--network",
            "dev",
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "market",
            "--quantity",
            "0.01",
        ],
    );
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains("--yes"),
        "the first trade there must be gated; got: {}",
        out.stderr
    );
}

/// Acknowledging mainnet must not disarm the prompt for a private real-funds
/// stage. The acknowledgement is keyed per network precisely so this cannot
/// happen, and a single flag is what made it possible.
#[test]
fn a_mainnet_acknowledgement_does_not_cover_a_custom_stage() {
    let config = r#"{
        "mainnet_acknowledged": true,
        "custom_networks": {
            "dev": { "base_url": "http://127.0.0.1:1", "funds": "real" }
        }
    }"#;

    let out = run_with_config(
        config,
        "ack-not-shared",
        &[
            "--network",
            "dev",
            "--api-key",
            "k",
            "--api-secret",
            "s",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "market",
            "--quantity",
            "0.01",
        ],
    );
    assert_ne!(out.code, Some(0), "the stage's own prompt must still fire");
    assert!(
        out.stderr.contains("--yes"),
        "mainnet's acknowledgement must not cover `dev`; got: {}",
        out.stderr
    );
}

/// A stage that declares nothing about its funds refuses to mint them, rather
/// than assuming play funds. `Unknown` fails closed — that is the whole reason
/// the classification is tri-state.
#[test]
fn an_undeclared_funds_stage_refuses_the_faucet() {
    let config = r#"{
        "custom_networks": {
            "dev": { "base_url": "http://127.0.0.1:1", "faucet": true }
        }
    }"#;

    let out = run_with_config(
        config,
        "custom-unknown",
        &["--network", "dev", "account", "credit"],
    );
    assert_ne!(out.code, Some(0), "undeclared funds must refuse the faucet");
    assert!(
        out.stderr
            .contains("does not declare whether it moves real ones"),
        "the refusal must say why; got: {}",
        out.stderr
    );
    // ...and it says so before the invocation runs, too.
    assert!(
        out.stderr.contains("does not declare \"funds\""),
        "the missing declaration must be reported; got: {}",
        out.stderr
    );
}

/// Selecting a stage nothing declares stops the invocation and names what *is*
/// declared. A silent fallback here would send a request somewhere the user did
/// not ask for.
#[test]
fn an_undeclared_label_stops_the_invocation() {
    let out = run_with_config(
        DEV_STAGE,
        "custom-missing",
        &["--network", "other", "markets"],
    );
    assert_ne!(out.code, Some(0));
    assert!(
        out.stderr.contains(r#"declared: "dev""#),
        "the error should list what is declared; got: {}",
        out.stderr
    );
}

/// A label that could address another target's stored credentials never reaches
/// the credential store: it is refused by the flag parser, as a usage error.
#[test]
fn an_unsafe_label_is_refused_as_a_usage_error() {
    for bad in ["../mainnet", "one/two", "custom"] {
        let out = run_with_config(DEV_STAGE, "custom-unsafe", &["--network", bad, "markets"]);
        assert_eq!(
            out.code,
            Some(2),
            "--network {bad:?} should be a usage error; got: {}",
            out.stderr
        );
    }
}
