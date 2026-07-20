//! Build script: bake the compiled-against spec tag and the resolved
//! `nexus-exchange` SDK version into the binary so `nexus --version` can report
//! them with no runtime file I/O.
//!
//! Both values come from files already committed to the repo — the pinned
//! `.api-version` spec tag and the version `Cargo.lock` resolved for the SDK —
//! so what `--version` prints can never drift from what the build actually
//! links against. We always emit both env vars (falling back to `unknown`) so
//! the `env!` reads in the crate can never fail the build if a file is missing.

use std::path::Path;

fn main() {
    // `CARGO_MANIFEST_DIR` is the package root, where both files live. Fall back
    // to "." so a stripped-down build environment still produces a binary.
    let root = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());

    // Spec tag: repo-root `.api-version` (e.g. "v0.7.1\n"), trimmed. This is the
    // spec the CLI is compiled against; the rs SDK pins the same tag and sends
    // it as `X-Nexus-Api-Version` on every request, so it mirrors the wire.
    println!("cargo:rerun-if-changed=.api-version");
    let spec_tag = std::fs::read_to_string(Path::new(&root).join(".api-version"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=NEXUS_SPEC_TAG={spec_tag}");

    // Resolved SDK version from `Cargo.lock` — the exact version this binary
    // links, not the semver requirement in `Cargo.toml`.
    println!("cargo:rerun-if-changed=Cargo.lock");
    let sdk_version = std::fs::read_to_string(Path::new(&root).join("Cargo.lock"))
        .ok()
        .and_then(|lock| sdk_version_from_lock(&lock))
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=NEXUS_SDK_VERSION={sdk_version}");
}

/// Extract the resolved `nexus-exchange` version from `Cargo.lock`.
///
/// The lockfile is a sequence of `[[package]]` tables; within a table `name`
/// precedes `version`. We match the name exactly so neither our own
/// `nexus-exchange-cli` crate nor any `nexus-exchange-*` sibling is mistaken
/// for the SDK, and return the first matching table's version.
///
/// Keys are parsed as `key = value` split on the first `=` and trimmed, rather
/// than by a fixed `"name = "` prefix, so the scan tolerates any spacing Cargo
/// might write. Lines without `=` (table headers, `dependencies` entries) are
/// skipped, and only `name`/`version` are acted on.
fn sdk_version_from_lock(lock: &str) -> Option<String> {
    let mut in_target = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_target = false;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "name" => in_target = value == "nexus-exchange",
            "version" if in_target => return Some(value.to_string()),
            _ => {}
        }
    }
    None
}
