#!/usr/bin/env python3
"""Render the body of a catalog-autobump PR (ENG-17337).

Says what the new pin changes for someone running `nexus examples` offline:
which examples appear, disappear, or change, between the old and new built-in
catalog. Stdlib only, so the workflow needs no setup.

    python3 scripts/render_catalog_pin_body.py --old-ref untagged \\
        --new-ref catalog-2026.09.24 --old old.json --new src/examples_catalog.json \\
        --pr-checks will-run
"""

from __future__ import annotations

import argparse
import json
import sys

REPO = "https://github.com/nexus-xyz/nexus-exchange-examples"


def load(path: str) -> dict[str, dict]:
    with open(path) as f:
        catalog = json.load(f)
    return {e["path"]: e for e in catalog.get("examples", [])}


def render(old_ref: str, new_ref: str, old: dict[str, dict], new: dict[str, dict], checks: str) -> str:
    added = sorted(new.keys() - old.keys())
    removed = sorted(old.keys() - new.keys())
    changed = sorted(p for p in new.keys() & old.keys() if new[p] != old[p])

    lines = [
        "## What",
        f"Pins the examples catalog built into `nexus` to [`{new_ref}`]({REPO}/tree/{new_ref}) "
        f"(was `{old_ref}`). `nexus examples list/show` fall back to this copy when the "
        "examples repository can't be reached and nothing is cached.",
        "",
        "## Why",
        "The examples repo tags every commit on `main` that passes CI; this keeps the "
        "offline copy on the newest one so a release never ships a stale catalog.",
        "",
        "Part of ENG-17337",
        "",
    ]
    if not (added or removed or changed):
        lines.append("No example was added, removed or changed; only the pin moved.")
    else:
        if added:
            lines.append("**Added:** " + ", ".join(f"`{p}`" for p in added))
        if removed:
            lines.append("**Removed:** " + ", ".join(f"`{p}`" for p in removed))
        if changed:
            lines.append("**Changed entries:** " + ", ".join(f"`{p}`" for p in changed))
    lines.append("")
    if checks == "will-run":
        lines.append("PR checks run on this PR.")
    else:
        lines.append(
            "**PR checks did not run**: this was opened with the default `GITHUB_TOKEN` "
            "(`SDK_DISPATCH_TOKEN` is unset). The sync script validated the catalog "
            "before writing it; push an empty commit or re-run CI before merging."
        )
    lines += ["", "🤖 Opened by `catalog-autobump.yml`"]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--old-ref", required=True)
    p.add_argument("--new-ref", required=True)
    p.add_argument("--old", required=True)
    p.add_argument("--new", required=True)
    p.add_argument("--pr-checks", choices=["will-run", "will-not-run"], required=True)
    a = p.parse_args(argv)
    sys.stdout.write(render(a.old_ref, a.new_ref, load(a.old), load(a.new), a.pr_checks))
    return 0


if __name__ == "__main__":
    sys.exit(main())
