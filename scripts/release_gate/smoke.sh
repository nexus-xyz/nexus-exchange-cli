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
# server's own message, which may say anything. `NEXUS_SMOKE_BASE_URL` points the read elsewhere,
# through the binary's own `--base-url`.
#
# EXITS 2 AND 1 ARE EXERCISED ON EVERY PR; EXIT 0 ONLY BY THE REAL READ. In both modes, after the
# build and before any testnet read, the binary just built makes the same read four times through
# `--base-url`, and the classification below has to give:
#
#   http://127.0.0.1:1                           nothing listens there                    2  unreachable
#   scripts/release_gate/stub_server.py          answers 200 `[]` on every path           1  failed (no market)
#   scripts/release_gate/stub_server.py 503      answers 503 on every path, an outage     2  unreachable
#   scripts/release_gate/stub_server.py 404 FILE answers 404 with a `{ code, message }`   1  failed (a 404)
#                                                body whose message has a line reading
#                                                "network error"
#
# Any of them coming out different, or not finishing within 180 s, fails the job as
# `prepublish-smoke (outcomes)`: a smoke that cannot tell those apart would report an outage as a
# failure or a broken release as an outage. A pass (exit 0) needs a testnet with markets, so only
# the real read below shows it, on a release PR or a change to this gate. The probes run the binary
# directly, not this script, with its stdout and stderr captured, so they print no annotation of
# their own: one plain line each, with the case, the exit code and the first line of the output.
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
target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

work="$(mktemp -d)"
stub_pid=""
trap 'if [ -n "$stub_pid" ]; then kill "$stub_pid" 2> /dev/null || true; fi; rm -rf "$work"' EXIT

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

# One `nexus market summary` against $1, or against testnet when $1 is empty, classified: sets
# `code` (0 passed, 1 failed, 2 unreachable) and `line`, and prints nothing. The caller reports.
#
# $2, if set, is a hard limit in seconds (the probes'), enforced by $timeout_tool; `status` is
# then 124 if the read hit it. The real read has none: the SDK's own 30 s timeout and its retries
# bound it.
read_markets() {
  local args=(--network testnet) target="testnet" error
  local run=(env -i PATH="/usr/bin:/bin" HOME="$work/home" "$work/bin/nexus")
  if [ -n "${2:-}" ]; then
    run=("$timeout_tool" "$2" "${run[@]}")
  fi
  # The testnet host is not a flag here: it is built into the SDK crate the binary names, so an
  # unreachable testnet can also be a stale crate pin.
  local host_note="; its testnet host is the one built into the nexus-exchange crate in ${installed}"
  if [ -n "$1" ]; then
    args+=(--base-url "$1")
    target="$1"
    host_note=""
  fi

  set +e
  "${run[@]}" "${args[@]}" --output json market summary > "$work/out.json" 2> "$work/err.txt"
  status=$?
  set -e

  cause=""
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
}

# The probes' limit. Long enough for the SDK's retries with backoff on a transient error (a
# client-side timeout alone took about 2 minutes), short enough that a hung probe ends the job.
probe_limit=180
# `timeout` is GNU coreutils; macOS has none by default, and Homebrew's coreutils names it
# `gtimeout`. Without either the probes still run, only without their limit.
if command -v timeout > /dev/null; then
  timeout_tool="timeout"
elif command -v gtimeout > /dev/null; then
  timeout_tool="gtimeout"
else
  timeout_tool=""
  echo "note: neither timeout nor gtimeout is installed, so the outcome probes run without their ${probe_limit} s limit"
fi

# The outcome probes (header). $1 names the case, $2 is the base URL, $3 the code it must give.
probe() {
  local first
  read_markets "$2" "${timeout_tool:+$probe_limit}"
  if [ -n "$timeout_tool" ] && [ "$status" -eq 124 ]; then
    echo "::error title=prepublish-smoke (outcomes)::The ${1} case did not finish within ${probe_limit} s, so the smoke cannot be trusted to tell its outcomes apart."
    exit 1
  fi
  # The first line of what it printed: the cause of its error, or the first line of its JSON.
  first="$(printf '%s' "${cause:-$(head -n 1 "$work/out.json")}" | sed 's/^ *//')"
  echo "outcome probe: ${1}: exit ${code}; ${first}"
  if [ "$code" -ne "$3" ]; then
    echo "::error title=prepublish-smoke (outcomes)::The ${1} case gave exit ${code}, not ${3}, so the smoke cannot be trusted to tell its outcomes apart: ${line}"
    exit 1
  fi
}

# The stub on a free port, answering $1 (empty: 200 `[]`) to everything, with the bytes of file $2
# as the body if it is given. Sets stub_pid and port.
start_stub() {
  # Created first, so the read below never meets a missing file, however slowly the stub starts.
  : > "$work/stub.port"
  python3 -I scripts/release_gate/stub_server.py ${1:+"$1"} ${2:+"$2"} > "$work/stub.port" &
  stub_pid=$!
  # Wait for the first line to end (a newline in the file), or for the stub to die: a non-empty
  # file can still hold half the port (ENG-21487).
  for _ in $(seq 100); do
    if [ "$(wc -l < "$work/stub.port")" -ge 1 ] || ! kill -0 "$stub_pid" 2>/dev/null; then break; fi
    sleep 0.1
  done
  port="$(head -n 1 "$work/stub.port")"
  if ! [[ "$port" =~ ^[0-9]+$ ]]; then
    echo "::error title=prepublish-smoke (outcomes)::scripts/release_gate/stub_server.py ${1:-} ${2:-} did not print its port within 10 s."
    exit 1
  fi
}

stop_stub() {
  kill "$stub_pid" 2> /dev/null || true
  wait "$stub_pid" 2> /dev/null || true
  stub_pid=""
}

probe "unreachable (nothing listening on 127.0.0.1:1)" "http://127.0.0.1:1" 2

start_stub
probe "failed (a stub that answers [] with no market)" "http://127.0.0.1:${port}" 1
stop_stub

start_stub 503
probe "unreachable (a stub that answers 503, an outage)" "http://127.0.0.1:${port}" 2
stop_stub

# The API's `{ code, message }` error envelope, which the SDK decodes into the error it prints. The
# message's second line reads "network error", as the SDK's own transport error does: only the
# first line of the cause is classified, so this is a 404 and must fail, not read as unreachable.
printf '%s' '{"code": "NOT_FOUND", "message": "no such route\nnetwork error: connection refused"}' > "$work/404.json"
start_stub 404 "$work/404.json"
probe "failed (a stub that answers 404 with \"network error\" in its message)" "http://127.0.0.1:${port}" 1
stop_stub

if [ "$mode" = "build" ]; then
  echo "::notice title=prepublish-smoke (read not attempted)::Not a release PR: the release binary built and ran from a clean install, and gave the unreachable and failed outcomes where it had to (a dead port and a 503 stub unreachable, an empty stub and a 404 stub failed). No testnet read was made, so a pass was not shown. The read runs on the release PR."
  summary "build only" "Not a release PR: \`${installed}\` built and ran from a clean install, and gave *unreachable* against a dead port and a 503 stub and *failed* against an empty stub and a 404 stub, as it had to. No testnet read was attempted, so this is not a smoke pass."
  exit 0
fi

read_markets "${NEXUS_SMOKE_BASE_URL:-}"

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
