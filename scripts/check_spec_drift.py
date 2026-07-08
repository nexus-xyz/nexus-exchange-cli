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

Two independent invariants are enforced:

1. endpoints.txt <-> spec
   Every endpoint the CLI targets (endpoints.txt) must exist in the pinned
   OpenAPI spec (.api-version). A miss means a breaking change, rename, or typo
   in the spec. Spec operations the CLI does not cover are reported as an
   informational coverage gap, and the coverage % is printed (this is the number
   the dashboard's CLI panel reads).

2. CLI code <-> endpoints.txt
   The set of SDK methods the CLI actually calls (parsed from the source files in
   CLI_SOURCES and mapped through METHOD_OP) must equal the endpoints.txt set,
   modulo two explicit, documented allowlists:

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
    "health_check": ("GET", "/health"),  # no /api/v1 variant yet
    # authenticated account (read)
    "fetch_balance": ("GET", "/api/v1/account"),
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
    "deposit": ("POST", "/account/deposit"),  # no /api/v1 variant yet
    "claim_credit": ("POST", "/api/v1/account/credit"),
    "create_api_key": ("POST", "/keys"),  # no /api/v1 variant yet
    "delete_api_key": ("DELETE", "/keys/{key_id}"),  # no /api/v1 variant yet
    "revoke_agent": ("DELETE", "/agents/{address}"),  # no /api/v1 variant yet
    # websocket
    "mint_web_socket_token": ("POST", "/ws/token"),  # no /api/v1 variant yet
    # ── ahead of the pinned spec (see CODE_ONLY_OPS) ──
    "amend_order": ("PUT", "/orders/{order_id}"),
    "set_leverage": ("POST", "/account/leverage"),
    "set_margin_mode": ("POST", "/account/margin-mode"),
    "fetch_funding_payments": ("GET", "/funding-payments"),
    "create_transfer": ("POST", "/transfers"),
    "fetch_transfers": ("GET", "/transfers"),
    "create_sub_account": ("POST", "/sub-accounts"),
    "fetch_sub_accounts": ("GET", "/sub-accounts"),
}

# Called by a command but intentionally absent from endpoints.txt: these ops are
# AHEAD OF the pinned spec, so adding them to endpoints.txt would (correctly)
# fail the endpoints.txt<->spec invariant until the spec ships them. Move a row
# out of here into endpoints.txt once the pinned spec gains the operation.
CODE_ONLY_OPS = {
    ("PUT", "/orders/{}"),               # order amend  -> amend_order
    ("POST", "/account/leverage"),       # account leverage -> set_leverage
    ("POST", "/account/margin-mode"),    # account margin-mode -> set_margin_mode
    ("GET", "/funding-payments"),        # funding-payments -> fetch_funding_payments
    ("POST", "/transfers"),              # transfers create -> create_transfer
    ("GET", "/transfers"),               # transfers list -> fetch_transfers
    ("POST", "/sub-accounts"),           # sub-accounts create -> create_sub_account
    ("GET", "/sub-accounts"),            # sub-accounts list -> fetch_sub_accounts
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
#   GET  /account/{address}/adl-history, GET /markets/{market_id}/adl-events —
#     ADL history/events; no CLI command yet.
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
            src = open(path).read()
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


def check_code_vs_targets(targeted):
    """Invariant 2: called SDK-method ops == endpoints.txt, modulo the two
    documented allowlists. Returns the number of errors printed."""
    called, _ = called_ops()
    targeted_norm = {(m, normalize_path(p)) for m, p in targeted}

    # (a) called but not listed (and not an intentional code-only op).
    called_missing_from_targets = sorted(called - targeted_norm - CODE_ONLY_OPS)
    # (b) listed but not called (and not an intentional non-REST target).
    targets_without_call = sorted(targeted_norm - called - NON_REST_TARGETS)
    # Bonus: a CODE_ONLY_OPS entry no command calls is stale — catch it.
    stale_code_only = sorted(CODE_ONLY_OPS - called)

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

    if not errors:
        print(
            f"\nOK: the CLI calls {len(called)} mapped SDK op(s); all are in "
            f"endpoints.txt or CODE_ONLY_OPS, and every endpoints.txt entry has a "
            f"calling command or is in NON_REST_TARGETS."
        )
    return errors


def main():
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <openapi.json>")
    with open(sys.argv[1]) as f:
        spec = json.load(f)
    version = spec.get("info", {}).get("version", "?")
    targeted = load_targeted()
    available = spec_ops(spec)

    missing = [op for op in targeted if op not in available]
    uncovered = sorted(available - set(targeted))

    print(f"Spec version: {version}")
    pct = 100.0 * len(targeted) / len(available) if available else 0.0
    print(
        f"CLI targets {len(targeted)} of {len(available)} spec endpoints "
        f"({pct:.1f}% coverage)."
    )

    if uncovered:
        print(f"\nNot covered by the CLI ({len(uncovered)}):")
        for m, p in uncovered:
            print(f"  - {m} {p}")

    failures = 0
    if missing:
        failures += len(missing)
        print(
            f"\nERROR: {len(missing)} targeted endpoint(s) are NOT in the spec "
            f"(removed/renamed/typo):"
        )
        for m, p in missing:
            print(f"  - {m} {p}")
    else:
        print("\nOK: every targeted endpoint exists in the pinned spec.")

    # Invariant 2: CLI code <-> endpoints.txt.
    failures += check_code_vs_targets(targeted)

    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
