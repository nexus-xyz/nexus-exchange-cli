//! Standing guards on `release-please-config.json` — the file that decides what
//! version the next release cuts.
//!
//! This repo has been bitten by release config twice: the `release-as` bootstrap
//! (ENG-4341) and the phantom releases it caused, then a missing pre-1.0 pair
//! that had release-please proposing `1.0.0` off a single `BREAKING CHANGE:`
//! footer (ENG-7411). Both were one-line edits with no test in the way, and
//! neither is visible until a release PR appears with the wrong number on it.
//!
//! These tests are the cheap insurance: they run in the existing `cargo test`
//! job, need no network, and fail the moment an edit re-arms either bug. They
//! assert *policy*, not the current version — bumping `0.3.0` keeps them green.
//!
//! The policy itself is `AGENTS.md`'s: "Pre-1.0 versioning: bump minor on
//! breaking changes, patch on features/fixes", the same rule configured in
//! nexus-exchange-ts and nexus-exchange-mcp.

use serde_json::Value;

fn read(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn config() -> Value {
    serde_json::from_str(&read("release-please-config.json"))
        .expect("release-please-config.json must be valid JSON")
}

/// The keys of `packages`, asserted non-empty.
///
/// Every per-package check below loops over this, so an empty (or missing)
/// `packages` map would make them all vacuously pass — the exact silent-hole the
/// guards exist to prevent. `packages` is `required` in the upstream schema, so
/// this also catches an edit that drops it entirely.
fn package_names(cfg: &Value) -> Vec<String> {
    let packages = cfg
        .get("packages")
        .and_then(Value::as_object)
        .expect("release-please-config.json must have a `packages` object");
    assert!(
        !packages.is_empty(),
        "`packages` is empty — release-please would release nothing, and every \
         per-package guard in this file would pass vacuously"
    );
    packages.keys().cloned().collect()
}

/// Resolve a key the way release-please does: the per-package entry wins, and
/// the root acts as the default for anything the package doesn't set.
///
/// Both levels take the same `ReleaserConfigOptions`, so checking only the root
/// would miss a package-level override that quietly reverses the policy — and
/// checking only the package would miss the root default it inherits.
///
/// Deliberately plain `get` lookups rather than `Value::pointer`: package keys
/// are paths, and `/` and `~` are JSON Pointer syntax. A monorepo-style key like
/// `packages/core` would resolve to nothing and silently fall back to the root —
/// leaving the per-package check inert in exactly the case it exists for.
fn effective<'a>(cfg: &'a Value, package: &str, key: &str) -> Option<&'a Value> {
    cfg.get("packages")
        .and_then(|packages| packages.get(package))
        .and_then(|entry| entry.get(key))
        .or_else(|| cfg.get(key))
}

/// Every `release-as` in the config tree, as JSON paths.
///
/// Recursive rather than a top-level check, because the key is valid at the root
/// *and* inside any `packages` entry — a nested one is exactly as sticky.
fn find_release_as(node: &Value, path: &str) -> Vec<String> {
    match node {
        Value::Object(map) => map
            .iter()
            .flat_map(|(k, v)| {
                if k == "release-as" {
                    vec![format!("{path}.{k}")]
                } else {
                    find_release_as(v, &format!("{path}.{k}"))
                }
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .enumerate()
            .flat_map(|(i, v)| find_release_as(v, &format!("{path}[{i}]")))
            .collect(),
        _ => Vec::new(),
    }
}

/// `release-as` forces every release to one literal version. It is a ONE-SHOT
/// override: committing it freezes the version permanently, so release-please
/// re-proposes an already-published version forever and no later release can be
/// cut. That is ENG-4341 in this repo (and ENG-7413 in nexus-exchange-ts).
///
/// The supported way to force a single version is the `Release-As:` footer on
/// one commit, which applies once and leaves no committed state behind.
#[test]
fn release_as_is_never_committed() {
    let found = find_release_as(&config(), "$");
    assert!(
        found.is_empty(),
        "`release-as` must never be committed — it freezes the version and \
         blocks every later release. Found at: {}. For a one-off version, use \
         the `Release-As: x.y.z` commit footer instead.",
        found.join(", ")
    );
}

/// The ENG-7411 pair. Without both, stock semver applies and the first
/// `BREAKING CHANGE:` footer proposes `1.0.0`:
///
/// - `bump-minor-pre-major`       — breaking → the `X` in `0.X.Y`, not major
/// - `bump-patch-for-minor-pre-major` — `feat:` → the `Y`, not minor
///
/// Both are inert once the version reaches `1.0.0` (release-please gates them on
/// `major < 1`), so they cannot pin anything. Deliberately going 1.0 means
/// deleting them *and* this test — which is the point: it makes that a decision
/// someone takes on purpose rather than a side effect of tidying the config.
#[test]
fn pre_1_0_bump_policy_stays_configured() {
    let cfg = config();
    for key in ["bump-minor-pre-major", "bump-patch-for-minor-pre-major"] {
        // Set at the root, so it is the default for any package added later.
        assert_eq!(
            cfg.get(key),
            Some(&Value::Bool(true)),
            "release-please-config.json must set `{key}: true` at the root to \
             keep the CLI on 0.X.Y (ENG-7411)"
        );
        // ...and not reversed by a per-package override.
        for package in package_names(&cfg) {
            assert_eq!(
                effective(&cfg, &package, key),
                Some(&Value::Bool(true)),
                "package {package:?} overrides `{key}` to a non-true value, \
                 which re-arms the 1.0.0 bump for that package"
            );
        }
    }
}

/// A third way to reach the same bug. The `versioning` strategy is applied
/// *instead of* the default bump logic, so `always-bump-major` would cut `1.0.0`
/// with both pre-major flags still set and apparently correct.
#[test]
fn versioning_strategy_stays_default() {
    let cfg = config();
    for package in package_names(&cfg) {
        let versioning = effective(&cfg, &package, "versioning");
        let is_default = match versioning {
            None => true,
            Some(v) => v.as_str() == Some("default"),
        };
        assert!(
            is_default,
            "package {package:?} sets `versioning` to {versioning:?}; only the \
             `default` strategy honours the pre-1.0 bump flags (e.g. \
             `always-bump-major` would cut 1.0.0 regardless)"
        );
    }
}

/// The `v` in the tag is load-bearing for distribution, not cosmetic:
/// `[package.metadata.binstall]` in Cargo.toml reconstructs the artifact URL as
/// `releases/download/v{version}/...`, so a tag cut without the `v` uploads
/// artifacts where `cargo binstall` will not look.
///
/// `include-v-in-tag` defaults to `true`; this guards against someone setting it
/// to `false` at either level.
#[test]
fn tags_keep_their_v_prefix() {
    let cfg = config();
    for package in package_names(&cfg) {
        assert_ne!(
            effective(&cfg, &package, "include-v-in-tag"),
            Some(&Value::Bool(false)),
            "package {package:?} sets `include-v-in-tag: false`; tags must stay \
             `vX.Y.Z` because binstall builds artifact URLs from `download/v{{version}}` \
             (see [package.metadata.binstall] in Cargo.toml)"
        );
    }
}

/// The `version` under `[package]` in Cargo.toml.
///
/// Hand-scanned rather than parsed: `toml` is not a dependency, and Cargo.toml
/// carries several other `version = ` lines (dependencies, metadata) that a
/// naive search would hit first.
fn cargo_toml_version() -> String {
    let text = read("Cargo.toml");
    let mut in_package = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = line.strip_prefix("version") {
                if let Some(v) = rest.trim_start().strip_prefix('=') {
                    return v.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    panic!("could not find `version` in Cargo.toml's [package] section");
}

/// release-please bumps `Cargo.toml` and `.release-please-manifest.json`
/// together. If they drift, the manifest is what release-please believes the
/// last release was, so the next proposal is computed off the wrong baseline —
/// while the binary reports the other number in `nexus --version`.
#[test]
fn cargo_toml_and_release_manifest_agree() {
    let version = cargo_toml_version();
    assert!(
        version.split('.').count() == 3 && version.split('.').all(|c| c.parse::<u32>().is_ok()),
        "Cargo.toml version must be X.Y.Z, got {version:?}"
    );

    let manifest: Value = serde_json::from_str(&read(".release-please-manifest.json"))
        .expect(".release-please-manifest.json must be valid JSON");

    // The manifest is keyed by the same package paths as the config.
    let cfg = config();
    for package in package_names(&cfg) {
        assert_eq!(
            manifest.get(&package).and_then(Value::as_str),
            Some(version.as_str()),
            "package {package:?}: .release-please-manifest.json and Cargo.toml \
             have drifted — release-please would compute the next version off \
             the wrong baseline"
        );
    }
}
