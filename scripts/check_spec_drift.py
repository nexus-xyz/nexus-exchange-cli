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
    # NOTE: `set_margin_mode` is deliberately unmapped. The SDK exposes it, but it
    # targets POST /account/margin-mode, which no spec version has ever defined
    # and no service routes. The `account margin-mode` command that called it was
    # withdrawn in ENG-7740; ENG-7614 tracks the engine work that must land first.
    # Do not add a row here without a spec operation to point it at.
    "fetch_funding_payments": ("GET", "/funding-payments"),
    "create_transfer": ("POST", "/transfers"),
    "fetch_transfers": ("GET", "/transfers"),
    "create_sub_account": ("POST", "/sub-accounts"),
    "fetch_sub_accounts": ("GET", "/sub-accounts"),
    "cancel_orders": ("POST", "/orders/batch-cancel"),
    "fetch_order_by_client_id": ("GET", "/orders/by-client-id/{client_order_id}"),
    "cancel_order_by_client_id": ("DELETE", "/orders/by-client-id/{client_order_id}"),
}

# Why a row can be absent from the pinned spec. The distinction is the entire
# point of this table: one of these is a documentation lag, the other is a dead
# end, and the old table could not tell them apart.
#
#   SERVED_UNSPECIFIED — the route works; the spec has not caught up. The command
#                        does what it says. Fix is a spec PR.
#   ROUTE_INVISIBLE    — nothing serves it. The command is a callable dead end: it
#                        parses, authenticates, dispatches, and cannot succeed.
#                        Fix is a product/contract decision, or withdraw the
#                        command (which is what ENG-7740 did to `margin-mode`).
SERVED_UNSPECIFIED = "served-unspecified"
ROUTE_INVISIBLE = "route-invisible"

# Called by a command but intentionally absent from endpoints.txt: adding them
# there would (correctly) fail the endpoints.txt<->spec invariant while the spec
# lacks the op. Move a row into endpoints.txt once the pinned spec gains the
# operation — `check_allowlist_is_honest` FAILS if you don't.
#
# Each row must justify itself: (calling CLI command, why, tracking issue). An
# unattributed row is rejected. That rule exists because
# `("POST", "/account/margin-mode")` sat here for months claiming to be "ahead of
# the pinned spec" when the path had never appeared in ANY spec version and no
# service routed it — the `spec-drift` gate stayed green the whole time, and the
# command shipped as a dead end (ENG-7740). There was no issue to point at,
# because the row was a guess rather than a plan. Naming one is the forcing
# function.
CODE_ONLY_OPS = {
    ("POST", "/account/leverage"): ("account leverage", SERVED_UNSPECIFIED, "ENG-3817"),
    ("GET", "/funding-payments"): ("funding-payments", SERVED_UNSPECIFIED, "ENG-3817"),
    # These four are the same shape margin-mode was: the SDK ships methods, the
    # CLI exposes commands, and neither `/transfers` nor `/sub-accounts` has a
    # served route or a contract (ENG-7800 verifies both against v0.7.2). They are
    # classified honestly here rather than hidden behind "ahead of spec"; ENG-7800
    # owns the withdraw-or-specify decision.
    ("POST", "/transfers"): ("transfers create", ROUTE_INVISIBLE, "ENG-7800"),
    ("GET", "/transfers"): ("transfers list", ROUTE_INVISIBLE, "ENG-7800"),
    ("POST", "/sub-accounts"): ("sub-accounts create", ROUTE_INVISIBLE, "ENG-7800"),
    ("GET", "/sub-accounts"): ("sub-accounts list", ROUTE_INVISIBLE, "ENG-7800"),
    # ENG-5487's three, converted from bare tuples when #46 (ENG-7927) turned this
    # set into an attributed mapping. SERVED_UNSPECIFIED rather than
    # ROUTE_INVISIBLE: unlike /transfers and /sub-accounts, these paths are not
    # absent from the served surface as a category — the spec simply does not name
    # these three operations, while the SDK ships the methods this PR wires up.
    ("POST", "/orders/batch-cancel"): ("order cancel-batch", SERVED_UNSPECIFIED, "ENG-5487"),
    ("GET", "/orders/by-client-id/{}"): ("order get-by-client-id", SERVED_UNSPECIFIED, "ENG-5487"),
    ("DELETE", "/orders/by-client-id/{}"): (
        "order cancel-by-client-id",
        SERVED_UNSPECIFIED,
        "ENG-5487",
    ),
}

# Well-formed tracking reference, e.g. ENG-7800.
_ISSUE_RE = re.compile(r"^ENG-\d+$")

# NOTE (merge, ENG-7927 x ENG-7962): the 'has the spec caught up' half of
# invariant 3 is NOT repeated here. `check_code_vs_targets` already does it, and
# does it better: it compares by PATH under any method, which is what caught
# `PUT /orders/{}` sitting here while the spec had `PATCH`. Checking it a second
# time by exact (method, path) would report the weaker result alongside the
# stronger one. What this file adds is ATTRIBUTION - a row must name the command
# it backs, why the op is absent, and an issue.

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


# The dual-stack v1 prefix (ENG-4949 / ENG-4751). The pinned spec documents many
# operations TWICE — once on the legacy gateway route (`/orders`) and once on the
# direct per-service route (`/api/v1/orders`) — pending the layout decision in
# ENG-8155. METHOD_OP above already picks ONE spelling per operation, mirroring
# what the SDK actually calls, so subtracting path strings reported the spelling
# the CLI did not pick as an uncovered gap and divided by a denominator counting
# 101 paths where the API has 68 operations: coverage read 37.6% when it was
# 55.9%, and 33 of the 63 reported gaps were the other spelling of something the
# CLI already covers (ENG-11847).
V1_PREFIX = "/api/v1"


def canonical_op(op):
    """One (method, path) per operation, whichever spelling the spec used.

    Applied ONLY to the coverage figures. Invariant 1 keeps comparing literal
    paths, because an endpoints.txt entry must name a path the spec really
    declares — and because METHOD_OP's whole point is that the row mirrors the
    spelling the SDK sends, which a canonicalized comparison would stop checking.
    """
    method, path = op
    if path.startswith(V1_PREFIX + "/"):
        path = path[len(V1_PREFIX):]
    return (method, path)


def coverage_figures(targeted, available):
    """Canonical coverage sets, split out of the reporting so they are testable.

    Separate because a report that hides every gap and a report with no gaps
    print the same line — the tests need something to call other than the
    reporting path itself.
    """
    canon = lambda ops: {canonical_op(op) for op in ops}
    spec_set, mine = canon(available), canon(targeted)
    return {
        "spec": spec_set,
        "targeted": mine,
        "covered": spec_set & mine,
        "uncovered": sorted(spec_set - mine),
        "spellings": len(available) - len(spec_set),
    }


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
    # `CODE_ONLY_OPS` keys are normalised once here and used normalised everywhere
    # below. They were previously compared RAW against `called` and `targeted_norm`,
    # which are both normalised — so a row written the natural way
    # (`/orders/{order_id}` rather than `/orders/{}`) could not match anything.
    #
    # Found while writing a test for the caught-up check (@Luc-Campos's LOW 1 on
    # #46): that comparison was one of THREE with the same mismatch, and fixing only
    # it left a test that could not isolate the branch, because the other two still
    # fired. All three misreport in a different direction:
    #
    #   * `called_missing_from_targets` — the row fails to exempt the call, so the op
    #     reads as "called but not in endpoints.txt";
    #   * `stale_code_only` — the row reads as "no command calls this" even when one
    #     does;
    #   * the caught-up check — the row silently opts out of it (LOW 1).
    #
    # It is latent on the real tree only because every committed row happens to be
    # written with bare `{}`, a convention nothing stated and nothing enforced. This
    # makes the convention unnecessary rather than merely documented.
    code_only_norm = {(m, normalize_path(p)) for m, p in CODE_ONLY_OPS}
    # Normalised key -> the raw key, so errors still name the line as written.
    code_only_raw_by_norm = {(m, normalize_path(p)): (m, p) for m, p in CODE_ONLY_OPS}

    called_missing_from_targets = sorted(called - targeted_norm - code_only_norm)
    # (b) listed but not called (and not an intentional non-REST target).
    targets_without_call = sorted(targeted_norm - called - NON_REST_TARGETS)
    # Invariant 3: a CODE_ONLY_OPS entry no command calls is stale. Reported with the
    # raw key so the reader can find the row.
    stale_code_only = sorted(
        code_only_raw_by_norm[op] for op in (code_only_norm - called)
    )
    # Invariant 3: a CODE_ONLY_OPS entry the pinned spec already defines is not
    # "ahead of spec". Compare by PATH (any method) so a wrong verb in METHOD_OP
    # is caught too — that is the failure mode ENG-7962 found. Report the spec's
    # own methods for the path so the fix is obvious from the log.
    spec_methods_by_path = {}
    for m, p in available:
        spec_methods_by_path.setdefault(normalize_path(p), set()).add(m)
    # `normalize_path` on BOTH sides (@Luc-Campos on #46). `spec_methods_by_path`
    # is keyed by the normalized path, so testing the raw allowlist key opted a row
    # out of this check whenever the two spellings differed — `/positions/{market_id}`
    # written the natural way against a spec `/positions/{marketId}` silently missed.
    # It still failed, via `stale_code_only`, but with a message naming the wrong
    # cause: "nothing calls this" rather than "the spec has caught up".
    caught_up_code_only = sorted(
        (m, p, sorted(spec_methods_by_path[normalize_path(p)]))
        for m, p in CODE_ONLY_OPS
        if normalize_path(p) in spec_methods_by_path
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
    the coverage figures and the informational uncovered list. Returns the number
    of errors printed.

    This used to say the coverage line was "the coverage line the dashboard
    scrapes". It is not: the interfaces fleet dashboard counts non-comment lines
    of endpoints.txt (the numerator only), and no workflow reads this stdout. The
    claim mattered because it is the kind of thing that stops someone making a
    correct change to the wording — as it nearly stopped this one (ENG-11847).
    """
    missing = [op for op in targeted if op not in available]

    cov = coverage_figures(targeted, available)
    uncovered = cov["uncovered"]
    pct = 100.0 * len(cov["covered"]) / len(cov["spec"]) if cov["spec"] else 0.0
    print(
        f"CLI covers {len(cov['covered'])} of {len(cov['spec'])} spec operations "
        f"({pct:.1f}% coverage) — {len(available)} documented paths, "
        f"{cov['spellings']} of them a second spelling of an operation already "
        f"counted."
    )

    if uncovered:
        print(f"\nNot covered by the CLI ({len(uncovered)}):")
        for m, p in uncovered:
            print(f"  - {m} {p}")
    else:
        print("\nOK: every spec operation is covered by the CLI.")

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


def check_allowlist_is_honest():
    """Invariant 3: every CODE_ONLY_OPS row's claim is true and attributed.

    `CODE_ONLY_OPS` is the escape hatch that lets a command ship without a spec
    operation, so it is the one place a false claim goes unnoticed. Two ways a row
    can lie. This function owns one of them; `check_code_vs_targets` owns the other.

      (a) HERE — the row is unattributed: no calling command, no reason, or no
          tracking issue. The withdrawn margin-mode op was exactly this: a bare
          entry with a comment asserting "ahead of the pinned spec", which was
          never true (ENG-7740). The path is spelled out in the `#` comments
          above rather than here, because `coverage.rs`'s
          `margin_mode_is_absent_from_every_drift_artifact` strips `#` lines
          before scanning and a docstring is not stripped — naming it here would
          trip a guard that exists to keep it out of the tables.
      (b) NOT here — the pinned spec DOES contain the op, so the row's premise has
          expired. `check_code_vs_targets` checks that by PATH under any method
          (ENG-7962), which is strictly stronger than the by-(method, path) check
          this function originally carried, so it is not duplicated.

    Also prints the ROUTE_INVISIBLE rows as a standing report: those are commands
    with nothing behind them, and they should be visible in every run rather than
    buried in a Python literal.

    Returns the number of errors printed.
    """
    errors = 0

    malformed = []
    for op, row in sorted(CODE_ONLY_OPS.items()):
        if not (isinstance(row, tuple) and len(row) == 3):
            malformed.append((op, "expected a (command, kind, issue) triple"))
            continue
        command, kind, issue = row
        if not command or not isinstance(command, str):
            malformed.append((op, "no calling CLI command named"))
        elif kind not in (SERVED_UNSPECIFIED, ROUTE_INVISIBLE):
            malformed.append(
                (op, f"kind must be {SERVED_UNSPECIFIED!r} or {ROUTE_INVISIBLE!r}, got {kind!r}")
            )
        elif not _ISSUE_RE.match(issue or ""):
            malformed.append((op, f"tracking issue must look like ENG-1234, got {issue!r}"))

    if malformed:
        errors += len(malformed)
        print(
            f"\nERROR: {len(malformed)} CODE_ONLY_OPS row(s) are unattributed. A row "
            f"lets a command ship with no spec operation, so it must name the "
            f"command it backs, why the op is absent, and the issue that resolves it:"
        )
        for (m, p), why in malformed:
            print(f"  - {m} {p}: {why}")

    invisible = sorted(
        (op, row) for op, row in CODE_ONLY_OPS.items()
        if isinstance(row, tuple) and len(row) == 3 and row[1] == ROUTE_INVISIBLE
    )
    if invisible:
        print(
            f"\nNOTE: {len(invisible)} command(s) target a route nothing serves — they "
            f"parse, authenticate, dispatch, and cannot succeed. Withdraw or specify "
            f"(this is the ENG-7740 shape):"
        )
        for (m, p), (command, _, issue) in invisible:
            print(f"  - `nexus {command}` -> {m} {p}  ({issue})")

    if not errors:
        # Only the half this function owns. The "none is in the pinned spec" clause
        # moved to `check_code_vs_targets` (by path, under any method — ENG-7962),
        # so asserting it here printed an OK next to that function's ERROR when the
        # caught-up check fired. A function about honesty should not claim a result
        # it did not compute (@Luc-Campos on #46).
        print(
            f"\nOK: all {len(CODE_ONLY_OPS)} CODE_ONLY_OPS row(s) are attributed."
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

    print(f"Spec version: {version}")

    # Invariant 1: endpoints.txt <-> spec.
    failures = check_targets_vs_spec(targeted, available)
    # Invariants 2-4: CLI code <-> endpoints.txt, allowlist hygiene, and
    # CLI_SOURCES completeness.
    failures += check_code_vs_targets(targeted, available)
    # Invariant 3, attribution half (ENG-7927): every CODE_ONLY_OPS row names the
    # command it backs, why the op is absent, and a tracking issue.
    failures += check_allowlist_is_honest()

    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
