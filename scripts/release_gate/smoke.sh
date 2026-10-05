#!/usr/bin/env bash
# Pre-publish smoke test (ENG-18798). Builds the `nexus` binary as a release ships it, installs it
# outside the checkout, and makes one unauthenticated read against the public testnet with it. No
# keys, no writes.
#
#   scripts/release_gate/smoke.sh               build, install, read
#   scripts/release_gate/smoke.sh --build-only  build and install, no network (PRs that are not a release)
#
# The read lists markets, the same read every SDK's smoke test makes: `nexus market summary`, which
# is the SDK's `fetch_market_summaries`, the keyless way to enumerate them. `nexus markets` lists
# them with `fetch_markets` instead, which cannot decode what testnet serves while the spec pin
# sits at v0.8.1 (ENG-18801).
#
# Exit codes, kept apart on purpose: 0 passed, 1 failed, 2 testnet unreachable. The workflow fails
# on 1 and on 2, under different names. Unreachable is not a pass, and it is not the CLI's fault:
# re-run the job once testnet answers.
#
#   0  passed       the read decoded and returned at least one market
#   1  failed       the binary got an answer and could not use it (decode, 4xx, empty list)
#   2  unreachable  no usable answer from testnet: network, timeout, 5xx, rate limit
#
# The binary exits 1 on every error, so the split comes from the error it prints: the SDK's
# transient errors ("network error", "request timed out", "service unavailable", "rate limited")
# are the unreachable ones. Only the first cause is read; the lines under it can quote the
# server's own message, which may say anything. `NEXUS_SMOKE_BASE_URL` points the read elsewhere, through the binary's
# own `--base-url`, for testing the outcomes themselves.
set -euo pipefail

mode="read"
case "${1:-}" in
  "") ;;
  --build-only) mode="build" ;;
  *) echo "usage: $0 [--build-only]" >&2; exit 64 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "$root"
# Where cargo puts the build, CARGO_TARGET_DIR included, so a binary left in ./target by an older
# build is never the one tested.
target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# What cargo-dist builds for a release (`[profile.dist]`), copied out of the target dir and run
# with a fresh HOME and an empty environment: no config file, no stored or exported credentials.
cargo build --profile dist --locked --quiet
mkdir -p "$work/bin" "$work/home"
cp "$target_dir/dist/nexus" "$work/bin/nexus"
nexus() {
  env -i PATH="/usr/bin:/bin" HOME="$work/home" "$work/bin/nexus" "$@"
}
# Its own exit status is not passed on: clap exits 2 on a usage error, which would read as
# unreachable.
if ! installed="$(nexus --version)"; then
  echo "::error title=prepublish-smoke (failed)::The installed release binary does not run: \`nexus --version\` failed."
  exit 1
fi
echo "built and installed ${installed}"

summary() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '### Testnet smoke: %s\n\n%s\n' "$1" "$2" >> "$GITHUB_STEP_SUMMARY"
  fi
}

if [ "$mode" = "build" ]; then
  echo "::notice title=prepublish-smoke (read not attempted)::Not a release PR: the release binary built and ran from a clean install, and no testnet read was made. The read runs on the release PR."
  summary "build only" "Not a release PR: \`${installed}\` built and ran from a clean install. No testnet read was attempted, so this is not a smoke pass."
  exit 0
fi

args=(--network testnet)
target="testnet"
# The testnet host is not a flag here: it is built into the SDK crate the binary names, so an
# unreachable testnet can also be a stale crate pin.
host_note="; its testnet host is the one built into the nexus-exchange crate in ${installed}"
if [ -n "${NEXUS_SMOKE_BASE_URL:-}" ]; then
  args+=(--base-url "$NEXUS_SMOKE_BASE_URL")
  target="$NEXUS_SMOKE_BASE_URL"
  host_note=""
fi

set +e
nexus "${args[@]}" --output json market summary > "$work/out.json" 2> "$work/err.txt"
status=$?
set -e

if [ "$status" -eq 0 ]; then
  set +e
  line="$(python3 - "$work/out.json" "$target" <<'EOF'
import json, sys
path, target = sys.argv[1], sys.argv[2]
try:
    markets = json.load(open(path))
except ValueError as err:
    print(f"nexus market summary against {target} printed JSON that does not parse: {err}")
    sys.exit(1)
if not isinstance(markets, list):
    print(f"nexus market summary against {target} printed a {type(markets).__name__}, not a list of markets")
    sys.exit(1)
if not markets:
    print(f"nexus market summary decoded an EMPTY list from {target}")
    sys.exit(1)
print(f"nexus market summary decoded {len(markets)} markets from {target} (first: {markets[0].get('market_id')})")
EOF
)"
  code=$?
  set -e
else
  # The error the binary reports, from its `Error:` line on (the network notice and the
  # --base-url deprecation notice above it are not the error), on one line.
  error="$(sed -n '/^Error: /,$p' "$work/err.txt")"
  [ -n "$error" ] || error="$(cat "$work/err.txt")"
  error="$(printf '%s' "$error" | tr -s '[:space:]' ' ' | cut -c1-400)"
  # The first line under `Caused by:` (the `Error:` line when there is none). A 404 whose message
  # has a line reading "network error" is still a 404 (ENG-18798 review).
  cause="$(sed -n '/^Caused by:$/{n;p;q}' "$work/err.txt")"
  [ -n "$cause" ] || cause="$(sed -n '/^Error: /{s/^Error: //;p;q}' "$work/err.txt")"
  if grep -qE '^ *([0-9]+: )?(network error|request timed out|service unavailable|rate limited)' <<< "$cause"; then
    code=2
    line="${target} gave no usable answer (exit ${status}${host_note}): ${error}"
  else
    code=1
    line="nexus market summary against ${target} failed (exit ${status}): ${error}"
  fi
fi

case "$code" in
  0)
    line="smoke: passed: ${line}"
    echo "$line"
    summary "✅ passed" "$line"
    ;;
  2)
    line="smoke: unreachable: ${line}"
    echo "$line"
    echo "::error title=prepublish-smoke (testnet unreachable)::NOT a pass: the testnet read got no usable answer, so nothing about this release was verified. Re-run this job once testnet answers. ${line}"
    summary "⚠️ TESTNET UNREACHABLE: not a pass" "$line"
    ;;
  *)
    line="smoke: failed: ${line}"
    echo "$line"
    echo "::error title=prepublish-smoke (failed)::The release binary could not make an unauthenticated testnet read. ${line}"
    summary "❌ FAILED" "$line"
    code=1
    ;;
esac
exit "$code"
