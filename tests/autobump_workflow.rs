//! Standing guards on `sdk-autobump.yml` — the workflow that opens the SDK bump
//! PR unattended.
//!
//! The bug these exist for: `gh pr merge --auto` was called *before* `gh pr
//! create`. On a brand-new branch there is no PR yet, so arming could never
//! succeed. It was invisible because `allow_auto_merge` is currently `false` on
//! this repo, and the probe short-circuits before the arming call — so the body
//! said "could not arm: the setting is off", which was true for the wrong reason.
//! The day that setting flips, arming would start failing while the PR body
//! claimed it was armed, which is the ENG-7688 shape: a pipeline that reads as
//! automated while a human is still the only thing that moves it.
//!
//! Ordering bugs in a scheduled workflow have no natural test surface — nothing
//! runs the daily job on a PR, and the failure only appears once a repo setting
//! changes months later. These are text-level assertions on the checked-in YAML
//! rather than a real run, which is the trade: they cannot prove the workflow
//! works, only that the two orderings it got wrong stay right. They need no
//! network and run in the existing `cargo test` job.

fn read(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

const WORKFLOW: &str = ".github/workflows/sdk-autobump.yml";
const RENDERER: &str = "scripts/render_autobump_pr_body.py";

/// Byte offsets of every occurrence of `needle`, ignoring comment lines — a
/// mention in a `#` comment must not satisfy (or break) an ordering assertion.
fn offsets_in_code(haystack: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut base = 0usize;
    for line in haystack.split_inclusive('\n') {
        if !line.trim_start().starts_with('#') {
            let mut from = 0usize;
            while let Some(i) = line[from..].find(needle) {
                out.push(base + from + i);
                from += i + needle.len();
            }
        }
        base += line.len();
    }
    out
}

/// Collect the string literal that follows each occurrence of `prefix`, e.g.
/// `auto_merge="armed"` with prefix `auto_merge="` yields `armed`. Literals that
/// begin with `$` are shell expansions, not states, and are skipped.
fn literals_after(haystack: &str, prefix: &str) -> Vec<String> {
    offsets_in_code(haystack, prefix)
        .into_iter()
        .filter_map(|i| {
            let rest = &haystack[i + prefix.len()..];
            let end = rest.find('"')?;
            let lit = &rest[..end];
            (!lit.starts_with('$') && !lit.is_empty()).then(|| lit.to_string())
        })
        .collect()
}

/// The whole point. `gh pr merge --auto` needs a PR to exist, so it must come
/// after `gh pr create`, not before it.
#[test]
fn auto_merge_is_armed_only_after_the_pr_is_created() {
    let wf = read(WORKFLOW);

    let create = *offsets_in_code(&wf, "gh pr create")
        .first()
        .expect("sdk-autobump.yml no longer calls `gh pr create`");
    let arm = offsets_in_code(&wf, "gh pr merge");
    assert_eq!(
        arm.len(),
        1,
        "expected exactly one `gh pr merge` call in {WORKFLOW}; found {}. If arming \
         moved or was duplicated, re-check that every path still creates the PR first.",
        arm.len()
    );

    assert!(
        create < arm[0],
        "`gh pr merge --auto` appears BEFORE `gh pr create` in {WORKFLOW}. Auto-merge \
         cannot be armed on a branch that has no PR yet, so this can only ever fail — \
         silently today, because `allow_auto_merge` is off and the probe returns first. \
         Create the PR, then arm it."
    );
}

/// Arming is a decision about an unattended merge, so it stays gated on both
/// signals that say a human should look first: a breaking spec delta, and a
/// bump that does not compile.
#[test]
fn arming_stays_gated_on_verdict_and_compilation() {
    let wf = read(WORKFLOW);

    assert!(
        wf.contains(r#"[ "$VERDICT" != "breaking" ] && [ "$COMPILES" = "clean" ]"#),
        "the auto-merge gate in {WORKFLOW} no longer requires both a non-breaking \
         verdict and a clean `cargo check`. Either condition alone is not enough to \
         let a dependency bump land without review."
    );

    // The gate must precede arming, or it gates nothing.
    let gate = wf
        .find(r#"[ "$VERDICT" != "breaking" ]"#)
        .expect("auto-merge gate not found");
    let arm = offsets_in_code(&wf, "gh pr merge")[0];
    assert!(
        gate < arm,
        "the auto-merge gate must come before the arming call"
    );
}

/// A PR body that claims "armed" when arming failed is the failure this whole
/// arrangement exists to prevent, so the workflow re-renders on failure — and
/// with a state distinct from `unavailable`, which asserts something specific
/// and different (that the repo setting is off).
#[test]
fn a_failed_arming_call_corrects_the_pr_body() {
    let wf = read(WORKFLOW);

    let arm = offsets_in_code(&wf, "gh pr merge")[0];
    let tail = &wf[arm..];
    assert!(
        tail.contains("arm-failed"),
        "nothing in {WORKFLOW} re-renders the PR body after `gh pr merge` fails, so a \
         failed arming leaves a body that says auto-merge is armed when it is not."
    );
    // The branch selector sits between the two, so match per line rather than on
    // one literal: `gh pr edit "$branch" --body-file ...`.
    let writes_body_back = offsets_in_code(tail, "gh pr edit").into_iter().any(|i| {
        tail[i..]
            .lines()
            .next()
            .is_some_and(|l| l.contains("--body-file"))
    });
    assert!(
        writes_body_back,
        "the corrected body is never written back — no `gh pr edit ... --body-file` \
         follows the arming call, so the re-render goes nowhere."
    );
}

/// Cross-file: the workflow passes these states to the renderer as `--auto-merge`,
/// and argparse `choices` rejects anything it does not know. A state added on one
/// side only would fail the whole step at the moment it is first needed — which is
/// the failure path, the worst place to discover it.
#[test]
fn every_state_the_workflow_reports_is_one_the_renderer_accepts() {
    let wf = read(WORKFLOW);
    let py = read(RENDERER);

    let choices = {
        let at = py
            .find(r#""--auto-merge""#)
            .expect("renderer no longer takes --auto-merge");
        let rest = &py[at..];
        let open = rest.find('[').expect("no choices list after --auto-merge");
        let close = rest.find(']').expect("unterminated choices list");
        rest[open + 1..close]
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };

    let mut used = literals_after(&wf, "auto_merge=\"");
    used.extend(literals_after(&wf, "render_body \""));
    assert!(
        !used.is_empty(),
        "found no auto-merge states in {WORKFLOW}; this guard has gone blind"
    );

    for state in &used {
        assert!(
            choices.contains(state),
            "{WORKFLOW} can report auto-merge state {state:?}, but {RENDERER} only \
             accepts {choices:?} — rendering that state would abort the step with an \
             argparse error instead of opening or correcting the PR."
        );
    }
}
