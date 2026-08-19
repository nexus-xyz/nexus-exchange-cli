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
//! They cover two concerns with different lifetimes, which matters when someone
//! eventually takes the CLI to 1.0:
//!
//! 1. **Pre-1.0 versioning** — `pre_1_0_bump_policy_stays_configured` and
//!    `versioning_strategy_stays_default`. This is `AGENTS.md`'s policy: "Pre-1.0
//!    versioning: bump minor on breaking changes, patch on features/fixes", the
//!    same rule configured in nexus-exchange-ts and nexus-exchange-mcp. Going 1.0
//!    means deleting the two flags *and* the first of these tests — deliberate,
//!    which is the point.
//! 2. **Distribution invariants** — `tag_shape_stays_binstall_compatible`,
//!    `release_starts_as_draft_for_the_dist_handoff`,
//!    `release_as_is_never_committed`, `cargo_toml_and_release_manifest_agree`,
//!    and `windows_target_keeps_shipping` (the one guard here that reads
//!    `dist-workspace.toml` rather than the release-please config).
//!    These have nothing to do with being pre-1.0 and stay correct at every
//!    version. Do not delete them along with the pre-1.0 pair.
//!
//! A note on the assertion style: some guards reject a specific wrong value while
//! others require a specific right one. That tracks the upstream default in each
//! case — where the default is itself the broken setting (`include-component-in-tag`
//! is `true`, `draft` is `false`), an *omitted* key is as harmful as a wrong one,
//! so those assert the exact value rather than merely "not the wrong one".

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

/// `last-release-sha` is a STATIC floor, and a static floor goes stale the moment
/// the next release ships.
///
/// It was added in #36 to stop phantom release PRs: the `v0.1.0` bootstrap cut an
/// empty changelog, so with no recognised release boundary the note-collection walk
/// ran to the repo root every run and manufactured a minor bump from commits that
/// had already shipped. Pinning the floor to the `v0.3.0` release commit stopped
/// that — until 0.4.0 actually shipped, at which point the pin named the
/// release-before-last and release-please re-proposed all of 0.4.0's contents as
/// 0.5.0 (#63). Same defect, one floor higher.
///
/// The boundary release-please should use is the `v<version>` tag, which moves on
/// its own. That only works if the tag exists when the release PR is built, which
/// is why `release-please.yml` creates the tag *between* its two invocations — a
/// draft Release carries no tag, so a same-invocation `release-pr` sees an
/// unreleased repo and reaches for exactly this kind of floor.
///
/// So: no static floor, anywhere in the config. If a bootstrap ever needs one
/// again, it belongs in a single run, not in a committed file.
#[test]
fn no_static_release_floor_is_committed() {
    let cfg = config();
    // Both spellings the schema accepts, at the root and per-package: `packages`
    // inherits root keys, so checking only one level would leave a hole.
    let floors = ["last-release-sha", "bootstrap-sha"];
    for key in floors {
        assert!(
            cfg.get(key).is_none(),
            "`{key}` must not be committed: it pins the changelog boundary to one \
             commit, which is correct only until the next release ships — then \
             release-please re-proposes the release it just cut (#63). The \
             `v<version>` tag is the boundary that moves on its own; \
             release-please.yml creates it before building the release PR so \
             discovery works without a floor."
        );
        for pkg in package_names(&cfg) {
            assert!(
                effective(&cfg, &pkg, key).is_none(),
                "`{key}` must not be committed (found for package {pkg:?}) — see \
                 the root-level assertion above for why."
            );
        }
    }
}

/// The release PR must be built only after the git tag exists.
///
/// This is the other half of [`no_static_release_floor_is_committed`], and the
/// reason the workflow calls the action twice. A draft Release does not
/// materialise its git tag, so a `release-pr` that runs in the same invocation as
/// the Release finds no `v<version>`, concludes the repo has never been released,
/// and re-collects already-shipped commits. Asserting the *shape* of the workflow
/// rather than trusting a comment, because collapsing the two invocations back
/// into one would look like a tidy-up and would silently restore #63.
#[test]
fn the_release_pr_is_built_after_the_tag_exists() {
    let wf = read(".github/workflows/release-please.yml");
    let tag_step = wf.find("Create release tag").expect(
        "release-please.yml must still create the tag itself: a draft \
                 Release does not materialise one, and release.yml listens on tags",
    );
    let skip_pr = wf.find("skip-github-pull-request: true").expect(
        "the release-cutting invocation must set \
                 `skip-github-pull-request: true` so the release PR is not built \
                 before the tag exists (#63)",
    );
    let skip_release = wf.find("skip-github-release: true").expect(
        "the PR-opening invocation must set `skip-github-release: true` \
                 so it only grooms the PR",
    );

    assert!(
        skip_pr < tag_step,
        "the release must be cut BEFORE the tag step; found the \
         `skip-github-pull-request` invocation after it"
    );
    assert!(
        tag_step < skip_release,
        "the release PR must be opened AFTER the tag is created, or discovery \
         finds no release and re-proposes already-shipped commits (#63)"
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

/// The tag must be exactly `vX.Y.Z`, which is load-bearing for distribution
/// rather than cosmetic: `[package.metadata.binstall]` in Cargo.toml
/// reconstructs the artifact URL as `releases/download/v{version}/...`, so any
/// other tag shape uploads artifacts where `cargo binstall` will not look.
///
/// Two keys control that shape, and they need *opposite* assertions because
/// their upstream defaults differ:
///
/// - `include-v-in-tag` defaults to `true`, so it is enough to reject an
///   explicit `false` — dropping the key keeps the `v`.
/// - `include-component-in-tag` defaults to **`true`**, which would tag
///   `nexus-exchange-cli-v0.4.0`. The `false` in this config is what suppresses
///   that, so *deleting* the key breaks binstall exactly as thoroughly as
///   dropping the `v` does. It must therefore be present and explicitly `false`,
///   not merely "not true".
#[test]
fn tag_shape_stays_binstall_compatible() {
    let cfg = config();
    for package in package_names(&cfg) {
        assert_ne!(
            effective(&cfg, &package, "include-v-in-tag"),
            Some(&Value::Bool(false)),
            "package {package:?} sets `include-v-in-tag: false`; tags must stay \
             `vX.Y.Z` because binstall builds artifact URLs from `download/v{{version}}` \
             (see [package.metadata.binstall] in Cargo.toml)"
        );
        assert_eq!(
            effective(&cfg, &package, "include-component-in-tag"),
            Some(&Value::Bool(false)),
            "package {package:?} must set `include-component-in-tag: false` \
             (upstream defaults it to true, so an omitted key counts as broken). \
             Otherwise tags become `<component>-vX.Y.Z` and binstall, which \
             builds artifact URLs from `download/v{{version}}`, will not find them \
             (see [package.metadata.binstall] in Cargo.toml)"
        );
    }
}

/// The draft handoff between release-please and cargo-dist.
///
/// `release.yml` does not create its own GitHub Release — dist runs with
/// `create-release = false` and its header states the Release "is assumed to
/// exist as a draft", which it uploads signed artifacts into and then undrafts
/// (`gh release edit "$tag" ... --draft=false`). The `draft: true` here is what
/// creates it in that state.
///
/// `draft` defaults to `false` upstream, so an omitted key is as broken as an
/// explicit `false`: the tag would publish a Release immediately, announcing it
/// before any artifact is signed and uploaded — and the undraft step would have
/// nothing left to hand off.
#[test]
fn release_starts_as_draft_for_the_dist_handoff() {
    let cfg = config();
    for package in package_names(&cfg) {
        assert_eq!(
            effective(&cfg, &package, "draft"),
            Some(&Value::Bool(true)),
            "package {package:?} must set `draft: true` (upstream defaults it to \
             false, so an omitted key counts as broken). release.yml assumes the \
             Release already exists as a draft, uploads signed artifacts into it, \
             and only then undrafts it"
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

/// The whole `targets = [...]` value from `dist-workspace.toml`, brackets included.
///
/// Spans lines instead of matching one. The array is single-line today only by
/// accident of width — it is already 140 characters in a file whose every comment
/// stops at 80 — and the likeliest reason to edit it is *adding* a target, which
/// is precisely what tips it over into being wrapped. A line-scoped match would
/// then see `targets = [`, and the guard below would report the Windows target as
/// missing at the moment someone added a second Windows one.
fn dist_targets_array(text: &str) -> String {
    let mut lines = text.lines().skip_while(|line| {
        let line = line.trim_start();
        !(line.starts_with("targets") && line["targets".len()..].trim_start().starts_with('='))
    });

    let mut array = lines
        .next()
        .expect("could not find a `targets = [...]` key in dist-workspace.toml")
        .trim()
        .to_string();

    while !array.contains(']') {
        let next = lines
            .next()
            .expect("`targets` in dist-workspace.toml opens an array that never closes");
        array.push(' ');
        array.push_str(next.trim());
    }

    array
}

/// The Windows target keeps shipping even though it is no longer signed.
///
/// ENG-9357 removed `ssldotcom-windows-sign` from `dist-workspace.toml` and left
/// `x86_64-pc-windows-msvc` in `targets` on purpose: signing went, the platform
/// stayed. Those two lines now sit ~20 lines apart in the same file, under a
/// comment block explaining why Windows is not worth a purchased certificate —
/// which is exactly the reading that makes dropping the target look like
/// finishing the job.
///
/// Guarded because that mistake is *silent*. Every other way to break a release
/// turns CI red; this one leaves the pipeline green and simply stops producing
/// Windows artifacts, and nobody is watching a platform at 0.2% of downloads.
/// The same lapse in `Release` itself sat unnoticed for ten days.
///
/// Re-enabling signing later does not conflict with this — it adds a key back,
/// it does not remove the target. Dropping Windows deliberately means deleting
/// this test, which is the point.
#[test]
fn windows_target_keeps_shipping() {
    let targets = dist_targets_array(&read("dist-workspace.toml"));

    assert!(
        targets.contains("x86_64-pc-windows-msvc"),
        "dist-workspace.toml no longer builds `x86_64-pc-windows-msvc`. ENG-9357 \
         removed Authenticode *signing* while keeping the artifact: Windows users \
         still download a .zip and .msi, now carrying only a .minisig and a build \
         provenance attestation. Dropping the target ships nothing at all for \
         Windows, and does it quietly — no job fails, the release just comes out \
         short. Found: {targets}"
    );
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
