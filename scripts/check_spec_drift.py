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

Six invariants are enforced here: 1-4 and 9 against the spec and the CLI's own
source, and 7 against the README's own coverage sentence. The numbering is
fleet-wide, not per-file — 5, 6 and 8 belong to `check_sdk_parity.py`, which owns
the crate side — so a number means the same thing in the README, in CI logs and in
both checkers.

1 and 2 are the contract; 3, 4 and 9 exist because it can be silently defeated —
an allowlist entry that stops being earned, a command handler in a file the parser
never reads, or a call the parser cannot recognise — and a defeated check still
prints OK. `scripts/test_check_spec_drift.py` defeats each one and asserts it goes
red.

1. endpoints.txt <-> spec
   Every endpoint the CLI targets (endpoints.txt) must exist in the pinned
   OpenAPI spec (.api-version). A miss means a breaking change, rename, or typo
   in the spec. Spec operations the CLI does not cover are reported as an
   informational coverage gap, and the coverage % is printed for humans — the
   dashboard does NOT read this line, it recomputes the ratio itself from
   `endpoints.txt` (see `coverage()`).

2. CLI code <-> endpoints.txt
   The set of SDK methods the CLI actually calls (parsed from the source files in
   CLI_SOURCES and mapped through METHOD_OP) must EQUAL the endpoints.txt set —
   real set equality in both directions, not a subset check — modulo ONE
   explicit, documented allowlist:

     * NON_REST_TARGETS — listed in endpoints.txt but reached WITHOUT a named
                          REST method call (e.g. the WebSocket upgrade GET /ws,
                          opened by the streaming client).

   The check fails if (a) the CLI calls a mapped method whose op is not in
   endpoints.txt, or (b) endpoints.txt lists an op that no called method maps to
   and that is not in NON_REST_TARGETS.

   There is deliberately NO allowlist in the (a) direction any more. CODE_ONLY_OPS
   was one: an op could be implemented and kept out of endpoints.txt on the claim
   that the pinned spec would catch up. The claim was never verified and did not
   hold — re-checked against the pinned spec (v0.8.1, also the latest release), not
   one of the nine parked rows was defined (leverage, funding payments, batch
   cancel, the two client-order-id ops, transfers, sub-accounts), and several route
   nowhere on the venue either (ENG-7318, ENG-7800). The CLI shipped nine commands
   a user could read in `--help` and could not run, and the allowlist is precisely
   what kept this gate green over them.

   Policy (ENG-8616, fleet-wide): an operation the contract does not define must
   not be implemented. No attribution, no parking, no release-lag exception. When a
   published spec version defines the op, implement it against that version and
   list it in endpoints.txt. CODE_ONLY_OPS survives only as an EMPTY, SEALED set
   whose invariant is its own emptiness — ANY entry fails the check — so re-opening
   the hatch has to be a deliberate, reviewed change to this file rather than a
   one-line addition to a table.

3. Allowlist hygiene — an exemption must stay earned (ENG-7962)
   NON_REST_TARGETS suppresses a real invariant, so a stale entry is a silent hole
   rather than a visible one: an entry endpoints.txt does not list exempts nothing
   and is a stale grant waiting to hide a regression. That check is what ENG-7962
   generalised from — `amend_order` was mapped to `PUT /orders/{order_id}` while the
   SDK issues `PATCH`, and `PATCH /orders/{order_id}` had been in the spec since
   v0.7.1, so a covered operation sat exempted and uncounted under the old
   CODE_ONLY_OPS. Sealing that table empty removes the class outright.

4. CLI_SOURCES completeness
   No .rs file OUTSIDE CLI_SOURCES may contain a mapped SDK method call — anywhere
   under src/, at any depth. The parser only reads the listed files, so moving a
   command handler into a new module would otherwise silently under-count the CLI's
   targeted ops and read as green. Fail instead, and name the file to add.

9. METHOD_OP completeness — the parser must not generate its own blind spot
   Every `client.<method>()` call in CLI_SOURCES must have a METHOD_OP row.

   `_CALL_RE` is BUILT FROM METHOD_OP's own keys, so the parser can only see calls
   the table already knows about: an unmapped call matches nothing, contributes
   nothing to `called_ops`, and every invariant keyed on that set stays green. The
   table under validation was also generating its own validator.

   Not hypothetical. `sign_in` and `register_agent` had no row, so
   `POST /auth/login` and `POST /agents/register` — called by `auth login` and
   `agents register`, and present in the pinned spec since long before it was
   pinned — were absent from endpoints.txt and printed in the `Not covered by the
   CLI` list on every run for the whole life of this check (ENG-12786). The
   comment that explained them away named ENG-4046 as "a separate in-flight PR";
   it had merged one PR before this script was written.

   Invariant 4 governs WHERE the parser looks; this governs WHAT it recognises.
   Note the direction — an unmapped METHOD is fine (the withdrawn margin-mode
   setter is deliberately unmapped, and no command calls it), an unmapped CALL is
   not. This is a check on the call sites, not on the SDK's surface.

7. The README's coverage sentence is still true
   The ratio is printed by this script and *committed* in README.md, and for two
   spec releases nothing compared the two: the sentence claimed `38 of 98 (38.8%)`
   against `v0.7.2` while the pin was `v0.8.1`, and every run stayed green.
   Checked against what this run computed, and repairable with --sync-coverage so
   the bot that moves the pin can move the sentence with it.

Usage:
  check_spec_drift.py <openapi.json>                  # check
  check_spec_drift.py --sync-coverage <openapi.json>  # rewrite the README claim
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
# a new SDK method, together with the matching endpoints.txt line. There is no
# longer an "ahead of the pinned spec" bucket to put it in: if the pinned spec
# does not define the operation, the command does not ship (ENG-8616).
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
    # wallet-signed auth. Unauthenticated signed requests rather than HMAC ones,
    # which is why they were once described as outside this table's scope — but
    # they are ordinary spec operations the CLI calls, and leaving them unmapped
    # made them invisible to every invariant here (ENG-12786, invariant 9).
    "sign_in": ("POST", "/auth/login"),  # no /api/v1 variant yet
    "register_agent": ("POST", "/agents/register"),  # no /api/v1 variant yet
    # websocket
    "mint_web_socket_token": ("POST", "/ws/token"),  # no /api/v1 variant yet
    # NOTE: ten SDK methods are deliberately unmapped, because no command calls
    # them any more. Each targeted an operation no spec version has ever defined:
    #
    #   set_margin_mode              POST /account/margin-mode  (ENG-7740, ENG-7614)
    #   set_leverage                 POST /account/leverage     (ENG-7318)
    #   fetch_funding_payments       GET  /funding-payments     (ENG-3817)
    #   create_transfer              POST /transfers            (ENG-7800)
    #   fetch_transfers              GET  /transfers            (ENG-7800)
    #   create_sub_account           POST /sub-accounts         (ENG-7800)
    #   fetch_sub_accounts           GET  /sub-accounts         (ENG-7800)
    #   cancel_orders                POST /orders/batch-cancel  (ENG-5487)
    #   fetch_order_by_client_id     GET  /orders/by-client-id/{}  (ENG-5487)
    #   cancel_order_by_client_id    DELETE /orders/by-client-id/{}  (ENG-5487)
    #
    # `margin-mode` was withdrawn first (ENG-7740); the other nine followed in
    # ENG-12369 under the fleet policy in ENG-8616. nexus-exchange-rs deleted all
    # ten client methods in its PR #143, so a row here would name a method that
    # will not exist once the CLI bumps past 0.9.1. Do not add a row without a
    # PUBLISHED spec operation to point it at — that is the whole rule.
}

# The sealed hatch (ENG-8616 / ENG-12369).
#
# CODE_ONLY_OPS used to hold ops a command called while the pinned spec did not
# define them, on the claim that the spec would catch up. Nothing verified the
# claim, and it did not hold: `("POST", "/account/margin-mode")` sat here for
# months asserting "ahead of the pinned spec" when the path had never appeared in
# ANY spec version and no service routed it, and the gate stayed green while the
# command shipped as a dead end (ENG-7740). Attribution was added to force each row
# to name a command and an issue (ENG-7927); it made the rows honest without making
# them true, and nine survived it — a ticket reference beside a phantom op only
# makes it look sanctioned.
#
# So the table is now EMPTY and SEALED: its only invariant is its own emptiness,
# and ANY entry fails the check. It is kept as a named concept, rather than
# deleted, so that re-opening the hatch is a deliberate, reviewed edit to this file
# and its test — not a one-line addition to a list that reviewers read as routine.
#
# An operation the pinned spec does not define is not implemented. When a PUBLISHED
# spec version defines it, add the METHOD_OP row and the endpoints.txt line
# together, and the op is covered rather than exempted.
CODE_ONLY_OPS = set()

# NOTE (ENG-12369): the two rot checks this table used to carry — "no command calls
# this row any more" and "the pinned spec has caught up with it" — are gone with the
# rows. Both could only fire on a NON-empty table, and emptiness implies both. They
# were also the whole reason a phantom op could sit here indefinitely: an op that
# never appears in any spec version, and never will, satisfies neither and reads
# green forever (ENG-7740 sat that way for months). Sealing is strictly stronger
# than policing, so nothing weaker is repeated here.

# Listed in endpoints.txt but reached WITHOUT a named SDK REST method call, so the
# source parser cannot (and should not) see it. The WebSocket upgrade is opened by
# the streaming client (ws_client.connect(...) in src/wsclient.rs), not a
# `client.<method>()` call. Paths use the normalized `{}` placeholder form.
NON_REST_TARGETS = {
    ("GET", "/ws"),
}

# Spec operations that exist but the CLI deliberately does not target. Documented
# here so the exclusion is intentional, not an oversight:
#   PUT/GET/DELETE /admin/tiers* — admin-only tier management; out of CLI scope.
#   POST /ws-tokens — deprecated; superseded by POST /ws/token (which we use).
#   GET  /stream — deprecated SSE stream; superseded by the /ws upgrade.


def normalize_path(p):
    """Collapse any `{placeholder}` segment to a bare `{}` so paths match by
    position, not placeholder name."""
    return re.sub(r"\{[^}]*\}", "{}", p)


# The dual-stack prefix. The spec mounts most operations twice — `GET /account`
# and `GET /api/v1/account` are one operation at two mounts — and the CLI, like
# every other client surface, targets exactly one mount per operation.
API_V1_PREFIX = "/api/v1"


def canonical_op(op):
    """Canonical form of a `(method, path)` op, collapsing the dual-stack `/api/v1`
    twin so the coverage ratio can reach 100%.

    A literal count puts both mounts in the denominator while the numerator can
    only ever hold one, so a surface targeting every operation perfectly still
    scored well under 100% and the tile could never read full (ENG-10035). This
    repo had exactly that: `38 of 101 (37.6%)`, where 101 is path-ops, not
    operations.

    **Never use this to decide whether an operation EXISTS.** The twin is not
    universal — the bridge domain is `/api/v1`-native and has no host-root mount at
    all — so collapsing invents bare labels the spec never documents. Crediting one
    would credit a route that 404s, which is ENG-8463. Collapse ONLY to key the
    ratio; match literally for existence, and report uncovered operations as their
    literal mounts.

    Ported deliberately, not invented: the source of truth is
    `normalise_op` in the monorepo's `.github/scripts/collect-interfaces-metrics.py`
    (and its sibling `eng/ops/intelligence/interfaces/capability_coverage.py`),
    which is what actually computes the interfaces dashboard from this repo's
    `endpoints.txt`. Two collectors in one tree with opposite conventions is the
    root cause ENG-10035 documents, so this agrees with them by construction:
    `/api/v1` is stripped only as a **prefix** (a bare `/api/v1` path is left
    alone), and the method is upper-cased.
    """
    method, path = op
    if path.startswith(API_V1_PREFIX + "/"):
        path = path[len(API_V1_PREFIX) :]
    return (method.upper(), path)


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


# The collector's set, verbatim (`collect-interfaces-metrics.py:103`). `head` and
# `options` are in it and were missing here: v0.8.1 documents neither, so the two
# denominators agreed by luck rather than by construction. The first HEAD or
# OPTIONS operation would move the dashboard's denominator while invariant 7 pinned
# the README to this one — the same latent divergence the `deprecated` split was
# ported to prevent, and not something a reader would think to check.
HTTP_METHODS = ("get", "post", "put", "patch", "delete", "head", "options")


def spec_ops(spec):
    """Every operation the spec documents, deprecated ones INCLUDED.

    This is the existence set, so it must stay literal and complete: a deprecated
    operation is still mounted and still served, and a targeted line naming one is
    not a drift error. `deprecated_ops` splits them out for the *ratio* only.
    """
    ops = set()
    for p, methods in (spec.get("paths") or {}).items():
        # `isinstance` on the PATH ITEM as well as the operation, and `or {}` above:
        # both are the collector's shape (`_spec_ops`), and neither was ported. A
        # non-object path item (a `$ref` string, a null from a hand-edit) turned the
        # run into an AttributeError traceback where the bare loop was harmless —
        # a checker that crashes on malformed input reports nothing about the input
        # it was pointed at.
        if not isinstance(methods, dict):
            continue
        for m, op in methods.items():
            if m.lower() in HTTP_METHODS and isinstance(op, dict):
                ops.add((m.upper(), p))
    return ops


def deprecated_ops(spec):
    """The operations the spec marks `deprecated: true`.

    Ported from the collector for the same reason `canonical_op` is: the monorepo's
    `collect-interfaces-metrics.py` splits deprecated operations out of its
    denominator (`_spec_ops`, and `spec_ops_at`/`fetch_spec` return only the live
    set), so counting them here would make the README's "the dashboard computes the
    same ratio, under the same rule" claim false the moment the spec deprecates
    anything. Both surfaces read 68 at v0.8.1 only because it deprecates nothing
    (verified); the legacy gateway mounts going deprecated is the stated ENG-4740
    direction, which is exactly when an unported filter would start lying.

    An operation the CLI targets that is deprecated is not a failure — invariant 1
    matches against the full set — but it IS reported, because it is a wrapper the
    CLI will lose.
    """
    out = set()
    for p, methods in (spec.get("paths") or {}).items():
        if not isinstance(methods, dict):
            continue
        for m, op in methods.items():
            if m.lower() not in HTTP_METHODS or not isinstance(op, dict):
                continue
            if op.get("deprecated"):
                out.add((m.upper(), p))
    return out


# Match `client.<method>(` or `<receiver>.<method>(` where <method> is one of the
# mapped SDK methods. We anchor on the method name from METHOD_OP rather than the
# receiver, so a renamed binding (e.g. `ws_client`) is still seen.
_CALL_RE = re.compile(
    r"\.(" + "|".join(sorted(METHOD_OP, key=len, reverse=True)) + r")\s*\("
)

# Invariant 9's parser, and deliberately NOT built from METHOD_OP — that is the
# entire point. `_CALL_RE` above can only match methods the table already lists,
# so it is structurally unable to report a call the table is missing; a second
# regex that does not consult the table is the only way to see one.
#
# Anchored on the receiver instead of the method name, since the method name is
# exactly what is unknown here. `\w*client` covers the two bindings that exist
# (`client` in main.rs, `ws_client` in wsclient.rs) plus a field access
# (`self.client`), and rustfmt's habit of breaking the receiver onto its own line
# is why the separator is `\s*` rather than nothing.
#
# The limit, stated rather than papered over: a receiver whose name does not
# contain "client" (`c.clone().sign_in()`) is not seen. That is under-detection of
# the same kind the check has today, not a new hole, and no such binding exists in
# either source. `_CALL_RE` stays receiver-agnostic and keeps counting ops
# regardless of the binding — the two regexes answer different questions.
_CLIENT_CALL_RE = re.compile(r"\b\w*client\s*\.\s*([A-Za-z_]\w*)\s*\(")

# Methods on the SDK client that are NOT REST operations, so invariant 9 must not
# demand a METHOD_OP row for them. One entry, and it is earned: `connect` opens the
# WebSocket upgrade, which endpoints.txt reaches through NON_REST_TARGETS
# (`GET /ws`) precisely because there is no named REST method behind it.
#
# Kept honest the way NON_REST_TARGETS is (invariant 3) — an entry nothing calls is
# stale and fails, so this cannot quietly become a parking space for a method
# someone did not want to map. It is the only allowlist here that still GRANTS
# anything: CODE_ONLY_OPS is sealed empty (ENG-8616), so its rule is emptiness
# rather than staleness, and re-opening it is a reviewed change to this file.
NON_OP_CLIENT_METHODS = {"connect"}


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


def client_calls(sources=CLI_SOURCES):
    """Every `<...>client.<method>(` call in `sources`, as {method: [file, ...]}.

    Independent of METHOD_OP by construction — see `_CLIENT_CALL_RE`."""
    found = {}
    for path in sources:
        try:
            with open(path) as f:
                src = f.read()
        except OSError as e:
            sys.exit(f"ERROR: cannot read CLI source {path!r}: {e}")
        for m in _CLIENT_CALL_RE.finditer(src):
            found.setdefault(m.group(1), set()).add(os.path.relpath(path, REPO))
    return {k: sorted(v) for k, v in found.items()}


def check_method_op_complete(sources=None):
    """Invariant 9: every SDK method the CLI calls has a METHOD_OP row.

    Returns the number of errors printed."""
    calls = client_calls(sources if sources is not None else CLI_SOURCES)
    if not calls:
        sys.exit(
            "ERROR: parsed zero `client.<method>()` calls from the CLI sources; the "
            "call pattern or the client binding may have changed — update "
            "_CLIENT_CALL_RE."
        )

    errors = 0

    unmapped = sorted(
        (m, files) for m, files in calls.items()
        if m not in METHOD_OP and m not in NON_OP_CLIENT_METHODS
    )
    if unmapped:
        errors += len(unmapped)
        print(
            f"\nERROR: {len(unmapped)} SDK method(s) the CLI calls have no METHOD_OP "
            f"row, so the drift parser cannot see them. Every invariant here is "
            f"computed from the calls it recognises, which means an unmapped call is "
            f"not merely uncounted — it is unchecked:"
        )
        for method, files in unmapped:
            print(f"  - {method}  ({', '.join(files)})")
        print(
            "  Add a METHOD_OP row naming the spec operation it issues (and the "
            "matching endpoints.txt line), or, if it issues no REST request, add it "
            "to NON_OP_CLIENT_METHODS saying what it does instead."
        )

    stale = sorted(NON_OP_CLIENT_METHODS - set(calls))
    if stale:
        errors += len(stale)
        print(
            f"\nERROR: {len(stale)} NON_OP_CLIENT_METHODS entr(ies) are not called by "
            f"any command — exempting a call that is not there:"
        )
        for method in stale:
            print(f"  - {method}")

    if not errors:
        print(
            f"OK: all {len(calls)} SDK method(s) the CLI calls are mapped in "
            f"METHOD_OP ({len(NON_OP_CLIENT_METHODS)} non-REST, exempted)."
        )
    return errors


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
    """Invariants 2-4: called SDK-method ops == endpoints.txt (modulo the one
    documented allowlist, NON_REST_TARGETS — CODE_ONLY_OPS is sealed empty and
    suppresses nothing), that allowlist is still earned, and no unscanned source
    file reaches the API. Returns the number of errors printed.

    `sources` / `src_dir` default to the real CLI_SOURCES / SRC_DIR; the self-test
    overrides them with synthetic Rust so each invariant can be defeated in
    isolation."""
    sources = CLI_SOURCES if sources is None else sources
    src_dir = SRC_DIR if src_dir is None else src_dir
    called, _ = called_ops(sources)
    targeted_norm = {(m, normalize_path(p)) for m, p in targeted}

    # (a) called but not listed. No allowlist stands in this direction any more:
    # CODE_ONLY_OPS is sealed empty (ENG-8616), so an op the CLI calls and
    # endpoints.txt does not carry is a failure, full stop. The suppression that
    # used to live here — subtracting the allowlist from this set — is what kept
    # nine unbacked commands green.
    called_missing_from_targets = sorted(called - targeted_norm)
    # (b) listed but not called (and not an intentional non-REST target).
    targets_without_call = sorted(targeted_norm - called - NON_REST_TARGETS)
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
            f"NOT in endpoints.txt. Add the line if the PINNED spec defines the "
            f"operation; otherwise delete the command — an op the contract does not "
            f"define must not be implemented (ENG-8616):"
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
            f"endpoints.txt, and every endpoints.txt entry has a calling command "
            f"or is in NON_REST_TARGETS."
        )
        print(
            f"OK: NON_REST_TARGETS is still earned ({len(NON_REST_TARGETS)} "
            f"non-REST), and no source file outside CLI_SOURCES reaches the API."
        )
    return errors


def spec_misses(targeted, available):
    """The targeted ops the spec does not document, matched LITERALLY.

    Called by invariant 1, whose returned failure count is what `main` reads to
    decide whether invariant 7 can say anything meaningful (see its call site).
    """
    return [op for op in targeted if op not in available]


def coverage(targeted, available, deprecated=frozenset()):
    """The ratio, computed once and read by both invariant 1 and invariant 7.

    Returns `(num, den, pct, uncovered)`, where `pct` is the already-rounded string
    both callers print, and `uncovered` maps each uncovered canonical operation to
    its sorted literal mounts.

    Deliberately one function. The two callers used to compute this inline from the
    same inputs, which is a divergence waiting to happen: invariant 7 asserts the
    README against a number, and if its copy of the arithmetic ever drifted from
    invariant 1's, the guard would enforce the wrong figure while printing the
    right one.

    Two rules are load-bearing here, both carried over from the collector:

    * The ratio is keyed on CANONICAL operations, so the 100% ceiling is reachable
      (ENG-10035). Counting mounts put 101 path-ops in a denominator whose
      numerator could only ever hold 68.

    * The numerator intersects LITERALLY first and collapses only after. The order
      is the care point, not a style choice: `{canonical_op(o) for o in targeted} &
      canon_available` reads equivalent but is not, because collapsing is not
      injective over the mounts the spec documents. A phantom `/api/v1` twin of a
      host-root-only operation would fold onto the real one and be credited for a
      route that 404s (ENG-8463). Collapsing after the literal intersection keeps
      the rule ENG-10035 states: an operation counts as covered when the CLI
      targets a mount that literally exists.

    Deprecated operations leave both sides (see `deprecated_ops`), so they can
    neither inflate the denominator nor be credited.
    """
    live = set(available) - set(deprecated)
    canon_available = {canonical_op(op) for op in live}
    canon_targeted = {canonical_op(op) for op in set(targeted) & live}

    num, den = len(canon_targeted), len(canon_available)
    pct = f"{100.0 * num / den:.1f}" if den else "0.0"

    # Keyed by canonical label so a covered operation cannot reappear under its
    # twin, but the mounts are kept LITERAL: the canonical label may name a mount
    # the spec never documents, and a reader who acts on one ships a method that
    # 404s.
    uncovered = {}
    for op in sorted(live):
        canon = canonical_op(op)
        if canon not in canon_targeted:
            uncovered.setdefault(canon, []).append(op)

    return num, den, pct, uncovered


def check_targets_vs_spec(targeted, available, deprecated=frozenset()):
    """Invariant 1: every endpoints.txt op exists in the pinned spec. Also prints
    the human-facing coverage line and the informational uncovered list.

    The coverage line is NOT scraped by anything. The interfaces dashboard reads
    `endpoints.txt` and the spec directly and recomputes the ratio in the monorepo's
    `collect-interfaces-metrics.py`; this line is for the reader of a CI log and the
    step summary. What IS load-bearing is that it agrees with the README, which
    invariant 7 enforces — off the same `coverage()` call, so the two cannot drift.

    Returns the number of errors printed.
    """
    # Existence stays LITERAL, and over the FULL set including deprecated ops. A
    # targeted op counts as real only if that exact mount is in the spec —
    # collapsing here would let a `/api/v1/X` documented only as `/X` pass as
    # present while 404ing in production (ENG-8463) — and a deprecated operation is
    # still served, so dropping it here would report a live route as removed.
    missing = spec_misses(targeted, available)

    num, den, pct, uncovered = coverage(targeted, available, deprecated)
    print(
        f"CLI targets {num} of {den} spec "
        f"operations ({pct}% coverage), "
        f"from {len(available)} path-ops with dual-stack mounts collapsed "
        f"(ENG-10035)."
    )

    if deprecated:
        # Out of both sides of the ratio, so say so rather than leaving a reader to
        # wonder why the path-op count and the denominator moved apart.
        print(
            f"  ({len(deprecated)} deprecated path-op(s) excluded from both sides, "
            f"matching the dashboard collector.)"
        )
        hit = sorted(set(targeted) & set(deprecated))
        if hit:
            print(
                f"\nNOTE: {len(hit)} targeted operation(s) are DEPRECATED in the "
                f"pinned spec. Not a failure — they are still mounted — but the CLI "
                f"will lose them, and they no longer count toward coverage:"
            )
            for m, path in hit:
                print(f"  - {m} {path}")

    if uncovered:
        print(f"\nNot covered by the CLI ({len(uncovered)}):")
        for canon in sorted(uncovered):
            mounts = ", ".join(f"{m} {p}" for m, p in uncovered[canon])
            print(f"  - {mounts}")

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


def check_allowlist_is_sealed():
    """Invariant 3, the hatch half: CODE_ONLY_OPS must be EMPTY.

    This function used to validate attribution — every row naming the command it
    backs, why the op was absent, and a tracking issue (ENG-7927). That made the
    rows honest without making them true: nine attributed rows still described
    operations no spec version had ever defined, and a ticket reference beside a
    phantom op only makes it look sanctioned. So the table is sealed instead of
    policed, and the assertion is its emptiness (ENG-8616 / ENG-12369).

    ANY entry fails, whether or not the pinned spec defines the op. That is
    deliberate and is the difference from every earlier version of this check: the
    old one passed precisely when the op was absent from the spec, which is the
    case that shipped nine dead-end commands. Adding an entry now means editing
    this function and its test too — which is the point, because that is a change a
    reviewer reads as a policy decision rather than as list maintenance.

    Returns the number of errors printed.
    """
    if not CODE_ONLY_OPS:
        print("\nOK: CODE_ONLY_OPS is empty and sealed.")
        return 0

    print(
        f"\nERROR: CODE_ONLY_OPS holds {len(CODE_ONLY_OPS)} entr(ies) and must be "
        f"EMPTY (ENG-8616): an operation the pinned spec does not define must not "
        f"be implemented. Delete the command and its METHOD_OP row — or, once a "
        f"PUBLISHED spec version defines the operation, target that version and "
        f"list the op in endpoints.txt. Operations are not parked here:"
    )
    for op in sorted(CODE_ONLY_OPS):
        method, path = op if isinstance(op, tuple) else (op, "")
        print(f"  - {method} {path}".rstrip())
    return len(CODE_ONLY_OPS)


# The README sentence this script is allowed to contradict nobody about. Kept as
# one regex so a reworded paragraph fails loudly rather than silently unchecking
# itself: a guard that stops matching is a guard that stops guarding.
#
# Whitespace-tolerant between every token, not literal spaces. The README wraps at
# ~80 columns and this sentence's numbers already straddle a line break, so editing
# any earlier word in the paragraph reflows it. With fixed spacing that reflow
# failed the guard as "no parseable coverage claim" — which reads as stale numbers
# and sends the reader to check arithmetic that was never wrong.
#
# `_WS` is one run of whitespace containing AT MOST ONE line break, not `\s+`.
# Markdown treats a single newline inside a paragraph as a space, so tolerating it
# is the correct reading of the source — but a BLANK line ends the paragraph, and a
# sentence split across two paragraphs is a rewording, which this guard exists to
# catch. `\s+` would have quietly matched across it.
#
# The break is `\r?\n`, not `\n`. The repo has no `.gitattributes` and ships Windows
# installers, so a contributor with `core.autocrlf=true` gets a CRLF working copy;
# with a bare `\n` this guard failed there with "no parseable coverage claim" — a
# message that sends them to audit numbers that were never wrong, on a file they
# never edited. The claim is read and written with `newline=""` for the same
# reason: universal-newline mode would translate the whole file's endings on the
# first repair, turning a four-value edit into a whole-file diff.
_WS = r"(?=\s)[ \t]*\r?\n?[ \t]*"
README_COVERAGE_RE = re.compile(
    r"\*\*(?P<num>\d+)" + _WS + r"of" + _WS + r"(?P<den>\d+)\*\*" + _WS
    + r"spec" + _WS + r"operations" + _WS
    + r"\(\*\*(?P<pct>[\d.]+)%\*\*\)," + _WS + r"measured" + _WS + r"against"
    + _WS + r"the" + _WS + r"pinned" + _WS + r"`(?P<tag>v[\d.]+)`" + _WS + r"spec"
)


def _splice_groups(m, values):
    """Return `m`'s matched text with each named group replaced by `values[name]`.

    Group-wise, not a re-render of the sentence: the surrounding prose keeps its
    exact wrapping and wording, so `--sync-coverage` can only ever change the four
    values the guard reads. A writer that rebuilt the sentence would silently
    reformat human-owned text and undo any rewording someone had made deliberately.
    """
    text = m.group(0)
    base = m.start()
    for name in sorted(values, key=lambda n: m.start(n), reverse=True):
        start, end = m.start(name) - base, m.end(name) - base
        text = text[:start] + values[name] + text[end:]
    return text


def _same_release(spec_version, pinned_tag):
    """Whether a spec document's `info.version` is the release `.api-version` pins.

    Compared with a leading `v` stripped from either side: the pin is a git tag
    (`v0.8.1`) and `info.version` is a bare semver (`0.8.1`), and they name the same
    release. Returns False for a missing or unparseable version rather than
    guessing — see the caller.
    """
    if not spec_version or not pinned_tag:
        return False
    return str(spec_version).lstrip("v").strip() == str(pinned_tag).lstrip("v").strip()


def check_readme_coverage_claim(targeted, available, readme="README.md",
                                api_version_file=".api-version",
                                deprecated=frozenset(), write=False,
                                spec_version=None):
    """Invariant 7: the README's coverage sentence matches what this run computed.

    Added because the numbers went stale in silence for two spec releases. The
    README claimed `38 of 98 (38.8%)` against `v0.7.2` while the pin was `v0.8.1`
    and the real ratio was different again — and every check stayed green, because
    the checker *printed* a coverage number and nothing compared it to the one
    committed next to it. AGENTS.md already promised this guard existed; this is it.

    Fails on a claim that is wrong **or** absent. An unparseable paragraph is a
    failure, not a skip: silently passing when the sentence has been reworded is
    the exact hole being closed.

    With `write=True` (`--sync-coverage`) it repairs the sentence instead of failing
    on it. That mode exists because this guard otherwise reds a PR nobody can fix:
    `sdk-autobump.yml` moves `.api-version` and the managed block, and it cannot
    know the new coverage numbers, so every spec-pin bump would land with a red
    invariant 7 and an auto-merge that can never complete. The bot now runs the
    repair and commits the result. An unparseable sentence is still a hard failure
    here — a writer that cannot find its target must not invent one.
    """
    try:
        with open(api_version_file) as f:
            pinned = f.read().strip()
    except OSError as e:
        print(f"\nERROR: cannot read {api_version_file}: {e}")
        return 1

    # The tag in the sentence comes from `.api-version`; the ratio comes from
    # whatever spec document this run was handed. Nothing tied the two together, so
    # a spec for a DIFFERENT release produced a sentence pairing this pin with that
    # release's numbers — in check mode a false "claim is stale" whose suggested
    # remedy corrupts the sentence, and under `--sync-coverage` a false claim
    # written by the tool, at exit 0.
    #
    # Not a contrived case: `openapi.pinned.json` is gitignored and the README tells
    # you to reuse that filename, so a stale local copy is what a contributor will
    # have. Refuse in BOTH modes — a writer that cannot prove its inputs correspond
    # must not write.
    if not _same_release(spec_version, pinned):
        shown = spec_version if spec_version else "not declared"
        wrote = " (and write it into the README)" if write else ""
        print(
            f"\nERROR: the spec passed to this run is {shown}, but {api_version_file} "
            f"pins {pinned}. The coverage ratio is computed from the spec document "
            f"while the tag in {readme} comes from the pin, so pairing them would "
            f"state a ratio for one release against the name of another{wrote}. "
            f"Fetch the pinned spec and re-run:\n"
            f"  curl -fsSL -o openapi.pinned.json \\\n"
            f"    https://raw.githubusercontent.com/nexus-xyz/nexus-exchange-api/"
            f"{pinned}/openapi.json"
        )
        return 1

    try:
        # `newline=""`: read the file's own line endings verbatim rather than
        # translating them, so the repair below rewrites four values and nothing
        # else. See the note on `_WS`.
        with open(readme, newline="") as f:
            text = f.read()
    except OSError as e:
        print(f"\nERROR: cannot read {readme}: {e}")
        return 1

    m = README_COVERAGE_RE.search(text)
    if not m:
        print(
            f"\nERROR: {readme} has no parseable coverage claim. Expected a sentence "
            f'like "**38 of 68** spec operations (**55.9%**), measured against the '
            f'pinned `v0.8.1` spec". Reword the guard in '
            f"check_spec_drift.py:README_COVERAGE_RE if the wording must change."
        )
        return 1

    num, den, pct, _ = coverage(targeted, available, deprecated)

    claimed = (m.group("num"), m.group("den"), m.group("pct"), m.group("tag"))
    actual = (str(num), str(den), pct, pinned)
    if claimed != actual:
        if write:
            values = {"num": str(num), "den": str(den), "pct": pct, "tag": pinned}
            with open(readme, "w", newline="") as f:
                f.write(text[: m.start()] + _splice_groups(m, values) + text[m.end():])
            print(
                f"Rewrote {readme}'s coverage claim: "
                f"{claimed[0]} of {claimed[1]} ({claimed[2]}%) against {claimed[3]} "
                f"-> {actual[0]} of {actual[1]} ({actual[2]}%) against {actual[3]}."
            )
            return 0
        print(
            f"\nERROR: {readme}'s coverage claim is stale.\n"
            f"  claims: {claimed[0]} of {claimed[1]} ({claimed[2]}%) "
            f"against {claimed[3]}\n"
            f"  actual: {actual[0]} of {actual[1]} ({actual[2]}%) "
            f"against {actual[3]}\n"
            f"  Fix with: python3 scripts/check_spec_drift.py --sync-coverage "
            f"<openapi.json>"
        )
        return 1

    print(
        f"\nOK: {readme}'s coverage claim ({num} of {den}, {pct}%, {pinned}) "
        f"matches this run."
    )
    return 0


def main():
    # Positional-flexible on purpose: this is called from three workflows and by
    # hand, and `... openapi.json --sync-coverage` is the order half of them will
    # reach for. A usage error there would be a confusing way to fail a repair.
    #
    # But only the flags this script defines are filtered out. Anything else that
    # starts with `-` is a usage error, not a path: the earlier filter kept every
    # unrecognised token as positional, so `--sync-coverge` became the spec path and
    # failed with a FileNotFoundError traceback naming the typo as a missing file.
    flags = {"--sync-coverage"}
    unknown = [a for a in sys.argv[1:] if a.startswith("-") and a not in flags]
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    sync_coverage = "--sync-coverage" in sys.argv[1:]
    if unknown or len(args) != 1:
        if unknown:
            print(f"unrecognised option(s): {' '.join(unknown)}", file=sys.stderr)
        sys.exit(f"usage: {sys.argv[0]} [--sync-coverage] <openapi.json>")
    with open(args[0]) as f:
        spec = json.load(f)
    version = (spec.get("info") or {}).get("version")
    targeted = load_targeted()
    available = spec_ops(spec)
    deprecated = deprecated_ops(spec)

    print(f"Spec version: {version or '?'}")

    if sync_coverage:
        # Repair only, and only the four values invariant 7 reads. Nothing else is
        # checked: this runs from `sdk-autobump.yml` right after the pin moves, and
        # a bump that also breaks invariant 1 must fail in the PR's own CI run with
        # invariant 1's message, not be pre-empted by a writer.
        sys.exit(
            check_readme_coverage_claim(
                targeted, available, deprecated=deprecated, write=True,
                spec_version=version,
            )
        )

    # Invariant 1: endpoints.txt <-> spec.
    spec_failures = check_targets_vs_spec(targeted, available, deprecated)
    failures = spec_failures
    # Invariants 2-4: CLI code <-> endpoints.txt, allowlist hygiene, and
    # CLI_SOURCES completeness.
    failures += check_code_vs_targets(targeted, available)
    # Invariant 3, hatch half (ENG-8616): CODE_ONLY_OPS is sealed empty, so any
    # entry fails. This replaces the ENG-7927 attribution rule, which let an
    # attributed phantom op ship.
    failures += check_allowlist_is_sealed()
    # Invariant 9 (ENG-12786): every `client.<method>()` call is mapped, so the
    # parser above cannot be blind to a command it has no row for. Runs after the
    # checks above deliberately — those report what the mapping SAYS, this reports
    # what it is MISSING, and the second is the more useful last word when both
    # fire.
    failures += check_method_op_complete()
    # Invariant 7: the README's committed coverage sentence is still true.
    #
    # Skipped when invariant 1 already failed, because then it cannot say anything
    # true. A renamed or removed spec endpoint drops out of the numerator, so this
    # check would fire a second, misleading "the README's coverage claim is stale"
    # — pointing at the sentence when the fix is the endpoint — and double-count the
    # same root cause in the exit status. The claim is re-checked for real on the
    # next run, once invariant 1 is green.
    if spec_failures:
        print(
            "\nSKIPPED (invariant 7): the README's coverage claim is not checked "
            "while invariant 1 is failing — the numerator is computed from the spec, "
            "so a missing endpoint would report the sentence as stale when the fix "
            "is above. Re-run once the endpoint is resolved."
        )
    else:
        failures += check_readme_coverage_claim(
            targeted, available, deprecated=deprecated, spec_version=version
        )

    if failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
