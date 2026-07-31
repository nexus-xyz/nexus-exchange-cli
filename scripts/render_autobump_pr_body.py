#!/usr/bin/env python3
"""Render the spec-autobump PR body markdown.

Kept out of the workflow's inline shell so the markdown (full of backticks and
`${...}` examples) isn't fighting shell quoting — and so the body is easy to
eyeball/diff. Driven by `.github/workflows/spec-autobump.yml` (ENG-7962; ported
from nexus-exchange-rs, the reference implementation for ENG-3563).

Reads the captured oasdiff breaking output from a file so the verbatim verdict
lands in the PR. Writes the rendered markdown to stdout.

Two things this body has to say that the SDK's version does not:

  * **Auto-merge may not be armable.** `allow_auto_merge` is a repo setting and it
    is OFF on this repo, so the arming step reports honestly instead of pretending
    (`--auto-merge armed|unavailable|skipped`). A body that claimed "auto-merge is
    armed" when the API call had failed would be the ENG-7688 failure shape.
  * **The CLI is downstream of `nexus-exchange-rs`.** The CLI issues no HTTP of its
    own, so an op it targets that the new spec renames or removes cannot be fixed
    here until the crate ships a regenerated wrapper. Such a PR legitimately sits
    RED on drift while it waits — that is the check working, not a broken check.

Usage:
  render_autobump_pr_body.py --new-tag vX.Y.Z --old-tag vA.B.C \
      --verdict {non-breaking|breaking} --oasdiff-file PATH \
      --auto-merge {armed|unavailable|skipped}
"""
import argparse
import sys

AUTO_MERGE_CHOICES = ("armed", "unavailable", "skipped")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--new-tag", required=True)
    ap.add_argument("--old-tag", required=True)
    ap.add_argument("--verdict", required=True, choices=["non-breaking", "breaking"])
    ap.add_argument("--oasdiff-file", required=True)
    ap.add_argument(
        "--auto-merge",
        required=True,
        choices=AUTO_MERGE_CHOICES,
        help=(
            "armed: auto-merge was requested successfully. unavailable: the repo "
            "has allow_auto_merge off (or the call failed). skipped: breaking, so "
            "it was deliberately not attempted."
        ),
    )
    args = ap.parse_args()

    try:
        with open(args.oasdiff_file) as f:
            oasdiff_out = f.read().strip() or "(no output captured)"
    except OSError:
        oasdiff_out = "(no output captured)"

    out = []
    out.append(
        f"nexus-exchange-api released **{args.new_tag}** "
        f"(was pinned at **{args.old_tag}**). Opened automatically by "
        f"`spec-autobump` (ENG-7962).\n"
    )
    out.append(f"### oasdiff verdict: **{args.verdict}**\n")
    out.append(
        f"Classified `{args.old_tag} -> {args.new_tag}` with "
        f"`oasdiff breaking --fail-on ERR` (the same gate the api repo runs as "
        f'"Classify API changes", and the same one nexus-exchange-rs uses). '
        f"ERR-level changes are breaking; WARN/INFO are not.\n"
    )
    out.append("<details><summary>oasdiff breaking output</summary>\n")
    out.append(f"```\n{oasdiff_out}\n```\n")
    out.append("</details>\n")

    out.append("### Applied\n")
    out.append(f"- Bumped `.api-version` to `{args.new_tag}`.")
    out.append('- Updated the bot-managed "currently targets" line in the README.')
    out.append(
        "- Nothing else. `endpoints.txt`, `METHOD_OP`, the coverage numbers and the "
        "`nexus-exchange` dependency are all human-owned: if the new spec needs any "
        "of them changed, `spec-drift` says so below and a human pushes the edit "
        "onto this branch.\n"
    )

    out.append("### The merge signal is `spec-drift`\n")
    out.append(
        "Green drift means the pin advance needs no code change: every operation "
        "the CLI targets still exists in `" + args.new_tag + "`, `endpoints.txt` "
        "still equals what the commands actually call, and neither allowlist is "
        "holding a stale exemption. An additive spec release stays green.\n"
    )
    out.append(
        "Red drift means the new spec removed or renamed an operation the CLI "
        "targets.\n"
    )

    out.append("#### If drift is red: this repo is downstream of `nexus-exchange-rs`\n")
    out.append(
        "The CLI is a thin layer over the published `nexus-exchange` crate and "
        "issues no path of its own, so it cannot reach a renamed or newly-added "
        "operation until that crate ships a wrapper for it. **A red drift check on "
        "this PR can therefore be legitimate and unfixable here**: the fix is a "
        "`nexus-exchange-rs` release, then a dependency bump in `Cargo.toml`, then "
        "the `endpoints.txt` / `METHOD_OP` edits on this branch.\n"
    )
    out.append(
        "Do not \"fix\" it by deleting the failing `endpoints.txt` lines — that "
        "makes the manifest stop describing what the commands do, which is the bug "
        "this whole check exists to prevent (ENG-7958). Leave the PR red and "
        "blocked on the crate release.\n"
    )

    if args.verdict == "non-breaking":
        out.append("### Merge gating (non-breaking)\n")
        if args.auto_merge == "armed":
            out.append(
                "GitHub auto-merge has been **armed** (squash). It does NOT merge on "
                "its own — the PR can only merge once:\n"
            )
            out.append(
                "- the required status checks pass — `drift` (spec-drift) and the CI "
                "`fmt` / `clippy` / `test` jobs, and"
            )
            out.append(
                "- the **ENG-4149** ruleset bypass for this bot is configured to "
                "satisfy the review requirement for pin-bump PRs only.\n"
            )
            out.append(
                "Until ENG-4149 lands, this PR sits green awaiting the bypass — "
                "auto-merge cannot fire. No premature merge."
            )
        elif args.auto_merge == "unavailable":
            out.append(
                "Auto-merge could **NOT** be armed: this repository has "
                "`allow_auto_merge` **disabled**, so the request was refused. Said "
                "plainly rather than swallowed, because a step that silently no-ops "
                "here is how a pipeline comes to look automated while a human is "
                "still the only thing that moves it (ENG-7688).\n"
            )
            out.append(
                "**A human must merge this PR**, once `drift` and CI are green. Two "
                "independent things gate real auto-landing: enabling "
                "`allow_auto_merge` on this repo, and the **ENG-4149** ruleset "
                "bypass. Neither is in scope for this PR."
            )
        else:  # skipped — shouldn't pair with non-breaking, but say so honestly
            out.append(
                "Auto-merge was not attempted. A human must merge this PR once "
                "`drift` and CI are green."
            )
    else:
        out.append("### Merge gating (breaking)\n")
        out.append(
            f"oasdiff flagged an ERR-level (breaking) change, so auto-merge was "
            f"**NOT** armed — deliberately, whatever the repo setting says. A human "
            f"owns this: review what `{args.new_tag}` changes, work out whether the "
            f"CLI's commands are affected (and whether a `nexus-exchange` release is "
            f"needed first — see above), then merge. Labeled "
            f"`breaking · needs-SDK-update`."
        )

    sys.stdout.write("\n".join(out) + "\n")


if __name__ == "__main__":
    main()
