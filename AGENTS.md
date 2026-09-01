# Contributing guide — nexus-exchange-cli

The command-line client for the Nexus Exchange API, built on `nexus-exchange-rs`.

## Merging

- Don't merge a PR without an approving review — CI passing isn't a substitute.
- Don't merge a PR you didn't author without an approving review **and** the
  author's sign-off. Check the author first
  (`gh pr view <n> --json author,reviewDecision`).
- Re-approval isn't needed for follow-up commits to an already-approved PR.
- Squash is the only merge method, and the branch is deleted on merge — one PR is
  always one commit on `main`.
- **The squash commit's subject is the PR title**, so release-please reads the
  title, not your commit messages. A title it can't parse contributes nothing to
  the version bump and lands under "Other" in the changelog.
- Declare a breaking change with `!` before the colon (`feat!:`) **and** a
  `BREAKING CHANGE:` footer saying what breaks and how to migrate. The `!` alone
  only bumps the version: with no footer, release-please falls back to the
  subject line, so the changelog's breaking section repeats the title and tells
  the reader nothing. `feat(cli)!: delete the phantom code-only ops` shipped that
  way in 0.5.0 and withdrew nine commands without naming one of them.
- The footer works only in a **commit** body — the squash body comes from the
  branch's commit messages, never the PR description, so a footer written only in
  the description is dropped at merge.

## Pull requests

- One concern per PR; link its tracking issue (`ENG-XXXX`) in the title.
- Write the title as a [conventional commit](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `deps:`, `docs:`, `chore:`, `ci:`) — it becomes the commit
  subject on `main`, so it is what drives the release. See [Merging](#merging).
- Dependency bumps use `deps:`, **not** `chore(deps):`. Release-please keys its
  changelog sections on the type and hides `chore`, so a `chore(deps):` bump is
  invisible in the release notes — which is how every `nexus-exchange` bump
  through 0.4.0 went unrecorded. `sdk-autobump` emits `deps:` for this reason.
- Respond to review comments before merging.

## Checks (before pushing)

- `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` all pass — CI
  enforces these.
- `python3 scripts/test_check_spec_drift.py` and
  `python3 scripts/test_sdk_parity.py` pass (no network). CI runs both on every PR,
  ahead of the checks they cover.
- If you touched `endpoints.txt`, `METHOD_OP`, either allowlist, the
  `nexus-exchange` dependency, or a file that calls the SDK:
  ```sh
  curl -fsSL https://raw.githubusercontent.com/nexus-xyz/nexus-exchange-api/$(cat .api-version)/openapi.json -o openapi.pinned.json
  python3 scripts/check_spec_drift.py openapi.pinned.json   # vs the spec
  python3 scripts/check_sdk_parity.py                       # vs the SDK crate
  ```

## Notes

- Capabilities are inherited from the `nexus-exchange-rs` dependency — bump it to
  pick up new endpoints rather than reimplementing.
- The spec pin (`.api-version`) is **derived from the `nexus-exchange` crate**, not
  chosen here — the crate is what sends `X-Nexus-Api-Version`. Don't hand-edit it or
  the marked README block. CI fails if the pin, the README line, and the crate
  disagree (invariants 5 and 8), and there are two different repairs:
  - `scripts/sync_sdk_version.py --write` — a **bump**: take a newly published
    crate, and let the pin and the README line follow.
  - `scripts/sync_sdk_version.py --repair` — a **repair**: re-derive the pin and the
    README line from the crate `Cargo.lock` already resolves. This is the one to
    reach for when a check is red, including after a hand-bump or a bare
    `cargo update`. `--write` is a no-op there — it returns at "Dependency is up to
    date" before it touches the README.
- Moving the pin moves the coverage ratio, and the README commits a copy of it
  (invariant 7). `scripts/check_spec_drift.py --sync-coverage <openapi.json>`
  rewrites just that sentence's numbers and tag. `sdk-autobump.yml` runs it after
  every bump, so a bot PR stays mergeable; do the same by hand if you move the pin
  yourself. **Fetch the spec at the new tag first.** The ratio comes from the spec
  file you pass and the tag comes from `.api-version`, so a stale `openapi.pinned.json`
  from the previous pin describes the wrong release; the checker refuses that pair
  rather than writing it, and the fix is the fetch, not the sentence.
- Follow the crate, not the spec. A spec release is not actionable here until
  `nexus-exchange-rs` ships wrappers and publishes; that is why this repo has
  `sdk-autobump.yml` rather than the fleet's `spec-autobump.yml` and is not a target
  of the api repo's dispatch fan-out.
- A crate bump can change the SDK's Rust API, not just its paths — no spec-level
  check sees that, so always `cargo check` after one.
- A CLI command can only reach what the SDK wraps, so `endpoints.txt` is capped by
  `nexus-exchange`. The checks catch a manifest that *mismatches* the SDK; they
  cannot see a wrapper the CLI never grew a command for (bridge deposits, today).
- Pre-1.0 versioning: bump minor on breaking changes, patch on features/fixes.
