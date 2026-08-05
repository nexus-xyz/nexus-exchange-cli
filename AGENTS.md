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
- Declare a breaking change with `!` before the colon (`feat!:`). A
  `BREAKING CHANGE:` footer works only in a **commit** body — the squash body
  comes from the branch's commit messages, never the PR description, so a footer
  written only in the description is dropped at merge.

## Pull requests

- One concern per PR; link its tracking issue (`ENG-XXXX`) in the title.
- Write the title as a [conventional commit](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `docs:`, `chore:`, `ci:`) — it becomes the commit subject on
  `main`, so it is what drives the release. See [Merging](#merging).
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
  the marked README block; run `scripts/sync_sdk_version.py --write`. CI fails if
  the pin, the README line, and the crate disagree.
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
