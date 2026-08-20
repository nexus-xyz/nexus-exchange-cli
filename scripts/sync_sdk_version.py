#!/usr/bin/env python3
"""Advance the CLI to a newer published `nexus-exchange` crate, and let the spec pin
follow (ENG-7962).

Replaces the earlier `sync_api_version.py`, which followed *spec* releases. That was
the wrong upstream for this repo. The CLI is the only surface in the fleet that
wraps another SDK rather than implementing the spec: py, ts and mcp generate against
the spec directly, but the CLI issues no HTTP of its own and reaches the API solely
through `nexus_exchange::Client`, which pins the spec tag and sends it as
`X-Nexus-Api-Version`. So:

  * a spec release alone gives the CLI nothing it can act on — the operations are
    not reachable until the crate wraps them, and advancing the pin without a crate
    release makes the README claim a version the binary never sends;
  * a crate release is the actionable event, and it carries the correct pin with it.

Hence the chain the CLI follows is `api -> rs -> cli`, not `api -> cli`. The pin is
DERIVED from the crate, never chosen here. `check_sdk_parity.py` enforces that in
CI (invariant 5).

Modes:
  --check   Report whether a newer crate has been published. Exit 0 if current,
            3 if behind, 1 on error. No files touched.
  --write   If behind, rewrite the Cargo.toml requirement, `.api-version` (copied
            from the new crate) and the README managed line. Idempotent. Does NOT
            touch Cargo.lock — the caller runs `cargo update -p nexus-exchange`
            afterwards, because only cargo can resolve the lockfile correctly.
  --repair  Re-derive the two bot-owned values — `.api-version` and the README
            managed line — from the crate `Cargo.lock` ALREADY resolves. No
            dependency bump, no lockfile change, no crates.io version lookup.

`--repair` exists because `--write` is a bump, not a repair, and the two were being
confused by the very checks that recommend it. `--write` returns at "Dependency is
up to date" before it ever reaches the README, so in the one scenario invariant 8
was built for — a hand-bump that moved Cargo.toml/Cargo.lock and left the README a
release behind, which is what #60 did on 0.9.0 -> 0.9.1 — it prints "up to date"
and changes nothing, leaving a red check with no working remedy while AGENTS.md
forbids hand-editing the block. And if the crate happens to be behind, `--write`
performs an unrelated dependency bump instead of the repair that was asked for.
Same hole on the invariant-5 side: `.api-version` drifting from the resolved
crate's pin (a bare `cargo update`, or a pin advanced by hand) is not a "newer crate
published" event either. `--repair` fixes exactly the derived values, from the tree
as it stands.

Tokenless: crates.io needs only a User-Agent. `--latest` overrides the lookup for
tests and offline runs.

Usage:
  sync_sdk_version.py --check [--latest X.Y.Z]
  sync_sdk_version.py --write [--latest X.Y.Z]
  sync_sdk_version.py --repair
"""
import argparse
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import sdk_crate  # noqa: E402
from sdk_crate import SDK_CRATE, fail, version_key  # noqa: E402

REPO = sdk_crate.REPO
API_VERSION_FILE = os.path.join(REPO, ".api-version")
README = os.path.join(REPO, "README.md")

# The README line the bot owns. Everything between the markers is regenerated, so
# the surrounding prose (the /api/v1 migration notes, the coverage discussion)
# stays human-owned.
MARK_START = "<!-- api-version-sync:start -->"
MARK_END = "<!-- api-version-sync:end -->"
MANAGED_BLOCK_RE = re.compile(
    re.escape(MARK_START) + r".*?" + re.escape(MARK_END), re.DOTALL
)


def render_managed_block(tag, crate_version):
    return (
        f"{MARK_START}\n\n"
        f"Currently targets Exchange API spec **`{tag}`** — the version pinned and "
        f"sent as `X-Nexus-Api-Version` by `nexus-exchange` "
        f"**`{crate_version}`**.\n\n"
        f"{MARK_END}"
    )


def update_readme(tag, crate_version):
    """Rewrite the managed block. Returns True if README changed. Fails loudly if
    the markers are missing — their absence is a setup error, not a reason to bump
    silently."""
    try:
        with open(README) as f:
            text = f.read()
    except OSError as e:
        fail(f"cannot read {README}: {e}")
    if MARK_START not in text or MARK_END not in text:
        fail(
            f"{README} is missing the {MARK_START} / {MARK_END} markers; add the "
            f"managed block under '### API coverage' so the bot has a line to own."
        )
    new_text = MANAGED_BLOCK_RE.sub(
        lambda _: render_managed_block(tag, crate_version), text, count=1
    )
    if new_text == text:
        return False
    with open(README, "w") as f:
        f.write(new_text)
    return True


def update_cargo_toml(old_version, new_version):
    """Rewrite the requirement string in place. Deliberately a targeted text edit,
    not a TOML round-trip: Cargo.toml carries load-bearing comments (the whole
    binstall/dist block) that a serializer would discard."""
    try:
        with open(sdk_crate.CARGO_TOML) as f:
            text = f.read()
    except OSError as e:
        fail(f"cannot read {sdk_crate.CARGO_TOML}: {e}")
    # Anchored at line start so a mention inside a comment or another key cannot
    # match. `required_version()` has already proven this is a plain requirement.
    pattern = re.compile(
        rf'^(?P<lhs>{re.escape(SDK_CRATE)}\s*=\s*)"{re.escape(old_version)}"',
        re.MULTILINE,
    )
    new_text, n = pattern.subn(rf'\g<lhs>"{new_version}"', text, count=1)
    if n != 1:
        fail(
            f"could not find a line `{SDK_CRATE} = \"{old_version}\"` in "
            f"{sdk_crate.CARGO_TOML} to rewrite (found {n} matches)"
        )
    with open(sdk_crate.CARGO_TOML, "w") as f:
        f.write(new_text)


def repair(locked):
    """Bring `.api-version` and the README managed line back in line with the crate
    `Cargo.lock` resolves. Idempotent; prints what it changed.

    The pin is DERIVED from the crate (see the module docstring), so the crate is
    the only input: reading it back out of the published `.crate` tarball is the
    same source `check_sdk_parity.py` compares against, which is what makes this a
    repair rather than a second opinion. Cargo.toml and Cargo.lock are deliberately
    untouched — whatever crate the tree resolves today is taken as the intent, and
    changing it is `--write`'s job, not this one's.
    """
    crate_tag = sdk_crate.crate_api_version(locked)
    try:
        with open(API_VERSION_FILE) as f:
            old_tag = f.read().strip()
    except OSError as e:
        fail(f"cannot read {API_VERSION_FILE}: {e}")

    print(f"Cargo.lock resolves {SDK_CRATE} {locked}, which pins spec {crate_tag}")
    if old_tag != crate_tag:
        with open(API_VERSION_FILE, "w") as f:
            f.write(crate_tag + "\n")
        print(f"Wrote .api-version = {crate_tag} (was {old_tag})")
    else:
        print(f".api-version already {crate_tag}")

    if update_readme(crate_tag, locked):
        print(f"Rewrote the README managed line (spec {crate_tag}, {locked})")
    else:
        print("README managed line already agrees with the tree")

    if old_tag != crate_tag:
        # The pin gates which operations endpoints.txt may name, so moving it can
        # move invariant 1. Say so rather than leaving a green repair to imply the
        # tree is now consistent everywhere.
        print(
            "NOTE: the pin moved, so re-run check_spec_drift.py — and "
            "`--sync-coverage` if the README's coverage sentence still names the old "
            "tag."
        )


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true", help="report only; no writes")
    mode.add_argument("--write", action="store_true", help="apply the bump if behind")
    mode.add_argument(
        "--repair",
        action="store_true",
        help="re-derive .api-version and the README managed line from the locked "
        "crate (no dependency bump)",
    )
    ap.add_argument(
        "--latest",
        metavar="X.Y.Z",
        help="override the latest crate version (default: query crates.io)",
    )
    args = ap.parse_args()

    required = sdk_crate.required_version()
    locked = sdk_crate.locked_version()

    if args.repair:
        repair(locked)
        return

    latest = args.latest or sdk_crate.latest_published()
    if args.latest and not sdk_crate.CRATE_VERSION_RE.match(latest):
        fail(f"--latest is not a plain X.Y.Z crate version: {latest!r}")

    our_tag = sdk_crate.crate_api_version(locked)
    print(f"Cargo.toml requires {SDK_CRATE} {required}; Cargo.lock resolves {locked}")
    print(f"latest published {SDK_CRATE}: {latest}")
    print(f"spec pinned by the resolved crate: {our_tag}")

    if version_key(latest) < version_key(locked):
        # e.g. the newest release was yanked after we took it. Not an error.
        print(f"Resolved crate {locked} is AHEAD of the latest published {latest}; "
              f"nothing to sync.")
        return

    if version_key(latest) == version_key(locked):
        print(f"Dependency is up to date ({locked}).")
        return

    new_tag = sdk_crate.crate_api_version(latest)
    print(f"Dependency is BEHIND: {locked} -> {latest}")
    print(f"spec pin moves {our_tag} -> {new_tag}" if new_tag != our_tag
          else f"spec pin unchanged at {new_tag} (crate bump only)")

    if args.check:
        # Distinct exit code so a workflow can branch on "behind" vs error (1).
        sys.exit(3)

    update_cargo_toml(required, latest)
    with open(API_VERSION_FILE, "w") as f:
        f.write(new_tag + "\n")
    readme_changed = update_readme(new_tag, latest)
    print(f"Wrote Cargo.toml requirement = {latest}")
    print(f"Wrote .api-version = {new_tag}")
    print(f"README managed line updated: {readme_changed}")
    print(
        f"NOTE: Cargo.lock is untouched — run `cargo update -p {SDK_CRATE}` to "
        f"resolve it, or check_sdk_parity.py will fail on the stale lock."
    )


if __name__ == "__main__":
    main()
