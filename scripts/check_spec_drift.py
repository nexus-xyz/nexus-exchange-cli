#!/usr/bin/env python3
"""Check the CLI's targeted endpoints against the pinned OpenAPI spec AND the
CLI's own command-handler code.

This is the CLI counterpart to the same-named script in the Rust SDK
(nexus-exchange-rs). The SDK script parses path literals out of `self.get("...")`
helper calls in src/rest.rs; the CLI never issues raw HTTP — it is a thin layer
over the SDK's `Client`, so it instead calls *named* SDK methods
(`client.fetch_markets()`, `client.create_order(...)`, ...). We therefore derive
the CLI's targeted op set from those `client.<method>(` calls and map each method
to its spec operation via the METHOD_OP table below.

Four invariants are enforced. 1 and 2 are the contract; 3 and 4 exist because both
of them can be silently defeated — an allowlist entry that stops being earned, or a
command handler in a file the parser never reads — and a defeated check still
prints OK. `scripts/test_check_spec_drift.py` defeats each one and asserts it goes
red.

1. endpoints.txt <-> spec
   Every endpoint the CLI targets (endpoints.txt) must exist in the pinned
   OpenAPI spec (.api-version). A miss means a breaking change, rename, or typo
   in the spec. Spec operations the CLI does not cover are reported as an
   informational coverage gap, and the coverage % is printed (this is the number
   the dashboard's CLI panel reads).

2. CLI code <-> endpoints.txt
   The set of SDK methods the CLI actually calls (parsed from the source files in
   CLI_SOURCES and mapped through METHOD_OP) must EQUAL the endpoints.txt set —
   real set equality in both directions, not a subset check — modulo two
   explicit, documented allowlists:

     * CODE_ONLY_OPS    — a command calls an SDK method, but the op is AHEAD OF
                          the pinned spec, so it is intentionally NOT in
                          endpoints.txt (listing it would break invariant 1
                          until the spec ships the op).
     * NON_REST_TARGETS — listed in endpoints.txt but reached WITHOUT a named
                          REST method call (e.g. the WebSocket upgrade GET /ws,
                          opened by the streaming client).

   The check fails if (a) the CLI calls a mapped method whose op is neither in
   endpoints.txt nor CODE_ONLY_OPS, or (b) endpoints.txt lists an op that no
   called method maps to and that is not in NON_REST_TARGETS.

3. Allowlist hygiene — an exemption must stay earned (ENG-7962)
   Both allowlists suppress a real invariant, so a stale entry is a silent hole
   rather than a visible one. Three ways an entry stops being earned, all fatal:

     * a CODE_ONLY_OPS op no command calls any more — nothing to exempt;
     * a CODE_ONLY_OPS op the PINNED SPEC now defines — either the spec caught up
       (move the line into endpoints.txt) or the METHOD_OP row names the wrong
       verb for a path the spec does model. The second case is what ENG-7962
       actually found: `amend_order` was mapped to `PUT /orders/{order_id}` while
       the SDK issues `PATCH`, and `PATCH /orders/{order_id}` has been in the spec
       since v0.7.1 — so a covered operation sat exempted and uncounted;
     * a NON_REST_TARGETS op that endpoints.txt does not list — exempting a line
       that isn't there.

4. CLI_SOURCES completeness
   No .rs file OUTSIDE CLI_SOURCES may contain a mapped SDK method call — anywhere
   under src/, at any depth. The parser only reads the listed files, so moving a
   command handler into a new module would otherwise silently under-count the CLI's
   targeted ops and read as green. Fail instead, and name the file to add.

Usage: check_spec_drift.py <openapi.json>
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

# CLI source files that dispatch to the SDK. The main match arm lives in
# main.rs; the WebSocket token mint lives in wsclient.rs. Keep this list in sync
# if command handling moves to new modules.
CLI_SOURCES = [
    os.path.join(REPO, "src", "main.rs"),
    os.path.join(REPO, "src", "wsclient.rs"),
]

# Every .rs file the CLI builds from. Invariant 4 walks this tree and scans the
# files NOT in CLI_SOURCES for mapped SDK calls, so "the handler moved to a new
# module" is a red check rather than a silent under-count — including a nested one
# (`src/commands/orders.rs`), which is the likeliest way such a module appears.
SRC_DIR = os.path.join(REPO, "src")

# Map each SDK `Client` method the CLI calls to the (METHOD, path) spec operation
# it issues. This is the CLI's equivalent of the SDK's HELPER_METHOD+path-literal
# parsing: the CLI has no path literals of its own, so the mapping is explicit.
# Paths use the spec's placeholder names. Add a row when a command starts calling
# a new SDK method (and add the matching endpoints.txt line, or a CODE_ONLY_OPS
# entry if the op is ahead of the pinned spec).
# /api/v1 migration (ENG-4949): the gateway REST proxy is being replaced by the
# per-service host-root `/api/v1` surface (parent ENG-4740). The move is dual-stack
# (ENG-4751): ops with an `/api/v1` variant point there; the rest keep the bare
# gateway path until they gain one. The path each row carries MUST mirror what the
# regenerated SDK actually calls (nexus-exchange-rs PR #85 / ENG-4947), which picks
# the base per request off the `/api/v1/` prefix — the CLI issues no path of its own.
METHOD_OP = {
    # public market data
    "fetch_markets": ("GET", "/markets"),  # list-all: no /api/v1 variant yet
    "fetch_market_summaries": ("GET", "/api/v1/markets/summary"),
    "fetch_tickers": ("GET", "/api/v1/tickers"),
    "fetch_ticker": ("GET", "/api/v1/markets/{market_id}/ticker"),
    "fetch_order_book": ("GET", "/api/v1/markets/{market_id}/orderbook"),
    "fetch_trades": ("GET", "/api/v1/markets/{market_id}/trades"),
    "fetch_ohlcv": ("GET", "/api/v1/markets/{market_id}/candles"),
    "fetch_funding_rate_history": ("GET", "/api/v1/markets/{market_id}/funding"),
    "fetch_mark_price": ("GET", "/api/v1/markets/{market_id}/mark-price"),
    "fetch_market_status": ("GET", "/api/v1/markets/{market_id}/status"),
    "health_check": ("GET", "/status"),  # v0.7.1 replaced /health with /status
    # ADL reads (HMAC-gated server-side despite the market scope)
    "fetch_market_adl_events": ("GET", "/markets/{market_id}/adl-events"),  # no /api/v1 variant yet
    "fetch_account_adl_history": ("GET", "/account/{address}/adl-history"),  # no /api/v1 variant yet
    # authenticated account (read)
    "fetch_balance": ("GET", "/api/v1/account"),
    # portfolio-parity reads (ENG-6460), added to the spec in v0.7.2
    "fetch_account_summary": ("GET", "/api/v1/account/summary"),
    "fetch_account_state": ("GET", "/api/v1/account/state"),
    "fetch_account_fees": ("GET", "/api/v1/account/fees"),
    "fetch_portfolio_history": ("GET", "/api/v1/account/portfolio-history"),
    "fetch_positions": ("GET", "/api/v1/positions"),
    "fetch_my_trades": ("GET", "/api/v1/fills"),
    "fetch_open_orders": ("GET", "/api/v1/orders"),
    "fetch_order": ("GET", "/orders/{order_id}"),  # v1 exposes no GET-by-id
    "fetch_withdrawals": ("GET", "/withdrawals"),  # no /api/v1 variant yet
    "fetch_rate_limit_status": ("GET", "/api/v1/account/rate-limit"),
    "fetch_api_keys": ("GET", "/keys"),  # no /api/v1 variant yet
    "fetch_agents": ("GET", "/agents"),  # no /api/v1 variant yet
    # trading & account mutations
    "create_order": ("POST", "/api/v1/orders"),
    "create_orders": ("POST", "/api/v1/orders/batch"),
    "cancel_order": ("DELETE", "/api/v1/orders/{order_id}"),
    "cancel_all_orders": ("DELETE", "/api/v1/orders"),
    # Per-market flatten: the same DELETE-orders op as cancel_all_orders, scoped
    # by a `market_id` query parameter (queries don't change the spec op).
    "cancel_orders_for_market": ("DELETE", "/api/v1/orders"),
    "deposit": ("POST", "/account/deposit"),  # no /api/v1 variant yet
    "claim_credit": ("POST", "/api/v1/account/credit"),
    "create_api_key": ("POST", "/keys"),  # no /api/v1 variant yet
    "delete_api_key": ("DELETE", "/keys/{key_id}"),  # no /api/v1 variant yet
    "revoke_agent": ("DELETE", "/agents/{address}"),  # no /api/v1 variant yet
    # v1 exposes no amend; the SDK issues signed_patch_with_query on the gateway
    # path (nexus-exchange 0.6.0 src/rest.rs::amend_order). It was mapped to PUT
    # here, which hid a covered operation behind CODE_ONLY_OPS — see ENG-7962 and
    # invariant 3.
    "amend_order": ("PATCH", "/orders/{order_id}"),
    # websocket
    "mint_web_socket_token": ("POST", "/ws/token"),  # no /api/v1 variant yet
    # ── ahead of the pinned spec (see CODE_ONLY_OPS) ──
    "set_leverage": ("POST", "/account/leverage"),
    "set_margin_mode": ("POST", "/account/margin-mode"),
    "fetch_funding_payments": ("GET", "/funding-payments"),
    "create_transfer": ("POST", "/transfers"),
    "fetch_transfers": ("GET", "/transfers"),
    "create_sub_account": ("POST", "/sub-accounts"),
    "fetch_sub_accounts": ("GET", "/sub-accounts"),
    "cancel_orders": ("POST", "/orders/batch-cancel"),
    "fetch_order_by_client_id": ("GET", "/orders/by-client-id/{client_order_id}"),
    "cancel_order_by_client_id": ("DELETE", "/orders/by-client-id/{client_order_id}"),
}

# Called by a command but intentionally absent from endpoints.txt: these ops are
# AHEAD OF the pinned spec, so adding them to endpoints.txt would (correctly)
# fail the endpoints.txt<->spec invariant until the spec ships them. Move a row
# out of here into endpoints.txt once the pinned spec gains the operation.
#
# Invariant 3 keeps this list honest: an entry the pinned spec defines (at the
# same path, under ANY method) fails the check, so "ahead of spec" cannot quietly
# become "covered but uncounted". `PUT /orders/{}` used to sit here on exactly
# that mistake (ENG-7962) — the operation is `PATCH /orders/{order_id}` and the
# spec has had it since v0.7.1, so it now lives in endpoints.txt.
CODE_ONLY_OPS = {
    ("POST", "/account/leverage"),       # account leverage -> set_leverage
    ("POST", "/account/margin-mode"),    # account margin-mode -> set_margin_mode
    ("GET", "/funding-payments"),        # funding-payments -> fetch_funding_payments
    ("POST", "/transfers"),              # transfers create -> create_transfer
    ("GET", "/transfers"),               # transfers list -> fetch_transfers
    ("POST", "/sub-accounts"),           # sub-accounts create -> create_sub_account
    ("GET", "/sub-accounts"),            # sub-accounts list -> fetch_sub_accounts
    ("POST", "/orders/batch-cancel"),    # order cancel-batch -> cancel_orders
    ("GET", "/orders/by-client-id/{}"),  # order get-by-client-id -> fetch_order_by_client_id
    ("DELETE", "/orders/by-client-id/{}"),  # order cancel-by-client-id -> cancel_order_by_client_id
}

# Listed in endpoints.txt but reached WITHOUT a named SDK REST method call, so the
# source parser cannot (and should not) see it. The WebSocket upgrade is opened by
# the streaming client (ws_client.connect(...) in src/wsclient.rs), not a
# `client.<method>()` call. Paths use the normalized `{}` placeholder form.
NON_REST_TARGETS = {
    ("GET", "/ws"),
}

# Spec operations that exist but the CLI deliberately does not target. Documented
# here so the exclusion is intentional, not an oversight:
#   POST /auth/login, POST /agents/register — wallet-signed auth flows owned by a
#     separate in-flight PR (ENG-4046); their endpoints.txt lines land with it.
#   PUT/GET/DELETE /admin/tiers* — admin-only tier management; out of CLI scope.
#   POST /ws-tokens — deprecated; superseded by POST /ws/token (which we use).
#   GET  /stream — deprecated SSE stream; superseded by the /ws upgrade.


def normalize_path(p):
    """Collapse any `{placeholder}` segment to a bare `{}` so paths match by
    position, not placeholder name."""
    return re.sub(r"\{[^}]*\}", "{}", p)


def load_targeted(path="endpoints.txt"):
    out = []
    seen = {}
    with open(path) as f:
        for lineno, raw in enumerate(f, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 1)
            if len(parts) != 2:
                sys.exit(
                    f"ERROR: {path}:{lineno}: expected 'METHOD /path', got {line!r}"
                )
            method, p = parts
            op = (method.upper(), p)
            if op in seen:
                sys.exit(
                    f"ERROR: {path}:{lineno}: duplicate endpoint "
                    f"{op[0]} {op[1]!r} (first seen on line {seen[op]})"
                )
            seen[op] = lineno
            out.append(op)
    return out


def spec_ops(spec):
    ops = set()
    for p, methods in spec.get("paths", {}).items():
        for m in methods:
            if m.lower() in ("get", "post", "put", "delete", "patch"):
                ops.add((m.upper(), p))
    return ops


# Match `client.<method>(` or `<receiver>.<method>(` where <method> is one of the
# mapped SDK methods. We anchor on the method name from METHOD_OP rather than the
# receiver, so a renamed binding (e.g. `ws_client`) is still seen.
_CALL_RE = re.compile(
    r"\.(" + "|".join(sorted(METHOD_OP, key=len, reverse=True)) + r")\s*\("
)


def called_ops(sources=CLI_SOURCES):
    """Derive the set of (METHOD, normalized_path) the CLI targets from the
    `client.<method>(` calls in the CLI source files, mapped via METHOD_OP."""
    ops = set()
    seen_methods = set()
    for path in sources:
        try:
            with open(path) as f:
                src = f.read()
        except OSError as e:
            sys.exit(f"ERROR: cannot read CLI source {path!r}: {e}")
        for m in _CALL_RE.finditer(src):
            method = m.group(1)
            seen_methods.add(method)
            mth, p = METHOD_OP[method]
            ops.add((mth, normalize_path(p)))
    if not ops:
        sys.exit(
            "ERROR: parsed zero SDK method calls from the CLI sources; the call "
            "pattern may have changed — update METHOD_OP / the parser."
        )
    return ops, seen_methods


def _walk_error(err):
    """os.walk swallows errors by default; this invariant must fail closed instead —
    a directory it cannot read is a directory it cannot clear."""
    where = getattr(err, "filename", None) or "the src tree"
    sys.exit(f"ERROR: cannot scan {where!r} for unscanned SDK calls: {err}")


def unscanned_sources(src_dir=SRC_DIR, sources=CLI_SOURCES):
    """Invariant 4: .rs files under src/ that the parser does NOT read but which
    contain a mapped SDK method call. Returns [(path, sorted(methods))].

    Walks the whole tree rather than listing one level. `src/` is flat today, but
    "the handler moved into a new module" is precisely what this invariant exists to
    catch, and the natural way to add a module is `src/commands/orders.rs` — which a
    single-level listing would not see, leaving the check green on the one change it
    was written for."""
    scanned = {os.path.abspath(p) for p in sources}
    offenders = []
    for root, dirs, names in os.walk(src_dir, onerror=_walk_error):
        dirs.sort()  # deterministic report order
        for name in sorted(names):
            path = os.path.join(root, name)
            if not name.endswith(".rs") or os.path.abspath(path) in scanned:
                continue
            try:
                with open(path) as f:
                    src = f.read()
            except OSError as e:
                sys.exit(f"ERROR: cannot read CLI source {path!r}: {e}")
            found = sorted({m.group(1) for m in _CALL_RE.finditer(src)})
            if found:
                offenders.append((os.path.relpath(path, REPO), found))
    return offenders


def check_code_vs_targets(targeted, available, sources=None, src_dir=None):
    """Invariants 2-4: called SDK-method ops == endpoints.txt (modulo the two
    documented allowlists), the allowlists are still earned, and no unscanned
    source file reaches the API. Returns the number of errors printed.

    `sources` / `src_dir` default to the real CLI_SOURCES / SRC_DIR; the self-test
    overrides them with synthetic Rust so each invariant can be defeated in
    isolation."""
    sources = CLI_SOURCES if sources is None else sources
    src_dir = SRC_DIR if src_dir is None else src_dir
    called, _ = called_ops(sources)
    targeted_norm = {(m, normalize_path(p)) for m, p in targeted}

    # (a) called but not listed (and not an intentional code-only op).
    called_missing_from_targets = sorted(called - targeted_norm - CODE_ONLY_OPS)
    # (b) listed but not called (and not an intentional non-REST target).
    targets_without_call = sorted(targeted_norm - called - NON_REST_TARGETS)
    # Invariant 3: a CODE_ONLY_OPS entry no command calls is stale.
    stale_code_only = sorted(CODE_ONLY_OPS - called)
    # Invariant 3: a CODE_ONLY_OPS entry the pinned spec already defines is not
    # "ahead of spec". Compare by PATH (any method) so a wrong verb in METHOD_OP
    # is caught too — that is the failure mode ENG-7962 found. Report the spec's
    # own methods for the path so the fix is obvious from the log.
    spec_methods_by_path = {}
    for m, p in available:
        spec_methods_by_path.setdefault(normalize_path(p), set()).add(m)
    caught_up_code_only = sorted(
        (m, p, sorted(spec_methods_by_path[p]))
        for m, p in CODE_ONLY_OPS
        if p in spec_methods_by_path
    )
    # Invariant 3: a NON_REST_TARGETS entry endpoints.txt does not list exempts
    # nothing.
    stale_non_rest = sorted(NON_REST_TARGETS - targeted_norm)
    # Invariant 4.
    unscanned = unscanned_sources(src_dir, sources)

    errors = 0
    if called_missing_from_targets:
        errors += len(called_missing_from_targets)
        print(
            f"\nERROR: {len(called_missing_from_targets)} op(s) the CLI calls are "
            f"NOT in endpoints.txt (add them, or add to CODE_ONLY_OPS if "
            f"intentionally ahead of spec):"
        )
        for m, p in called_missing_from_targets:
            print(f"  - {m} {p}")

    if targets_without_call:
        errors += len(targets_without_call)
        print(
            f"\nERROR: {len(targets_without_call)} endpoints.txt entr(ies) have no "
            f"calling command in the CLI sources (remove them, or add to "
            f"NON_REST_TARGETS if reached without a REST method call):"
        )
        for m, p in targets_without_call:
            print(f"  - {m} {p}")

    if stale_code_only:
        errors += len(stale_code_only)
        print(
            f"\nERROR: {len(stale_code_only)} CODE_ONLY_OPS entr(ies) are no longer "
            f"called by any command (remove them from the allowlist):"
        )
        for m, p in stale_code_only:
            print(f"  - {m} {p}")

    if caught_up_code_only:
        errors += len(caught_up_code_only)
        print(
            f"\nERROR: {len(caught_up_code_only)} CODE_ONLY_OPS entr(ies) are NOT "
            f"ahead of the pinned spec — the spec defines that path, so the "
            f"exemption is hiding a covered operation:"
        )
        for m, p, spec_methods in caught_up_code_only:
            if m in spec_methods:
                print(
                    f"  - {m} {p}: the pinned spec has it. Move the line into "
                    f"endpoints.txt and drop it from CODE_ONLY_OPS."
                )
            else:
                print(
                    f"  - {m} {p}: the pinned spec models this path as "
                    f"{'/'.join(spec_methods)}, not {m}. Fix the METHOD_OP verb to "
                    f"match what the SDK issues, then move the line into "
                    f"endpoints.txt."
                )

    if stale_non_rest:
        errors += len(stale_non_rest)
        print(
            f"\nERROR: {len(stale_non_rest)} NON_REST_TARGETS entr(ies) are not "
            f"listed in endpoints.txt, so they exempt nothing (remove them from the "
            f"allowlist, or add the endpoints.txt line they were meant to cover):"
        )
        for m, p in stale_non_rest:
            print(f"  - {m} {p}")

    if unscanned:
        errors += len(unscanned)
        print(
            f"\nERROR: {len(unscanned)} CLI source file(s) outside CLI_SOURCES call "
            f"mapped SDK methods, so their ops are invisible to this check (add "
            f"them to CLI_SOURCES):"
        )
        for path, methods in unscanned:
            print(f"  - {path}: {', '.join(methods)}")

    if not errors:
        print(
            f"\nOK: the CLI calls {len(called)} mapped SDK op(s); all are in "
            f"endpoints.txt or CODE_ONLY_OPS, and every endpoints.txt entry has a "
            f"calling command or is in NON_REST_TARGETS."
        )
        print(
            f"OK: both allowlists are still earned "
            f"({len(CODE_ONLY_OPS)} ahead-of-spec, {len(NON_REST_TARGETS)} non-REST), "
            f"and no source file outside CLI_SOURCES reaches the API."
        )
    return errors


def check_targets_vs_spec(targeted, available):
    """Invariant 1: every endpoints.txt op exists in the pinned spec. Also prints
    the coverage line the dashboard scrapes and the informational uncovered list.
    Returns the number of errors printed."""
    missing = [op for op in targeted if op not in available]
    uncovered = sorted(available - set(targeted))

    pct = 100.0 * len(targeted) / len(available) if available else 0.0
    print(
        f"CLI targets {len(targeted)} of {len(available)} spec endpoints "
        f"({pct:.1f}% coverage)."
    )

    if uncovered:
        print(f"\nNot covered by the CLI ({len(uncovered)}):")
        for m, p in uncovered:
            print(f"  - {m} {p}")

    if not missing:
        print("\nOK: every targeted endpoint exists in the pinned spec.")
        return 0

    print(
        f"\nERROR: {len(missing)} targeted endpoint(s) are NOT in the spec "
        f"(removed/renamed/typo):"
    )
    for m, p in missing:
        print(f"  - {m} {p}")
    return len(missing)


def main():
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <openapi.json>")
    with open(sys.argv[1]) as f:
        spec = json.load(f)
    version = spec.get("info", {}).get("version", "?")
    targeted = load_targeted()
    available = spec_ops(spec)

    print(f"Spec version: {version}")

    # Invariant 1: endpoints.txt <-> spec.
    failures = check_targets_vs_spec(targeted, available)
    # Invariants 2-4: CLI code <-> endpoints.txt, allowlist hygiene, and
    # CLI_SOURCES completeness.
    failures += check_code_vs_targets(targeted, available)

    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
