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
