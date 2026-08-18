#!/usr/bin/env python3
"""Check the CLI against the `nexus-exchange` crate it compiles against (ENG-7962).

`check_spec_drift.py` answers "does the CLI still match the spec it pins?". It
cannot answer "is that pin even the right one?", because it never looks at the SDK.
That gap is real and load-bearing: the CLI issues no HTTP of its own, so the crate
decides which spec version is actually spoken — it pins the tag and sends it as
`X-Nexus-Api-Version` on every request. A CLI pin ahead of (or behind) the crate's
is simply a false statement about runtime behaviour, and nothing was checking it.

Two invariants, both against the *published* crate (read from its crates.io
tarball, so it is the exact artifact cargo builds):

5. `.api-version` == the crate's `.api-version`
   The pin is a DERIVED value, not a choice. Advancing it without a crate release
   makes the README and the pin claim a spec version the binary never sends.

6. endpoints.txt is a SUBSET of the crate's endpoints.txt
   The CLI reaches operations only by calling named SDK methods, so the SDK's
   manifest is the ceiling. A CLI line outside it is unreachable — either a typo,
   or a METHOD_OP row describing an operation the SDK does not actually issue.

Invariant 6 is what makes the ENG-7962 bug structurally impossible rather than
caught by hand. `amend_order` was mapped to `PUT /orders/{order_id}`; the SDK's own
manifest lists `PATCH /orders/{order_id}` and no PUT, so this check fails that
mapping immediately — where the spec-only check could not, since the spec defines
both a PATCH and (elsewhere) PUT operations and neither file agreed with the other.

Subset, not equality, on purpose: the SDK wraps considerably more than the CLI
exposes (bridge deposits, ADL history, admin tiers). Those are coverage gaps to
report, not failures — the CLI is not required to surface everything the SDK can
do. The count is printed so the gap stays visible.

Needs network (one crates.io fetch, cached by version). Run:
  check_sdk_parity.py
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import check_spec_drift as csd  # noqa: E402
import sdk_crate  # noqa: E402


def check_pin(locked, crate_tag, our_tag):
    """Invariant 5. Returns the number of errors printed."""
    if our_tag == crate_tag:
        print(
            f"OK: .api-version ({our_tag}) matches the pin in "
            f"{sdk_crate.SDK_CRATE} {locked}."
        )
        return 0

    ours = sdk_crate.version_key(our_tag)
    theirs = sdk_crate.version_key(crate_tag)
    print(
        f"\nERROR: .api-version is {our_tag} but {sdk_crate.SDK_CRATE} {locked} "
        f"pins {crate_tag}."
    )
    if ours > theirs:
        print(
            f"  The pin is AHEAD of the SDK. The binary sends "
            f"`X-Nexus-Api-Version: {crate_tag}`, so claiming {our_tag} is wrong. "
            f"A spec release does not entitle the CLI to advance its pin — wait for "
            f"a {sdk_crate.SDK_CRATE} release that pins {our_tag}, bump the "
            f"dependency, and let the pin follow."
        )
    else:
        print(
            f"  The pin is BEHIND the SDK. Set .api-version to {crate_tag} (and the "
            f"README managed line with it): `python3 scripts/sync_sdk_version.py "
            f"--write`."
        )
    return 1


def check_manifest_subset(locked, sdk_ops, our_ops):
    """Invariant 6. Returns the number of errors printed."""
    sdk_norm = {(m, csd.normalize_path(p)) for m, p in sdk_ops}
    sdk_paths = {}
    for m, p in sdk_norm:
        sdk_paths.setdefault(p, set()).add(m)

    unreachable = sorted(
        (m, p) for m, p in our_ops if (m, csd.normalize_path(p)) not in sdk_norm
    )
    # The WebSocket upgrade is reached by the streaming client, not a REST wrapper,
    # so it is legitimately absent from the SDK's REST manifest.
    unreachable = [
        op for op in unreachable if (op[0], csd.normalize_path(op[1])) not in csd.NON_REST_TARGETS
    ]

    if not unreachable:
        # Count the exemptions actually present rather than the size of the
        # allowlist. They coincide today only because invariant 3 rejects an unused
        # NON_REST_TARGETS entry; deriving the number from the manifest keeps this
        # line correct on its own, and keeps it from claiming the WebSocket upgrade
        # is "wrapped by the crate" when the whole point is that it isn't.
        exempt = sum(
            1
            for m, p in our_ops
            if (m, csd.normalize_path(p)) in csd.NON_REST_TARGETS
        )
        covered = len(our_ops) - exempt
        note = (
            f" ({exempt} non-REST target(s) exempt — reached by the streaming "
            f"client, not a REST wrapper)"
            if exempt
            else ""
        )
        print(
            f"OK: all {covered} REST endpoints.txt operation(s) are wrapped by "
            f"{sdk_crate.SDK_CRATE} {locked} (which wraps {len(sdk_norm)} in "
            f"total){note}."
        )
        gap = len(sdk_norm) - covered
        if gap > 0:
            print(
                f"    Informational: {gap} operation(s) the SDK wraps have no CLI "
                f"command. Not a failure — the CLI is not required to expose "
                f"everything the SDK can reach."
            )
        return 0

    print(
        f"\nERROR: {len(unreachable)} endpoints.txt operation(s) are NOT wrapped by "
        f"{sdk_crate.SDK_CRATE} {locked}, so no CLI command can reach them:"
    )
    for m, p in unreachable:
        norm = csd.normalize_path(p)
        if norm in sdk_paths:
            print(
                f"  - {m} {p}: the SDK issues "
                f"{'/'.join(sorted(sdk_paths[norm]))} on this path, not {m}. Fix the "
                f"METHOD_OP verb and this line to match the SDK."
            )
        else:
            print(
                f"  - {m} {p}: the SDK does not wrap this path at all. Remove the "
                f"line, or land the wrapper in nexus-exchange-rs first."
            )
    return len(unreachable)


# The bot-managed README block, as `sync_sdk_version.py` renders it. Matched, not
# regenerated, so this stays a check rather than a second writer.
README_PIN_RE = re.compile(
    r"Currently targets Exchange API spec \*\*`(?P<tag>v[\d.]+)`\*\* — the version "
    r"pinned and sent as `X-Nexus-Api-Version` by `nexus-exchange` "
    r"\*\*`(?P<crate>[\d.]+)`\*\*\."
)


def check_readme_pin(locked, our_tag, readme=None):
    """Invariant 6: the README's managed line names the resolved crate and our pin.

    AGENTS.md says "CI fails if the pin, the README line, and the crate disagree."
    Two thirds of that was true: `check_pin` above compares `.api-version` to the
    crate's own pin, but nothing compared the README's *crate version* to
    `Cargo.lock`. So a hand-bump that skipped `sync_sdk_version.py --write` went
    green with the README a release behind — which is what happened on the
    0.9.0 -> 0.9.1 bump (ENG-10956), left `main` claiming 0.9.0 while shipping
    0.9.1, and was only corrected because a stale autobump PR happened to carry the
    line. This closes it.

    The fix is never a hand-edit of the block: run
    `python3 scripts/sync_sdk_version.py --write`.
    """
    readme = readme or os.path.join(csd.REPO, "README.md")
    try:
        with open(readme) as f:
            text = f.read()
    except OSError as e:
        print(f"\nERROR: cannot read {readme}: {e}")
        return 1

    m = README_PIN_RE.search(text)
    if not m:
        print(
            f"\nERROR: {readme} has no parseable api-version-sync line. It is "
            f"generated by sync_sdk_version.py; run it with --write, or update "
            f"check_sdk_parity.py:README_PIN_RE if the template changed."
        )
        return 1

    if (m.group("tag"), m.group("crate")) != (our_tag, locked):
        print(
            f"\nERROR: {readme}'s managed line disagrees with the tree.\n"
            f"  README: spec {m.group('tag')}, {sdk_crate.SDK_CRATE} "
            f"{m.group('crate')}\n"
            f"  tree:   spec {our_tag}, {sdk_crate.SDK_CRATE} {locked}\n"
            f"  Fix with: python3 scripts/sync_sdk_version.py --write"
        )
        return 1

    print(
        f"OK: README's managed line matches the tree (spec {our_tag}, "
        f"{sdk_crate.SDK_CRATE} {locked})."
    )
    return 0


def main():
    locked = sdk_crate.locked_version()
    required = sdk_crate.required_version()
    crate_tag = sdk_crate.crate_api_version(locked)
    # Absolute paths throughout: this runs from a workflow step and from a
    # contributor's shell, and load_targeted's default is CWD-relative.
    with open(os.path.join(csd.REPO, ".api-version")) as f:
        our_tag = f.read().strip()
    our_ops = csd.load_targeted(os.path.join(csd.REPO, "endpoints.txt"))

    print(
        f"{sdk_crate.SDK_CRATE}: Cargo.toml requires {required}, "
        f"Cargo.lock resolves {locked}"
    )
    print(
        f"{sdk_crate.SDK_CRATE} {locked} pins spec {crate_tag}; "
        f"this repo pins {our_tag}"
    )

    failures = check_pin(locked, crate_tag, our_tag)
    # Invariant 6: the README's managed line agrees with Cargo.lock and the pin.
    failures += check_readme_pin(locked, our_tag)
    failures += check_manifest_subset(
        locked, sdk_crate.crate_endpoints(locked), our_ops
    )

    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
