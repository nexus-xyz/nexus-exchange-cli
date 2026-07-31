#!/usr/bin/env python3
"""Render the SDK-autobump PR body markdown.

Kept out of the workflow's inline shell so the markdown (full of backticks and
`${...}` examples) isn't fighting shell quoting — and so the body is easy to
eyeball/diff. Driven by `.github/workflows/sdk-autobump.yml` (ENG-7962).

The body's job is to be honest about four things the bot cannot decide, each of
which has a way of silently reading as "fine" when it isn't:

  * **Does it compile?** A crate bump can move the SDK's Rust API, not just its
    paths — 0.7.0 added a `limit` parameter to `fetch_my_trades`. No spec-level
    check sees that, so the workflow runs `cargo check` and reports it here.
  * **Did PR checks run?** A PR opened with the default GITHUB_TOKEN does not
    trigger other workflows. A PR with no checks looks the same as one whose checks
    haven't finished, so say which it is.
  * **Was auto-merge armed?** `allow_auto_merge` is a repo setting and it is off
    here, so claiming "armed" when the call was refused would be the ENG-7688
    failure shape.
  * **Did the spec pin actually move?** A crate bump does not always move it
    (0.6.0 -> 0.6.1 both pin v0.7.1), and there is no delta to classify then.

Usage:
  render_autobump_pr_body.py --new-version X.Y.Z --old-tag vA.B.C --new-tag vD.E.F \
      --verdict {non-breaking|breaking|no-spec-change} --oasdiff-file PATH \
      --auto-merge {armed|unavailable|skipped} --compiles {clean|broken} \
      --pr-checks {will-run|will-not-run}
"""
import argparse
import sys


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--new-version", required=True, help="new nexus-exchange version")
    ap.add_argument("--old-tag", required=True, help="spec tag before the bump")
    ap.add_argument("--new-tag", required=True, help="spec tag the new crate pins")
    ap.add_argument(
        "--verdict",
        required=True,
        choices=["non-breaking", "breaking", "no-spec-change"],
    )
    ap.add_argument("--oasdiff-file", required=True)
    ap.add_argument(
        "--auto-merge", required=True, choices=["armed", "unavailable", "skipped"]
    )
    ap.add_argument("--compiles", required=True, choices=["clean", "broken"])
    ap.add_argument("--pr-checks", required=True, choices=["will-run", "will-not-run"])
    args = ap.parse_args()

    try:
        with open(args.oasdiff_file) as f:
            oasdiff_out = f.read().strip() or "(no output captured)"
    except OSError:
        oasdiff_out = "(no output captured)"

    out = []
    out.append(
        f"`nexus-exchange` **{args.new_version}** is published. Opened automatically "
        f"by `sdk-autobump` (ENG-7962).\n"
    )

    out.append("### What moved\n")
    out.append(f"- `Cargo.toml` / `Cargo.lock` → `nexus-exchange` **{args.new_version}**")
    if args.new_tag != args.old_tag:
        out.append(
            f"- `.api-version` → **{args.new_tag}** (was {args.old_tag}) — **copied "
            f"from the crate**, not chosen. The crate pins the spec tag and sends it "
            f"as `X-Nexus-Api-Version`, so this repo's pin is a derived value; "
            f"`check_sdk_parity.py` fails if the two disagree."
        )
    else:
        out.append(
            f"- `.api-version` unchanged at **{args.new_tag}** — this crate release "
            f"targets the same spec version."
        )
    out.append("- the bot-managed README block, to match.\n")
    out.append(
        "Nothing else. `endpoints.txt`, `METHOD_OP` and the coverage numbers are "
        "human-owned: if the new crate needs them changed, the checks below say so "
        "and a human pushes the edit onto this branch.\n"
    )

    # --- does it compile -----------------------------------------------------
    out.append("### Does it compile?\n")
    if args.compiles == "clean":
        out.append(
            "`cargo check --all-targets --all-features` passes against the new "
            "crate. Worth stating explicitly: a crate bump can change the SDK's Rust "
            "API and not just its paths, which no spec-level check would notice.\n"
        )
    else:
        out.append(
            "**No — `cargo check` FAILED against the new crate.** The SDK's Rust API "
            "changed in a way the current call sites don't satisfy (this is what "
            "happened at 0.7.0, which added a `limit` parameter to "
            "`fetch_my_trades`). See the workflow log for the exact error.\n"
        )
        out.append(
            "**This PR is incomplete as opened** and needs a code change pushed onto "
            "the branch before it can merge. That is expected and not a bug in the "
            "bot — the bot bumps the version; adapting to a changed API is a human "
            "job.\n"
        )

    # --- spec delta ----------------------------------------------------------
    if args.verdict == "no-spec-change":
        out.append("### Spec delta: none\n")
        out.append(
            f"Both {args.old_tag} and the new crate pin the same spec version, so "
            f"there is no delta to classify and oasdiff was not run. This is a pure "
            f"dependency bump.\n"
        )
    else:
        out.append(f"### Spec delta: **{args.verdict}**\n")
        out.append(
            f"Classified `{args.old_tag} -> {args.new_tag}` with "
            f"`oasdiff breaking --fail-on ERR` — the same gate the api repo runs as "
            f'"Classify API changes". ERR-level changes are breaking; WARN/INFO are '
            f"not.\n"
        )
        out.append("<details><summary>oasdiff breaking output</summary>\n")
        out.append(f"```\n{oasdiff_out}\n```\n")
        out.append("</details>\n")

    # --- verification --------------------------------------------------------
    out.append("### The merge signal\n")
    out.append(
        "Three checks have to be green, and they cover different things:\n"
    )
    out.append(
        "- **`drift`** — every operation the CLI targets still exists in the pinned "
        "spec, `endpoints.txt` still equals what the commands actually call, and "
        "neither allowlist holds a stale exemption."
    )
    out.append(
        "- **`sdk-parity`** — `.api-version` equals the crate's own pin, and every "
        "`endpoints.txt` line is an operation the crate actually wraps. This is the "
        "check that makes the pin a fact rather than a claim."
    )
    out.append("- **CI `fmt` / `clippy` / `test`** — the code still builds and passes.\n")

    if args.pr_checks == "will-not-run":
        out.append(
            "⚠️ **Those checks have NOT run on this PR.** It was opened with the "
            "default `GITHUB_TOKEN`, and GitHub does not trigger workflows from PRs "
            "created that way. An empty check list here means *not run*, not "
            "*passed*.\n"
        )
        out.append(
            "To get them: set a `SDK_DISPATCH_TOKEN` repo secret (any PAT/app token "
            "with repo scope) so future runs trigger checks, and for this PR push an "
            "empty commit or close/reopen it. The `cargo check` result above is the "
            "one signal that did run.\n"
        )

    # --- merge gating --------------------------------------------------------
    out.append("### Merge gating\n")
    if args.auto_merge == "armed":
        out.append(
            "GitHub auto-merge is **armed** (squash). It does not merge on its own — "
            "the PR still needs its required checks green and the **ENG-4149** "
            "ruleset bypass before anything happens.\n"
        )
    elif args.auto_merge == "unavailable":
        out.append(
            "Auto-merge could **NOT** be armed: this repository has "
            "`allow_auto_merge` **disabled**, so the request was refused. Said "
            "plainly rather than swallowed, because a step that silently no-ops here "
            "is how a pipeline comes to look automated while a human is still the "
            "only thing that moves it (ENG-7688).\n"
        )
        out.append(
            "**A human merges this PR.** Enabling `allow_auto_merge` and the "
            "ENG-4149 bypass are the two independent things that would change that; "
            "neither is in scope here.\n"
        )
    else:
        reason = (
            "the spec delta is breaking"
            if args.verdict == "breaking"
            else "`cargo check` failed"
        )
        out.append(
            f"Auto-merge was deliberately **not** attempted because {reason}. A human "
            f"owns this one: review what changed, make the code changes it implies, "
            f"then merge."
        )

    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
