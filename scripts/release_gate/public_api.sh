#!/usr/bin/env bash
# Public-surface snapshot (ENG-18798). Lists the commands, arguments and flags of the `nexus` binary
# exactly as a release ships it, and compares them with the committed `public-api.txt`.
#
#   scripts/release_gate/public_api.sh           check: fail on any difference (CI, `prepublish-surface`)
#   scripts/release_gate/public_api.sh --write   regenerate public-api.txt after a deliberate CLI change
#
# Any difference fails, additions included, so the snapshot moves in the same PR as the code. A
# removed or changed command or flag then shows up as a `-` line in the diff a reviewer reads,
# instead of first surfacing in someone's script after the release.
#
# What a release ships is the binary cargo-dist builds, which is `cargo build --profile dist`
# (`[profile.dist]` in Cargo.toml), so that is the build here, with --locked like every CI build.
# The listing comes from running that binary, copied out of target/, with no config and an empty
# environment (scripts/release_gate/cli_surface.py), not from reading src/cli.rs. Nothing here needs
# a tool the repo does not already use: cargo and python3.
set -euo pipefail

SNAPSHOT="public-api.txt"

mode="check"
case "${1:-}" in
  "") ;;
  --write) mode="write" ;;
  *) echo "usage: $0 [--write]" >&2; exit 2 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "$root"

name="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["name"])')"
version="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
# Where cargo puts the build, CARGO_TARGET_DIR included, so a binary left in ./target by an older
# build is never the one listed.
target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cargo build --profile dist --locked --quiet
mkdir -p "$work/bin"
cp "$target_dir/dist/nexus" "$work/bin/nexus"
built="${name} ${version}, dist profile"

python3 scripts/release_gate/cli_surface.py "$work/bin/nexus" > "$work/public-api.txt"

if [ "$mode" = "write" ]; then
  cp "$work/public-api.txt" "$SNAPSHOT"
  echo "wrote $SNAPSHOT ($(wc -l < "$SNAPSHOT") items) from ${built}"
  exit 0
fi

if diff -u --label "$SNAPSHOT (committed)" --label "$SNAPSHOT (built ${built})" \
  "$SNAPSHOT" "$work/public-api.txt" > "$work/diff.txt"; then
  echo "public surface matches $SNAPSHOT ($(wc -l < "$SNAPSHOT") items)"
  exit 0
fi

removed="$(grep -c '^-[^-]' "$work/diff.txt" || true)"
added="$(grep -c '^+[^+]' "$work/diff.txt" || true)"
cat "$work/diff.txt"
echo "::error title=prepublish-surface::The built binary's commands and flags differ from $SNAPSHOT: ${removed} item(s) gone or changed, ${added} new. If that is deliberate, run scripts/release_gate/public_api.sh --write and commit $SNAPSHOT in this PR, so the change is in the diff a reviewer reads. A removal or change is breaking."
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Public surface: ❌ differs from \`$SNAPSHOT\`"
    echo
    echo "${removed} item(s) gone or changed (\`-\`), ${added} new (\`+\`). Regenerate with \`scripts/release_gate/public_api.sh --write\` if deliberate."
    echo
    echo '```diff'
    cat "$work/diff.txt"
    echo '```'
  } >> "$GITHUB_STEP_SUMMARY"
fi
exit 1
