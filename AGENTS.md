# Contributing guide — nexus-exchange-cli

The command-line client for the Nexus Exchange API, built on `nexus-exchange-rs`.

## Merging

- Don't merge a PR without an approving review — CI passing isn't a substitute.
- Don't merge a PR you didn't author without an approving review **and** the
  author's sign-off. Check the author first
  (`gh pr view <n> --json author,reviewDecision`).
- Re-approval isn't needed for follow-up commits to an already-approved PR.

## Pull requests

- One concern per PR; link its tracking issue (`ENG-XXXX`) in the title.
- Respond to review comments before merging.

## Checks (before pushing)

- `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` all pass — CI
  enforces these.

## Notes

- Capabilities are inherited from the `nexus-exchange-rs` dependency — bump it to
  pick up new endpoints rather than reimplementing.
- Pre-1.0 versioning: bump minor on breaking changes, patch on features/fixes.
