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

## Notes

- Capabilities are inherited from the `nexus-exchange-rs` dependency — bump it to
  pick up new endpoints rather than reimplementing.
- Pre-1.0 versioning: bump minor on breaking changes, patch on features/fixes.
