//! Standing guards on how `sdk-autobump.yml` reports a bump that does not build.
//!
//! The workflow answers "does the new crate even compile?" itself, because a PR
//! opened with the default `GITHUB_TOKEN` cannot trigger `spec-drift` or CI, so
//! that `cargo check` is often the *only* signal an autobump PR ever gets. When it
//! failed, the answer went into the PR body and nowhere else: the run concluded
//! `success` and the PR looked like any other. The 0.8.0 bump (#53) therefore sat
//! for a day reading as "unreviewed" rather than "unbuildable", and was diagnosed
//! only when someone hit the same compile error by hand (ENG-9373).
//!
//! Both halves of the fix are single lines in a 300-line YAML file that no test
//! otherwise touches, and neither is visible until the next real bump — which is
//! the same shape as the release-config bugs `release_config.rs` guards. So they
//! are pinned here, in the existing `cargo test` job, with no network needed.
//!
//! These assert *reporting policy*, not the mechanics of the bump: the poll, the
//! oasdiff classification and the auto-merge arming are all free to change.

fn workflow() -> String {
    let path = format!(
        "{}/.github/workflows/sdk-autobump.yml",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The run's conclusion must match the answer the workflow computed. A green run
/// on a broken bump silences every alert wired to the workflow conclusion, which
/// is what made #53 invisible.
#[test]
fn a_broken_bump_fails_the_run() {
    let yaml = workflow();
    let guard = yaml
        .split_once("- name: Fail the run if the bump does not compile")
        .map(|(_, rest)| rest)
        .expect(
            "sdk-autobump.yml must keep a step that fails the run when the bump does not \
             compile — a green run on an unbuildable bump is ENG-9373",
        );
    assert!(
        guard.contains("steps.compile.outputs.result != 'clean'"),
        "the failing step must be conditioned on the compile result: {guard}"
    );
    assert!(
        guard.contains("exit 1"),
        "the step must actually fail the job, not just warn: {guard}"
    );
}

/// It has to fail *after* the PR exists. Failing inside the check step would leave
/// the pushed branch with no PR, and the next poll finds that branch, warns and
/// skips — so the bump would silently never get one. Ordering is load-bearing, not
/// cosmetic.
#[test]
fn the_run_fails_only_after_the_pr_is_opened() {
    let yaml = workflow();
    let open_pr = yaml
        .find("- name: Open the bump PR")
        .expect("sdk-autobump.yml must still open a PR");
    let fail = yaml
        .find("- name: Fail the run if the bump does not compile")
        .expect("sdk-autobump.yml must still fail the run on a broken bump");
    assert!(
        fail > open_pr,
        "the failing step must come AFTER the PR is opened, or a broken bump ends up \
         with a pushed branch and no PR (ENG-9373)"
    );
}

/// Draft is the only one of the three signals that shows up in the PR *list*,
/// which is where these get triaged. Asserted together with the reason it is
/// conditional: a breaking spec delta still compiles and is a real PR to review, so
/// it must not be drafted.
#[test]
fn a_broken_bump_is_opened_as_a_draft() {
    let yaml = workflow();
    assert!(
        yaml.contains("draft=(--draft)"),
        "sdk-autobump.yml must open a non-compiling bump as a draft (ENG-9373)"
    );
    let (_, after) = yaml
        .split_once("draft=()")
        .expect("the draft flag should default to empty");
    let decision = after
        .split_once("gh pr create")
        .map(|(head, _)| head)
        .expect("the draft decision must precede `gh pr create`");
    assert!(
        decision.contains("$COMPILES") && decision.contains("clean"),
        "drafting must be conditioned on the compile result, not on the spec verdict: \
         {decision}"
    );
    // Both `gh pr create` invocations (the second is the retry that drops a missing
    // label) have to carry it, or the retry path quietly opens a ready-for-review PR.
    assert_eq!(
        yaml.matches("gh pr create --base main --head \"$branch\" \"${draft[@]}\"")
            .count(),
        2,
        "both `gh pr create` calls — including the no-label retry — must pass the draft flag"
    );
}

/// The body must state the draft-and-fail behaviour, so the PR explains its own
/// state rather than leaving a reader to infer it from the absence of checks.
#[test]
fn the_pr_body_explains_the_draft_and_the_failure() {
    let path = format!(
        "{}/scripts/render_autobump_pr_body.py",
        env!("CARGO_MANIFEST_DIR")
    );
    let script = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let broken = script
        .split_once("**No — `cargo check` FAILED")
        .map(|(_, rest)| rest)
        .expect("the renderer must still have a broken-build branch");
    let broken = broken
        .split_once("--- spec delta")
        .map(|(head, _)| head)
        .unwrap_or(broken);
    for needed in ["draft", "fails"] {
        assert!(
            broken.contains(needed),
            "the broken-build body should say it is opened as a draft and that the run \
             fails; missing {needed:?}"
        );
    }
}
