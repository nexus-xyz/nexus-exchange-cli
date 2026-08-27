#!/usr/bin/env python3
"""Self-test for the drift checker: prove it goes RED when defeated (ENG-7962).

`check_spec_drift.py` is the CLI's whole merge signal for a spec-pin advance —
"safe to merge = spec-drift is green" is only true if a green run means something.
Until this file existed the checker had no test at all, so a parser or allowlist
edit that quietly stopped enforcing an invariant would still report OK. That is
the same failure shape as the bug ENG-7962 found: `amend_order` was mapped to
`PUT /orders/{order_id}` while the SDK issues `PATCH`, the wrong op was parked in
CODE_ONLY_OPS as "ahead of spec", and the check went green over an operation the
spec had covered since v0.7.1. One test would have caught it; there wasn't one.

The counterpart in nexus-exchange-rs is the same-named script. Each of the five
invariants gets at least one test that defeats it and asserts a non-zero error
count, plus regression tests pinning the ENG-7962 fix in the real repo files so
the synthetic fixtures cannot pass while reality drifts out from under them.

Hermetic: no network, no pinned-spec download, no writes outside a temp dir.

Run: python3 scripts/test_check_spec_drift.py   (stdlib unittest; no pytest needed)
"""
import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check_spec_drift as csd  # noqa: E402


def _quiet(fn, *args, **kwargs):
    """Run a check fn, swallowing its stdout; return its error count."""
    with contextlib.redirect_stdout(io.StringIO()):
        return fn(*args, **kwargs)


def spec_of(*ops, version="0.0.0-test"):
    """Build a minimal OpenAPI doc covering exactly `ops` (METHOD, path) pairs.

    `version` is settable because invariant 7 now refuses to pair a ratio with a pin
    naming a different release: a fixture that writes `.api-version` must declare
    the matching `info.version`, or it exercises that refusal instead of the branch
    it was written for.
    """
    paths = {}
    for method, path in ops:
        paths.setdefault(path, {})[method.lower()] = {"responses": {}}
    return {"info": {"version": version}, "paths": paths}


# The release the subprocess fixtures pin. One name, so the `.api-version` a
# fixture writes and the `info.version` its spec declares cannot drift apart —
# which invariant 7 would (correctly) reject.
PINNED_TAG = "v0.8.1"

# Two real METHOD_OP methods, used to build synthetic Rust. Real names matter:
# `_CALL_RE` is compiled from METHOD_OP at import time, so a made-up method name
# would simply not be recognised and the fixture would prove nothing.
MARKETS = csd.METHOD_OP["fetch_markets"]  # GET /markets
POSITIONS = csd.METHOD_OP["fetch_positions"]  # GET /api/v1/positions

RUST_CALLING_MARKETS = 'let m = client.fetch_markets().await?;\n'
RUST_CALLING_BOTH = RUST_CALLING_MARKETS + "let p = client.fetch_positions().await?;\n"


class SyntheticRepo:
    """A temp `src/` dir plus an endpoints.txt, so an invariant can be defeated
    without touching the real tree."""

    def __init__(self, case, files, targeted_lines):
        self.dir = tempfile.TemporaryDirectory()
        case.addCleanup(self.dir.cleanup)
        self.src_dir = os.path.join(self.dir.name, "src")
        os.mkdir(self.src_dir)
        self.sources = []
        for name, body in files.items():
            # `name` may be nested ("commands/orders.rs") so a submodule can be
            # placed the way a real one would be — invariant 4 has to walk into it.
            path = os.path.join(self.src_dir, name)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as f:
                f.write(body)
            self.sources.append(path)
        self.endpoints = os.path.join(self.dir.name, "endpoints.txt")
        with open(self.endpoints, "w") as f:
            f.write("\n".join(targeted_lines) + "\n")

    def unscan(self, name):
        """Drop `name` from the scanned set while leaving it on disk — the
        invariant-4 setup (a handler module the parser does not read). Accepts a
        bare filename or a nested one ("commands/orders.rs")."""
        self.sources = [
            p
            for p in self.sources
            if os.path.basename(p) != name
            and os.path.relpath(p, self.src_dir) != name
        ]

    def targeted(self):
        return csd.load_targeted(self.endpoints)

    def check(self, spec, code_only=(), non_rest=()):
        """Run invariants 2-4 over this synthetic repo. Both allowlists default to
        EMPTY rather than the real ones: the real entries describe the real tree,
        so inheriting them here would fire stale-allowlist errors on every fixture
        and drown the invariant under test. A test that needs an exemption passes
        it explicitly."""
        # CODE_ONLY_OPS is a plain set again, and sealed empty on the real tree
        # (ENG-8616). It is still patchable here because `check_code_vs_targets`
        # must be shown NOT to consult it: the `code_only` argument exists so a
        # test can prove an entry no longer suppresses anything.
        with patched("CODE_ONLY_OPS", set(code_only)), patched(
            "NON_REST_TARGETS", set(non_rest)
        ):
            return _quiet(
                csd.check_code_vs_targets,
                self.targeted(),
                csd.spec_ops(spec),
                sources=self.sources,
                src_dir=self.src_dir,
            )


@contextlib.contextmanager
def patched(name, value):
    """Temporarily set csd.<name> = value, restoring the original after."""
    original = getattr(csd, name)
    setattr(csd, name, value)
    try:
        yield
    finally:
        setattr(csd, name, original)


class TestInvariant1TargetsVsSpec(unittest.TestCase):
    """endpoints.txt -> the pinned spec."""

    def test_all_targets_present_passes(self):
        spec = spec_of(MARKETS, POSITIONS)
        errs = _quiet(
            csd.check_targets_vs_spec, [MARKETS, POSITIONS], csd.spec_ops(spec)
        )
        self.assertEqual(errs, 0)

    def test_removed_operation_fails(self):
        """The case the pin bump exists to catch: the new spec dropped an op the
        CLI targets."""
        spec = spec_of(MARKETS)  # POSITIONS removed
        errs = _quiet(
            csd.check_targets_vs_spec, [MARKETS, POSITIONS], csd.spec_ops(spec)
        )
        self.assertEqual(errs, 1)

    def test_renamed_path_fails(self):
        spec = spec_of(MARKETS, ("GET", "/api/v1/positions/all"))
        errs = _quiet(
            csd.check_targets_vs_spec, [MARKETS, POSITIONS], csd.spec_ops(spec)
        )
        self.assertEqual(errs, 1)

    def test_method_change_on_the_same_path_fails(self):
        """A verb-only change still has to bite — matching on path alone would let
        it through."""
        spec = spec_of(MARKETS, ("POST", "/api/v1/positions"))
        errs = _quiet(
            csd.check_targets_vs_spec, [MARKETS, POSITIONS], csd.spec_ops(spec)
        )
        self.assertEqual(errs, 1)

    def test_uncovered_spec_ops_are_informational_not_errors(self):
        """Coverage gaps are reported, never fatal — the CLI is not required to
        implement the whole spec."""
        spec = spec_of(MARKETS, POSITIONS, ("GET", "/admin/tiers"))
        errs = _quiet(
            csd.check_targets_vs_spec, [MARKETS, POSITIONS], csd.spec_ops(spec)
        )
        self.assertEqual(errs, 0)


class TestInvariant2CodeVsTargets(unittest.TestCase):
    """CLI code <-> endpoints.txt, as real set equality in both directions."""

    def _repo(self, files, targeted_lines):
        return SyntheticRepo(self, files, targeted_lines)

    def test_exact_match_passes(self):
        repo = self._repo(
            {"main.rs": RUST_CALLING_BOTH},
            ["GET /markets", "GET /api/v1/positions"],
        )
        self.assertEqual(repo.check(spec_of(MARKETS, POSITIONS)), 0)

    def test_called_but_unlisted_fails(self):
        """Direction (a): a command reaches an op endpoints.txt does not claim."""
        repo = self._repo({"main.rs": RUST_CALLING_BOTH}, ["GET /markets"])
        self.assertGreater(repo.check(spec_of(MARKETS, POSITIONS)), 0)

    def test_listed_but_uncalled_fails(self):
        """Direction (b) — the one a subset check would miss. endpoints.txt claims
        an op no command calls, which is how a manifest silently stops describing
        reality (the py bug in ENG-7958)."""
        repo = self._repo(
            {"main.rs": RUST_CALLING_MARKETS},
            ["GET /markets", "GET /api/v1/positions"],
        )
        self.assertGreater(repo.check(spec_of(MARKETS, POSITIONS)), 0)

    def test_equality_is_not_a_subset_check_in_either_direction(self):
        """Pins the property the issue asks about explicitly: neither side may be a
        proper subset of the other without a failure."""
        superset_code = self._repo(
            {"main.rs": RUST_CALLING_BOTH}, ["GET /markets"]
        )
        superset_manifest = self._repo(
            {"main.rs": RUST_CALLING_MARKETS},
            ["GET /markets", "GET /api/v1/positions"],
        )
        spec = spec_of(MARKETS, POSITIONS)
        self.assertGreater(superset_code.check(spec), 0)
        self.assertGreater(superset_manifest.check(spec), 0)

    def test_code_only_allowlist_no_longer_suppresses_an_ahead_of_spec_op(self):
        """Revert-with-the-test-kept for ENG-8616, on the (a) direction.

        This is the exact case that used to PASS: the op is implemented, the pinned
        spec does not define it, and an allowlist entry kept it out of
        endpoints.txt. That was the supported way to ship an operation with no
        contract, and it is how nine dead-end commands shipped green. The entry now
        suppresses nothing, so the call is reported like any other unlisted op."""
        repo = self._repo({"main.rs": RUST_CALLING_BOTH}, ["GET /markets"])
        self.assertGreater(repo.check(spec_of(MARKETS), code_only={POSITIONS}), 0)
        # And it is equally an error with no entry at all — the entry is inert,
        # not merely insufficient.
        self.assertGreater(repo.check(spec_of(MARKETS)), 0)

    def test_non_rest_allowlist_suppresses_a_listed_uncalled_op(self):
        repo = self._repo(
            {"main.rs": RUST_CALLING_MARKETS}, ["GET /markets", "GET /ws"]
        )
        self.assertEqual(
            repo.check(
                spec_of(MARKETS, ("GET", "/ws")), non_rest={("GET", "/ws")}
            ),
            0,
        )

    def test_parser_is_receiver_agnostic(self):
        """`_CALL_RE` anchors on the method name, not the receiver, so a renamed
        binding (`ws_client`, an owned clone, a chained expression) is still seen.
        If this regressed, ops would be silently under-counted."""
        for receiver in ("client", "ws_client", "self.client", "c.clone()"):
            with self.subTest(receiver=receiver):
                repo = self._repo(
                    {"main.rs": f"{receiver}.fetch_markets().await?;\n"},
                    ["GET /markets"],
                )
                self.assertEqual(repo.check(spec_of(MARKETS)), 0)

    def test_zero_parsed_calls_fails_closed(self):
        """If the call pattern ever changes wholesale, the checker must abort — not
        report an empty targeted set (which would fail every endpoints.txt line
        with a confusing message, or pass an empty manifest)."""
        repo = self._repo({"main.rs": "fn main() {}\n"}, ["GET /markets"])
        with self.assertRaises(SystemExit):
            repo.check(spec_of(MARKETS))


class TestInvariant3AllowlistHygiene(unittest.TestCase):
    """An exemption must stay earned. Each of these is a silent hole if unchecked."""

    def _repo(self, files, targeted_lines):
        return SyntheticRepo(self, files, targeted_lines)

    # The two CODE_ONLY_OPS rot checks that used to live here — "no command calls
    # this row" and "the pinned spec has caught up with it" — went with the rows
    # (ENG-12369). Both could only fire on a non-empty table, and `TestAllowlist\
    # IsSealed` asserts the stronger property: no table. The ENG-7962 wrong-verb
    # case they also covered is now caught by invariant 2(a) directly, since a
    # mis-mapped op is simply an op endpoints.txt does not list.

    def test_non_rest_entry_absent_from_endpoints_txt_fails(self):
        repo = self._repo({"main.rs": RUST_CALLING_MARKETS}, ["GET /markets"])
        self.assertGreater(
            repo.check(spec_of(MARKETS, ("GET", "/ws")), non_rest={("GET", "/ws")}),
            0,
        )


class TestInvariant4SourcesCompleteness(unittest.TestCase):
    """The parser only reads CLI_SOURCES, so an unread module that reaches the API
    is an under-count that reads as green."""

    def test_handler_in_an_unscanned_module_fails(self):
        repo = SyntheticRepo(
            self,
            {"main.rs": RUST_CALLING_MARKETS, "positions.rs": "client.fetch_positions().await?;\n"},
            ["GET /markets"],
        )
        repo.unscan("positions.rs")  # the module exists but nothing scans it
        self.assertGreater(repo.check(spec_of(MARKETS, POSITIONS)), 0)

    def test_unscanned_module_without_sdk_calls_passes(self):
        """Most modules (output formatting, credentials) never touch the SDK; they
        must not be flagged."""
        repo = SyntheticRepo(
            self,
            {"main.rs": RUST_CALLING_MARKETS, "output.rs": "fn render() {}\n"},
            ["GET /markets"],
        )
        repo.unscan("output.rs")
        self.assertEqual(repo.check(spec_of(MARKETS)), 0)

    def test_non_rust_files_are_ignored(self):
        repo = SyntheticRepo(
            self,
            {"main.rs": RUST_CALLING_MARKETS, "notes.md": "client.fetch_positions()\n"},
            ["GET /markets"],
        )
        repo.unscan("notes.md")
        self.assertEqual(repo.check(spec_of(MARKETS)), 0)

    def test_handler_in_a_nested_module_fails(self):
        """`src/` is flat today, so a one-level listing passed the test above while
        missing the likeliest shape of the very change this invariant guards: a
        handler moved to `src/commands/orders.rs`. The scan has to walk."""
        repo = SyntheticRepo(
            self,
            {
                "main.rs": RUST_CALLING_MARKETS,
                "commands/positions.rs": "client.fetch_positions().await?;\n",
            },
            ["GET /markets"],
        )
        repo.unscan("commands/positions.rs")
        self.assertGreater(repo.check(spec_of(MARKETS, POSITIONS)), 0)

    def test_nested_module_is_named_in_the_error(self):
        """The fix is "add this file to CLI_SOURCES", so the path has to appear —
        including its directory, or the reader looks in the wrong place."""
        repo = SyntheticRepo(
            self,
            {
                "main.rs": RUST_CALLING_MARKETS,
                "commands/positions.rs": "client.fetch_positions().await?;\n",
            },
            ["GET /markets"],
        )
        repo.unscan("commands/positions.rs")
        offenders = csd.unscanned_sources(repo.src_dir, repo.sources)
        self.assertEqual(len(offenders), 1)
        self.assertTrue(
            offenders[0][0].endswith(os.path.join("commands", "positions.rs")),
            f"expected the nested path in the report, got {offenders[0][0]!r}",
        )
        self.assertEqual(offenders[0][1], ["fetch_positions"])

    def test_nested_module_without_sdk_calls_passes(self):
        repo = SyntheticRepo(
            self,
            {"main.rs": RUST_CALLING_MARKETS, "commands/render.rs": "fn go() {}\n"},
            ["GET /markets"],
        )
        repo.unscan("commands/render.rs")
        self.assertEqual(repo.check(spec_of(MARKETS)), 0)


class TestManifestParser(unittest.TestCase):
    """load_targeted: endpoints.txt is hand-edited, so its parser fails loudly."""

    def _write(self, text):
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as fh:
            fh.write(textwrap.dedent(text))
            name = fh.name
        self.addCleanup(os.unlink, name)
        return name

    def test_comments_and_blank_lines_skipped(self):
        path = self._write(
            """\
            # a comment
            GET /markets

            GET /api/v1/positions
            """
        )
        self.assertEqual(
            csd.load_targeted(path), [("GET", "/markets"), ("GET", "/api/v1/positions")]
        )

    def test_method_is_upcased(self):
        path = self._write("get /markets\n")
        self.assertEqual(csd.load_targeted(path), [("GET", "/markets")])

    def test_malformed_line_fails_closed(self):
        with self.assertRaises(SystemExit):
            csd.load_targeted(self._write("/markets\n"))

    def test_duplicate_line_fails_closed(self):
        """A duplicate would inflate the coverage numerator."""
        with self.assertRaises(SystemExit):
            csd.load_targeted(self._write("GET /markets\nGET /markets\n"))


class TestNormalizePath(unittest.TestCase):
    def test_placeholder_names_collapse(self):
        self.assertEqual(csd.normalize_path("/orders/{order_id}"), "/orders/{}")
        self.assertEqual(csd.normalize_path("/orders/{id}"), "/orders/{}")

    def test_multiple_placeholders(self):
        self.assertEqual(
            csd.normalize_path("/a/{x}/b/{y}"),
            "/a/{}/b/{}",
        )

    def test_plain_path_unchanged(self):
        self.assertEqual(csd.normalize_path("/api/v1/tickers"), "/api/v1/tickers")


class TestInvariant9MethodOpCompleteness(unittest.TestCase):
    """Invariant 9 (ENG-12786): a `client.<method>()` call with no METHOD_OP row.

    The bug being pinned is subtle enough to restate: `_CALL_RE` is built from
    METHOD_OP's keys, so the invariant-2 parser CANNOT report a missing row — the
    call simply does not match, and every downstream set is short one op while the
    run stays green. So these tests defeat it the only way that means anything: a
    file that calls a method the table does not carry, and an assertion that the
    check goes red naming it.
    """

    def _sources(self, body, name="main.rs"):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        path = os.path.join(d.name, name)
        with open(path, "w") as f:
            f.write(body)
        return [path]

    def test_an_unmapped_call_fails(self):
        srcs = self._sources("let x = client.brand_new_method(&req).await?;\n")
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 1)

    def test_the_error_names_the_method_and_the_file(self):
        """An error that does not say which call it means is a puzzle, not a check."""
        srcs = self._sources("client.brand_new_method(&req).await?;\n", "handlers.rs")
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), patched("NON_OP_CLIENT_METHODS", set()):
            csd.check_method_op_complete(srcs)
        out = buf.getvalue()
        self.assertIn("brand_new_method", out)
        self.assertIn("handlers.rs", out)
        self.assertIn("METHOD_OP", out)

    def test_a_fully_mapped_source_passes(self):
        srcs = self._sources("let m = client.fetch_markets().await?;\n")
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 0)

    def test_rustfmt_line_breaks_do_not_hide_a_call(self):
        """rustfmt puts the receiver on its own line for a chained await, which is how
        every real call site in this repo is written. A regex that only matched
        `client.method(` on one line would see none of them."""
        srcs = self._sources(
            "let x = client\n    .brand_new_method(&req)\n    .await?;\n"
        )
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 1)

    def test_a_renamed_binding_is_still_seen(self):
        """`ws_client` is the second real binding; a check anchored on the exact name
        `client` would miss the file wsclient.rs entirely."""
        srcs = self._sources("ws_client.brand_new_method(&req).await?;\n")
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 1)

    def test_a_non_rest_method_is_exempt(self):
        srcs = self._sources("ws_client.connect(&url).await?;\n")
        self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 0)

    def test_a_non_rest_exemption_nothing_calls_is_stale(self):
        """The exemption list is held to the same rule as the other two: an entry that
        stops being earned is a silent hole, so it fails rather than lingering."""
        srcs = self._sources("let m = client.fetch_markets().await?;\n")
        with patched("NON_OP_CLIENT_METHODS", {"connect"}):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 1)

    def test_ordinary_rust_method_calls_are_not_flagged(self):
        """The receiver anchor is what keeps this from matching every `.iter()` in the
        tree. Without it the check would be unusable and would be turned off."""
        srcs = self._sources(
            "let s = name.to_string();\n"
            "for x in items.iter().map(|v| v.clone()) {}\n"
            "let m = client.fetch_markets().await?;\n"
        )
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(_quiet(csd.check_method_op_complete, srcs), 0)

    def test_zero_parsed_calls_fails_closed(self):
        """Same rule as the invariant-2 parser: if the binding is ever renamed
        wholesale, abort loudly rather than report a clean bill of health over a file
        this regex can no longer read."""
        srcs = self._sources("fn main() {}\n")
        with self.assertRaises(SystemExit):
            _quiet(csd.check_method_op_complete, srcs)

    def test_invariant_2_is_blind_to_what_invariant_9_catches(self):
        """The case that makes 9 worth having, shown rather than asserted.

        Once an endpoints.txt line exists, dropping its METHOD_OP row also fails
        invariant 2 — so it is fair to ask what 9 adds. It adds the case that
        actually happened: a command ships calling a method with NO row AND NO
        endpoints.txt line. There is then nothing on either side of invariant 2's
        equality to be unbalanced by, and it passes clean. That is the whole life of
        `sign_in` and `register_agent`.

        Same fixture, both checks, opposite verdicts."""
        repo = SyntheticRepo(
            self,
            {"main.rs": "client.fetch_markets().await?;\n"
                        "client.brand_new_method(&req).await?;\n"},
            ["GET /markets"],
        )
        self.assertEqual(
            repo.check(spec_of(MARKETS)), 0,
            "invariant 2 cannot see the unmapped call — it is not in either set",
        )
        with patched("NON_OP_CLIENT_METHODS", set()):
            self.assertEqual(
                _quiet(csd.check_method_op_complete, repo.sources), 1,
                "invariant 9 must catch it",
            )


class TestRealRepoState(unittest.TestCase):
    """Against the committed files, so the synthetic fixtures above cannot stay
    green while the real tree drifts."""

    def test_every_method_op_row_is_reachable_or_the_map_is_stale(self):
        """A METHOD_OP row no command calls is dead weight that makes the mapping
        harder to trust. Not fatal in the checker (the CLI may legitimately drop a
        command before the row is cleaned up), but it should be visible."""
        _, seen = _quiet(csd.called_ops)
        unused = sorted(set(csd.METHOD_OP) - seen)
        self.assertEqual(
            unused,
            [],
            f"METHOD_OP rows no CLI command calls: {unused}. Remove them, or add "
            f"the calling command.",
        )

    def test_the_real_sources_are_fully_mapped(self):
        """The invariant against the committed tree, not a fixture. This is the test
        that was red before ENG-12786: `sign_in` and `register_agent` were called by
        `auth login` and `agents register` with no METHOD_OP row."""
        self.assertEqual(_quiet(csd.check_method_op_complete), 0)

    def test_the_wallet_auth_ops_are_counted_not_invisible(self):
        """Pins the two the missing rows hid. They are ordinary spec operations the
        CLI calls; while they were unmapped, `POST /auth/login` was printed in the
        `Not covered by the CLI` list on every run."""
        for method, op in (
            ("sign_in", ("POST", "/auth/login")),
            ("register_agent", ("POST", "/agents/register")),
        ):
            with self.subTest(method=method):
                self.assertEqual(csd.METHOD_OP.get(method), op)
                self.assertIn(op, csd.load_targeted())

    def test_cli_sources_all_exist(self):
        for path in csd.CLI_SOURCES:
            self.assertTrue(os.path.isfile(path), f"CLI_SOURCES entry missing: {path}")

    def test_amend_order_is_patch_not_put(self):
        """ENG-7962 regression. The SDK issues `signed_patch_with_query` for
        `amend_order` (nexus-exchange 0.6.0 src/rest.rs); mapping it to PUT is what
        let the op hide in CODE_ONLY_OPS."""
        self.assertEqual(csd.METHOD_OP["amend_order"], ("PATCH", "/orders/{order_id}"))

    def test_order_amend_is_counted_not_exempted(self):
        """The other half of the same regression: the op belongs in endpoints.txt,
        and the stale PUT exemption must stay gone."""
        self.assertIn(("PATCH", "/orders/{order_id}"), csd.load_targeted())
        self.assertNotIn(("PUT", "/orders/{}"), csd.CODE_ONLY_OPS)

    def test_the_real_code_only_table_is_empty(self):
        """Runs the seal against the REAL table, not a synthetic one.

        `TestCheckAllowlistIsSealed` patches `CODE_ONLY_OPS` to prove the *check*
        rejects an entry. This proves the *committed* table currently passes it — a
        different claim, and the one that catches a hand-edit to the real file
        between checker runs. It is the assertion that would have to be deleted, in
        this file, to re-open the hatch (ENG-8616).
        """
        self.assertEqual(csd.CODE_ONLY_OPS, set(), "the allowlist must stay empty")
        self.assertEqual(_quiet(csd.check_allowlist_is_sealed), 0)

    def test_no_method_op_row_targets_a_withdrawn_operation(self):
        """The nine deleted commands must not creep back in through METHOD_OP.

        Deleting a subcommand while leaving its mapping row would leave the next
        person a ready-made row to wire a command to, which is how `margin-mode`
        came about. The SDK methods themselves were deleted in nexus-exchange-rs
        #143, so any of these would also stop compiling at the next crate bump.
        """
        withdrawn = {
            "set_leverage",
            "set_margin_mode",
            "fetch_funding_payments",
            "create_transfer",
            "fetch_transfers",
            "create_sub_account",
            "fetch_sub_accounts",
            "cancel_orders",
            "fetch_order_by_client_id",
            "cancel_order_by_client_id",
        }
        present = sorted(withdrawn & set(csd.METHOD_OP))
        self.assertEqual(present, [], f"withdrawn methods still mapped: {present}")

    def test_no_code_only_entry_shares_a_path_with_endpoints_txt(self):
        """Cheap invariant on the real files: an op cannot be both claimed and
        exempted."""
        targeted = {(m, csd.normalize_path(p)) for m, p in csd.load_targeted()}
        overlap = sorted(csd.CODE_ONLY_OPS & targeted)
        self.assertEqual(overlap, [], f"claimed AND exempted: {overlap}")


class TestCheckAllowlistIsSealed(unittest.TestCase):
    """`CODE_ONLY_OPS` must be empty, and ANY entry must go red (ENG-8616).

    This class replaces `TestCheckAllowlistIsHonest`, which mechanised the ENG-7927
    attribution rules — a row had to name the command it backed, a reason, and a
    tracking issue. Those rules worked and were tested, and they still let nine
    phantom operations ship: attribution makes a row honest about what it is, not
    true about whether the operation exists. A ticket reference beside a phantom op
    only makes it look sanctioned.

    The important case is `test_an_entry_absent_from_the_spec_fails`. Under every
    earlier version of this check that case PASSED — it was the definition of a
    legitimate row — and it is the exact shape of the nine commands a user could
    read in `--help` and could not run. It must now be red.

    `check_allowlist_is_sealed` reads the module-level `CODE_ONLY_OPS` directly and
    takes no arguments, so these patch it rather than going through `SyntheticRepo`.
    """

    def _errors(self, rows):
        with patched("CODE_ONLY_OPS", rows):
            return _quiet(csd.check_allowlist_is_sealed)

    def test_an_empty_table_passes(self):
        """The control. Without it every case below could pass by rejecting
        everything, which is the failure mode a seal is most prone to."""
        self.assertEqual(self._errors(set()), 0)

    def test_an_entry_absent_from_the_spec_fails(self):
        """Revert-with-the-test-kept. This is the case the old check was BUILT to
        allow: an implemented op the pinned spec does not define. It shipped
        `account leverage`, `transfers`, `sub-accounts` and six more."""
        self.assertGreater(self._errors({("POST", "/account/leverage")}), 0)

    def test_an_entry_the_spec_does_define_also_fails(self):
        """Sealed means sealed: the seal does not consult the spec at all, so there
        is no spec state in which an entry is acceptable. A check that failed only
        for absent ops would leave the hatch open for the next 'the spec will catch
        up' claim."""
        self.assertGreater(self._errors({("GET", "/markets")}), 0)

    def test_a_placeholder_spelled_entry_still_fails(self):
        """No spelling gets a row past the seal.

        Under the old check the placeholder name was load-bearing: a row written
        `/positions/{market_id}` against a spec `/positions/{marketId}` matched
        nothing and was reported under the wrong cause. The seal does not compare
        paths at all, so both spellings simply fail.
        """
        for path in ("/orders/{order_id}", "/orders/{orderId}", "/orders/{}"):
            with self.subTest(path=path):
                self.assertGreater(self._errors({("GET", path)}), 0)

    def test_several_entries_are_all_counted(self):
        self.assertEqual(
            self._errors({("POST", "/transfers"), ("GET", "/sub-accounts")}), 2
        )

    def test_the_error_names_the_entries_and_the_policy(self):
        """A seal that fails without saying what to do sends the reader back to the
        allowlist to 'fix' it. The message has to point at deletion instead."""
        with patched("CODE_ONLY_OPS", {("POST", "/transfers")}):
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                csd.check_allowlist_is_sealed()
        out = buf.getvalue()
        self.assertIn("POST /transfers", out)
        self.assertIn("ENG-8616", out)
        self.assertIn("must be", out)

    def test_the_ok_line_claims_only_emptiness(self):
        """The OK line used to say rows were 'attributed', which is a claim about
        rows that no longer exist. It must describe what was actually computed."""
        with patched("CODE_ONLY_OPS", set()):
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                csd.check_allowlist_is_sealed()
        out = buf.getvalue()
        self.assertIn("empty and sealed", out)
        self.assertNotIn("attributed", out)


# `TestCaughtUpCheckNormalisesBothSides` was removed with the code it tested
# (ENG-12369). Its three cases pinned the raw-vs-normalised comparison sites in the
# CODE_ONLY_OPS handling — the suppression subtraction, the stale-row check, and the
# caught-up check — where a row written `/positions/{market_id}` against a spec
# `/positions/{marketId}` matched nothing and was reported under the wrong cause.
# All three sites are gone: the seal does not compare paths at all, so there is no
# spelling for an entry to be written in that changes the outcome.
#
# `TestCheckAllowlistIsSealed.test_a_placeholder_spelled_entry_still_fails` keeps
# the surviving half of that concern — no way of spelling a row gets it past the
# seal — and invariant 2(a) still normalises both of its own sides, which
# `TestInvariant2CodeVsTargets` covers.


class TestCanonicalOpCollapsesDualMounts(unittest.TestCase):
    """ENG-10035: the coverage denominator must count operations, not path-ops.

    The spec mounts most operations twice (host-root and `/api/v1`), and the CLI
    targets one mount per operation. Counting both in the denominator made 100%
    unreachable and understated this repo at `38 of 101 (37.6%)`.
    """

    def test_the_api_v1_prefix_collapses(self):
        self.assertEqual(csd.canonical_op(("GET", "/api/v1/account")), ("GET", "/account"))
        self.assertEqual(
            csd.canonical_op(("DELETE", "/api/v1/orders/{order_id}")),
            ("DELETE", "/orders/{order_id}"),
        )

    def test_a_host_root_op_is_unchanged(self):
        self.assertEqual(csd.canonical_op(("GET", "/account")), ("GET", "/account"))

    def test_both_mounts_share_one_canonical_label(self):
        """The whole point: the twins must fold onto each other."""
        self.assertEqual(
            csd.canonical_op(("GET", "/api/v1/positions")),
            csd.canonical_op(("GET", "/positions")),
        )

    def test_api_v1_alone_is_not_stripped(self):
        """Only the `/api/v1/` PREFIX collapses. Pinned because the monorepo
        collector pins it too, and a divergence between the two IS ENG-10035."""
        self.assertEqual(csd.canonical_op(("GET", "/api/v1")), ("GET", "/api/v1"))

    def test_the_method_is_upper_cased(self):
        self.assertEqual(csd.canonical_op(("post", "/orders")), ("POST", "/orders"))

    def test_a_path_containing_api_v1_deeper_is_untouched(self):
        """The prefix is anchored, so an `/api/v1` appearing mid-path is not a
        dual-stack mount and must not be rewritten."""
        self.assertEqual(
            csd.canonical_op(("GET", "/proxy/api/v1/thing")),
            ("GET", "/proxy/api/v1/thing"),
        )


class TestCoverageRatioIsKeyedOnOperations(unittest.TestCase):
    """The ratio collapses twins; existence never does."""

    def _coverage_line(self, targeted, available):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            csd.check_targets_vs_spec(targeted, available)
        return buf.getvalue()

    def test_targeting_one_mount_of_every_operation_reads_100(self):
        """The ceiling ENG-10035 says was unreachable. Before the fix this pair of
        mounts made the best possible score 50%."""
        available = {("GET", "/account"), ("GET", "/api/v1/account")}
        out = self._coverage_line([("GET", "/account")], available)
        self.assertIn("1 of 1 spec operations (100.0% coverage)", out)
        self.assertIn("from 2 path-ops", out)

    def test_a_covered_operation_is_not_listed_under_its_twin(self):
        """The old list reported the untargeted mount of a covered operation, which
        is a worklist item that does not exist."""
        available = {("GET", "/account"), ("GET", "/api/v1/account")}
        out = self._coverage_line([("GET", "/account")], available)
        self.assertNotIn("Not covered", out)

    def test_an_uncovered_operation_is_reported_as_literal_mounts(self):
        """Never the canonical label: it can name a mount the spec does not
        document, and acting on one ships a method that 404s (ENG-8463)."""
        available = {("GET", "/stats"), ("GET", "/api/v1/stats")}
        out = self._coverage_line([], available)
        self.assertIn("Not covered by the CLI (1)", out)
        # Sorted, matching the monorepo collector's `sorted(v)` over the mounts.
        self.assertIn("GET /api/v1/stats, GET /stats", out)

    def test_a_phantom_twin_is_not_credited(self):
        """The load-bearing ordering. `/account/margin` has no `/api/v1` mount, so
        targeting `POST /api/v1/account/margin` targets a route that 404s. Collapsing
        before the literal intersection would fold it onto the real operation and
        credit it — exactly ENG-8463.
        """
        available = {("POST", "/account/margin")}
        out = self._coverage_line([("POST", "/api/v1/account/margin")], available)
        self.assertIn("0 of 1 spec operations (0.0% coverage)", out)
        # ...and existence still fails literally, which is the other half.
        self.assertIn("are NOT in the spec", out)

    def test_existence_stays_literal_after_the_change(self):
        """A targeted mount the spec does not document must still be an ERROR, not
        be rescued by having a documented twin."""
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_targets_vs_spec(
                [("GET", "/api/v1/account")], {("GET", "/account")}
            )
        self.assertEqual(errors, 1)
        self.assertIn("GET /api/v1/account", buf.getvalue())


class TestRealRepoCoverageAgreesWithTheDashboard(unittest.TestCase):
    """The committed `endpoints.txt` must target one mount per operation.

    If it ever targeted both mounts of the same operation, the numerator would
    silently drop below the line count and the two numbers would stop meaning the
    same thing.
    """

    def test_no_two_targets_are_twins_of_each_other(self):
        targeted = csd.load_targeted(
            os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                         "endpoints.txt")
        )
        canon = {}
        for op in targeted:
            canon.setdefault(csd.canonical_op(op), []).append(op)
        collisions = {k: v for k, v in canon.items() if len(v) > 1}
        self.assertEqual(
            collisions, {},
            "endpoints.txt targets both mounts of one operation; the coverage "
            "numerator collapses them, so the count would understate the manifest",
        )


def readme_claiming(case, sentence, pinned="v0.8.1"):
    """A temp README carrying `sentence` inside human-owned prose, plus the
    `.api-version` the claim is measured against. Returns `(readme, pin)`.

    Module-level, like `spec_of`: both the guard's tests and the writer's tests need
    the same fixture, and a shared helper beats one TestCase reaching into another.
    """
    d = tempfile.TemporaryDirectory()
    case.addCleanup(d.cleanup)
    readme = os.path.join(d.name, "README.md")
    with open(readme, "w") as f:
        f.write(
            "### API coverage\n\nUntouched prose above.\n\n"
            f"The check also prints a coverage number: the CLI currently "
            f"exercises {sentence}. And prose after it.\n\nBelow.\n"
        )
    pin = os.path.join(d.name, ".api-version")
    with open(pin, "w") as f:
        f.write(pinned + "\n")
    return readme, pin


class TestReadmeCoverageClaimGuard(unittest.TestCase):
    """Invariant 7: the README's committed coverage sentence.

    It shipped with no test at all — the only evidence it worked was a manual `sed`
    — while README.md and `spec-drift.yml` both promise every invariant has a
    defeat-test running ahead of it. Worse, its most fragile branch was the
    unparseable one: the guard matched with literal single spaces, so reflowing the
    paragraph (which wraps at ~80 columns, right across these numbers) failed it as
    "no parseable coverage claim" — a message that sends the reader to check
    arithmetic that was never wrong.
    """

    # Two mounts of one operation, one targeted: 1 of 1, 100.0%.
    AVAILABLE = {("GET", "/account"), ("GET", "/api/v1/account")}
    TARGETED = [("GET", "/account")]

    def _readme(self, sentence):
        return readme_claiming(self, sentence)

    def _run(self, sentence, write=False, targeted=None, available=None,
             deprecated=frozenset(), spec_version="0.8.1"):
        readme, pin = self._readme(sentence)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED if targeted is None else targeted,
                self.AVAILABLE if available is None else available,
                readme=readme,
                api_version_file=pin,
                deprecated=deprecated,
                write=write,
                # The fixture pins v0.8.1, so the default here is the spec that pin
                # names. Passing it is not incidental: invariant 7 now refuses to
                # pair a ratio with a tag from a different release, so a test that
                # omitted it would be exercising that refusal rather than the branch
                # it is named for.
                spec_version=spec_version,
            )
        return errors, buf.getvalue(), readme

    CLAIM = ("**1 of 1** spec operations (**100.0%**), measured against the pinned "
             "`v0.8.1` spec")

    def test_a_true_claim_passes(self):
        errors, out, _ = self._run(self.CLAIM)
        self.assertEqual(errors, 0, out)
        self.assertIn("matches this run", out)

    def test_a_stale_numerator_fails(self):
        errors, out, _ = self._run(
            "**0 of 1** spec operations (**0.0%**), measured against the pinned "
            "`v0.8.1` spec"
        )
        self.assertEqual(errors, 1)
        self.assertIn("coverage claim is stale", out)
        self.assertIn("claims: 0 of 1 (0.0%) against v0.8.1", out)
        self.assertIn("actual: 1 of 1 (100.0%) against v0.8.1", out)

    def test_a_stale_denominator_fails(self):
        """The ENG-10035 shape itself: the sentence kept the path-op count."""
        errors, out, _ = self._run(
            "**1 of 2** spec operations (**50.0%**), measured against the pinned "
            "`v0.8.1` spec"
        )
        self.assertEqual(errors, 1)
        self.assertIn("coverage claim is stale", out)

    def test_a_stale_tag_fails(self):
        """What actually went wrong for two releases: numbers measured against a pin
        the repo had already moved past."""
        errors, out, _ = self._run(
            "**1 of 1** spec operations (**100.0%**), measured against the pinned "
            "`v0.7.2` spec"
        )
        self.assertEqual(errors, 1)
        self.assertIn("against v0.7.2", out)
        self.assertIn("against v0.8.1", out)

    def test_a_reworded_sentence_fails_rather_than_skips(self):
        """A guard that stops matching must not silently stop guarding."""
        errors, out, _ = self._run("1 out of 1 operations, roughly 100 percent")
        self.assertEqual(errors, 1)
        self.assertIn("no parseable coverage claim", out)

    def test_a_missing_readme_fails(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme="/nonexistent/README.md",
                spec_version="0.8.1",
            )
        self.assertEqual(errors, 1)
        self.assertIn("cannot read", buf.getvalue())

    def test_a_missing_pin_file_fails(self):
        readme, _ = self._readme(self.CLAIM)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme=readme,
                api_version_file="/nonexistent/.api-version",
                spec_version="0.8.1",
            )
        self.assertEqual(errors, 1)
        self.assertIn("cannot read", buf.getvalue())


class TestTheCoverageClaimSurvivesAReflow(unittest.TestCase):
    """The fragile branch, from the direction that actually bites.

    Markdown treats a single newline inside a paragraph as a space, so a rewrap is
    not a reword — but the guard read it as one. Each of these is a line break the
    README's own 80-column wrapping produces the moment a word earlier in the
    paragraph changes; all three failed the original regex.
    """

    SENTENCE = ("**38 of 68** spec operations (**55.9%**), measured against the "
                "pinned `v0.8.1` spec")

    def _matches(self, sentence):
        m = csd.README_COVERAGE_RE.search(sentence)
        return m.groups() if m else None

    def test_the_unwrapped_sentence_matches(self):
        self.assertEqual(self._matches(self.SENTENCE), ("38", "68", "55.9", "v0.8.1"))

    def test_a_break_between_the_numbers_and_the_noun(self):
        """The break the committed README has today."""
        self.assertEqual(
            self._matches(self.SENTENCE.replace("** spec", "**\nspec")),
            ("38", "68", "55.9", "v0.8.1"),
        )

    def test_a_break_inside_the_ratio(self):
        self.assertEqual(
            self._matches(self.SENTENCE.replace("38 of 68", "38 of\n68")),
            ("38", "68", "55.9", "v0.8.1"),
        )

    def test_a_break_before_the_tag(self):
        self.assertEqual(
            self._matches(self.SENTENCE.replace("pinned `v0.8.1`", "pinned\n`v0.8.1`")),
            ("38", "68", "55.9", "v0.8.1"),
        )

    def test_a_break_after_the_article(self):
        self.assertEqual(
            self._matches(self.SENTENCE.replace("against the pinned", "against the\npinned")),
            ("38", "68", "55.9", "v0.8.1"),
        )

    def test_an_indented_continuation_line(self):
        self.assertEqual(
            self._matches(self.SENTENCE.replace("), measured", "),\n   measured")),
            ("38", "68", "55.9", "v0.8.1"),
        )

    def test_a_blank_line_does_NOT_match(self):
        """The limit, and deliberate: a blank line ends the paragraph, so the
        sentence has been split in two. That IS a rewording, and the guard should
        fail rather than tolerate it — which is why this is one-newline-tolerant
        whitespace and not `\\s+`."""
        self.assertIsNone(
            self._matches(self.SENTENCE.replace("against the pinned", "against the\n\npinned"))
        )

    def test_a_missing_separator_does_NOT_match(self):
        self.assertIsNone(self._matches(self.SENTENCE.replace("38 of", "38of")))

    def test_the_committed_readme_sentence_still_parses(self):
        """Hermetic guard on the real file: a rewrap or reword of the real paragraph
        fails here, in the self-tests, with a clear message — rather than in the
        checker as "no parseable coverage claim", which reads as stale numbers."""
        readme = os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "README.md"
        )
        with open(readme) as f:
            text = f.read()
        matches = csd.README_COVERAGE_RE.findall(text)
        self.assertEqual(
            len(matches), 1,
            "README.md must carry exactly one parseable coverage claim; the guard "
            "reads the first match, so a second copy could go stale unnoticed",
        )


class TestSyncCoverageRepairsTheClaim(unittest.TestCase):
    """`--sync-coverage`: the repair path that keeps a bot's pin bump mergeable.

    Without it, invariant 7 reds every autobump PR: `sdk-autobump.yml` moves
    `.api-version`, the coverage sentence is measured against a pin, and the bot
    cannot know the new numbers — so the guard fails on a sentence nobody can fix
    from the diff, and the auto-merge armed for a non-breaking bump sits forever
    unmergeable.
    """

    TARGETED = TestReadmeCoverageClaimGuard.TARGETED
    AVAILABLE = TestReadmeCoverageClaimGuard.AVAILABLE

    def _run(self, sentence, write=False, spec_version="0.8.1"):
        readme, pin = readme_claiming(self, sentence)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme=readme,
                api_version_file=pin, write=write, spec_version=spec_version,
            )
        return errors, buf.getvalue(), readme

    def test_it_rewrites_a_stale_claim_and_reports_what_it_did(self):
        errors, out, readme = self._run(
            "**0 of 2** spec operations (**0.0%**), measured against the pinned "
            "`v0.7.2` spec",
            write=True,
        )
        self.assertEqual(errors, 0, out)
        self.assertIn("Rewrote", out)
        self.assertIn("0 of 2 (0.0%) against v0.7.2 -> 1 of 1 (100.0%) against v0.8.1", out)
        self.assertIn(
            "**1 of 1** spec operations (**100.0%**), measured against the pinned "
            "`v0.8.1` spec",
            open(readme).read(),
        )

    def test_the_repaired_file_then_passes_the_check(self):
        """The property that matters to the workflow: repair, then the gate is green
        on the very next run."""
        _, _, readme = self._run(
            "**0 of 2** spec operations (**0.0%**), measured against the pinned "
            "`v0.7.2` spec",
            write=True,
        )
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme=readme,
                api_version_file=os.path.join(os.path.dirname(readme), ".api-version"),
                spec_version="0.8.1",
            )
        self.assertEqual(errors, 0, buf.getvalue())

    def test_it_touches_nothing_but_the_four_values(self):
        """A writer that rebuilt the sentence would reformat human-owned prose and
        undo deliberate rewordings. It splices group-wise instead."""
        _, _, readme = self._run(
            "**0 of 2** spec operations (**0.0%**), measured against the pinned "
            "`v0.7.2` spec",
            write=True,
        )
        text = open(readme).read()
        self.assertIn("Untouched prose above.", text)
        self.assertIn("And prose after it.", text)
        self.assertIn("### API coverage", text)

    def test_it_preserves_a_reflowed_line_break(self):
        """The writer must not silently unwrap the paragraph it edits."""
        _, _, readme = self._run(
            "**0 of 2**\nspec operations (**0.0%**), measured against the pinned "
            "`v0.7.2` spec",
            write=True,
        )
        self.assertIn("**1 of 1**\nspec operations", open(readme).read())

    def test_a_true_claim_is_left_byte_identical(self):
        """Idempotent, so the bot's commit carries no diff when the ratio held."""
        readme, pin = readme_claiming(self, TestReadmeCoverageClaimGuard.CLAIM)
        before = open(readme).read()
        with contextlib.redirect_stdout(io.StringIO()):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE,
                readme=readme, api_version_file=pin, write=True,
                spec_version="0.8.1",
            )
        self.assertEqual(errors, 0)
        self.assertEqual(open(readme).read(), before)

    def test_an_unparseable_sentence_is_still_a_hard_failure(self):
        """A writer that cannot find its target must not invent one, or the repair
        mode becomes a way to lose the guard entirely."""
        errors, out, readme = self._run("no numbers here at all", write=True)
        self.assertEqual(errors, 1)
        self.assertIn("no parseable coverage claim", out)
        self.assertIn("no numbers here at all", open(readme).read())


class TestDeprecatedOpsLeaveTheRatio(unittest.TestCase):
    """Ported from the collector's `_spec_ops`, for the same reason `canonical_op`
    was ported from its `normalise_op`: the README claims both surfaces compute the
    same ratio under the same rule, and invariant 7 enforces that claim. `v0.8.1`
    deprecates nothing, so this filter is a no-op today — it is here because
    deprecating the legacy gateway mounts (ENG-4740) is the stated direction, and
    that is the moment an unported filter would start disagreeing.
    """

    def _spec(self, *ops, deprecated=()):
        spec = spec_of(*ops)
        for method, path in deprecated:
            spec["paths"][path][method.lower()]["deprecated"] = True
        return spec

    def test_a_deprecated_operation_leaves_the_denominator(self):
        spec = self._spec(
            ("GET", "/markets"), ("GET", "/legacy"), deprecated=[("GET", "/legacy")]
        )
        num, den, pct, _ = csd.coverage(
            [("GET", "/markets")], csd.spec_ops(spec), csd.deprecated_ops(spec)
        )
        self.assertEqual((num, den, pct), (1, 1, "100.0"))

    def test_without_the_filter_the_same_input_reads_50_percent(self):
        """Pins the difference the filter makes, so a silent removal is visible."""
        spec = self._spec(
            ("GET", "/markets"), ("GET", "/legacy"), deprecated=[("GET", "/legacy")]
        )
        num, den, _, _ = csd.coverage([("GET", "/markets")], csd.spec_ops(spec))
        self.assertEqual((num, den), (1, 2))

    def test_existence_still_sees_a_deprecated_operation(self):
        """The half that must NOT change: a deprecated operation is still mounted and
        still served, so targeting one is not drift. Dropping it from the existence
        set would report a live route as removed — the ENG-8463 mistake in reverse.
        """
        spec = self._spec(("GET", "/legacy"), deprecated=[("GET", "/legacy")])
        self.assertIn(("GET", "/legacy"), csd.spec_ops(spec))
        errors = _quiet(
            csd.check_targets_vs_spec,
            [("GET", "/legacy")], csd.spec_ops(spec), csd.deprecated_ops(spec),
        )
        self.assertEqual(errors, 0)

    def test_targeting_a_deprecated_operation_is_reported_not_counted(self):
        spec = self._spec(
            ("GET", "/markets"), ("GET", "/legacy"), deprecated=[("GET", "/legacy")]
        )
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            csd.check_targets_vs_spec(
                [("GET", "/markets"), ("GET", "/legacy")],
                csd.spec_ops(spec), csd.deprecated_ops(spec),
            )
        out = buf.getvalue()
        self.assertIn("1 of 1 spec operations (100.0% coverage)", out)
        self.assertIn("are DEPRECATED in the pinned spec", out)
        self.assertIn("GET /legacy", out)

    def test_a_deprecated_operation_is_not_a_coverage_gap(self):
        """It must not show up in the uncovered worklist either: nobody should be
        sent to wrap an operation that is being withdrawn."""
        spec = self._spec(
            ("GET", "/markets"), ("GET", "/legacy"), deprecated=[("GET", "/legacy")]
        )
        _, _, _, uncovered = csd.coverage(
            [("GET", "/markets")], csd.spec_ops(spec), csd.deprecated_ops(spec)
        )
        self.assertEqual(uncovered, {})

    def test_a_falsey_deprecated_flag_is_not_deprecated(self):
        """`deprecated: false` is the common spelling in generated specs."""
        spec = spec_of(("GET", "/markets"))
        spec["paths"]["/markets"]["get"]["deprecated"] = False
        self.assertEqual(csd.deprecated_ops(spec), set())

    def test_the_real_pinned_spec_shape_is_documented(self):
        """`endpoints.txt` must not target something the CLI is losing without the
        run saying so. Hermetic: asserts the wiring, not the live spec."""
        self.assertEqual(csd.deprecated_ops({"paths": {}}), set())


class TestCommandLineParsing(unittest.TestCase):
    """A mistyped flag must be a usage error, not a filename.

    `main` filtered `--sync-coverage` out of `argv` and treated everything left as
    positional, so `--sync-coverge` became the spec path and the run died with a
    `FileNotFoundError` traceback naming the typo as a missing file. The reader is
    then debugging a path that was never meant to be one.
    """

    def _run(self, *argv):
        return subprocess.run(
            [sys.executable, os.path.join(HERE, "check_spec_drift.py"), *argv],
            capture_output=True, text=True,
        )

    def test_a_mistyped_flag_is_a_usage_error(self):
        proc = self._run("--sync-coverge", "openapi.json")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("unrecognised option", proc.stderr)
        self.assertIn("usage:", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)

    def test_an_unknown_flag_alone_is_a_usage_error(self):
        proc = self._run("--help")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("usage:", proc.stderr)
        self.assertNotIn("Traceback", proc.stderr)

    def test_no_arguments_is_a_usage_error(self):
        proc = self._run()
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("usage:", proc.stderr)

    def test_two_positionals_is_a_usage_error(self):
        proc = self._run("a.json", "b.json")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("usage:", proc.stderr)

    def test_the_flag_may_come_after_the_path(self):
        """The control, and the reason the filter exists: three workflows and half
        the humans write `... openapi.json --sync-coverage`. That must keep working,
        so this cannot be fixed by demanding flags come first."""
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        spec = os.path.join(d.name, "openapi.json")
        with open(spec, "w") as f:
            json.dump(spec_of(("GET", "/markets"), version=PINNED_TAG.lstrip("v")), f)
        with open(os.path.join(d.name, "endpoints.txt"), "w") as f:
            f.write("GET /markets\n")
        with open(os.path.join(d.name, ".api-version"), "w") as f:
            f.write(PINNED_TAG + "\n")
        with open(os.path.join(d.name, "README.md"), "w") as f:
            f.write("Coverage: the CLI currently exercises **0 of 1** spec "
                    "operations (**0.0%**), measured against the pinned "
                    "`v0.8.1` spec.\n")
        proc = subprocess.run(
            [sys.executable, os.path.join(HERE, "check_spec_drift.py"),
             spec, "--sync-coverage"],
            cwd=d.name, capture_output=True, text=True,
        )
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
        self.assertIn("Rewrote", proc.stdout)


class TestTheClaimSurvivesCrlfLineEndings(unittest.TestCase):
    """A CRLF working copy must not read as a reworded sentence.

    The repo has no `.gitattributes` and ships Windows installers, so a contributor
    with `core.autocrlf=true` gets CRLF on checkout. The guard tolerated `\n` inside
    the sentence but not `\r\n`, so on that machine an unedited README failed as "no
    parseable coverage claim" — pointing them at a regex, for a file they never
    touched.
    """

    TARGETED = TestReadmeCoverageClaimGuard.TARGETED
    AVAILABLE = TestReadmeCoverageClaimGuard.AVAILABLE

    def _crlf_readme(self, sentence):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        readme = os.path.join(d.name, "README.md")
        body = (f"### API coverage\n\nPreamble.\n\nThe CLI currently exercises "
                f"{sentence}. Trailing prose.\n")
        with open(readme, "w", newline="") as f:
            f.write(body.replace("\n", "\r\n"))
        pin = os.path.join(d.name, ".api-version")
        with open(pin, "w") as f:
            f.write("v0.8.1\n")
        return readme, pin

    def _run(self, sentence, write=False):
        readme, pin = self._crlf_readme(sentence)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme=readme,
                api_version_file=pin, write=write, spec_version="0.8.1",
            )
        return errors, buf.getvalue(), readme

    def test_a_true_claim_on_a_crlf_file_passes(self):
        errors, out, _ = self._run(TestReadmeCoverageClaimGuard.CLAIM)
        self.assertEqual(errors, 0, out)

    def test_a_crlf_break_inside_the_sentence_is_still_one_paragraph(self):
        """The wrap case, in CRLF: Markdown reads a single line break inside a
        paragraph as a space regardless of how the line ends."""
        errors, out, _ = self._run(
            "**1 of 1** spec operations (**100.0%**), measured against\nthe "
            "pinned `v0.8.1` spec"
        )
        self.assertEqual(errors, 0, out)

    def test_a_blank_crlf_line_still_ends_the_paragraph(self):
        """The tolerance must stay at ONE break. A sentence split across two
        paragraphs is a rewording, which is what this guard is for."""
        errors, out, _ = self._run(
            "**1 of 1** spec operations (**100.0%**), measured against\n\nthe "
            "pinned `v0.8.1` spec"
        )
        self.assertEqual(errors, 1)
        self.assertIn("no parseable coverage claim", out)

    def test_the_repair_does_not_rewrite_every_line_ending(self):
        """The writer reads and writes with `newline=""`, so a four-value repair
        stays a four-value diff. In universal-newline mode it would translate the
        whole file and bury the edit in a diff touching every line."""
        _, out, readme = self._run(
            "**0 of 2** spec operations (**0.0%**), measured against the pinned "
            "`v0.8.1` spec",
            write=True,
        )
        raw = open(readme, "rb").read()
        self.assertIn(b"**1 of 1**", raw, out)
        self.assertGreater(raw.count(b"\r\n"), 0, "fixture must be CRLF to prove anything")
        self.assertEqual(
            raw.count(b"\n"), raw.count(b"\r\n"),
            "every line ending must still be CRLF — the repair rewrote the file",
        )


class TestSpecParsingMatchesTheCollector(unittest.TestCase):
    """`spec_ops` / `deprecated_ops` must read a spec the way the collector does.

    Both are ports, and both had gaps that agreed with the collector only because
    `v0.8.1` happens not to exercise them — the same latent-divergence shape the
    `deprecated` split was ported to prevent.
    """

    def test_head_and_options_are_counted(self):
        """`collect-interfaces-metrics.py:103` includes both. Omitting them here
        would move the dashboard's denominator without moving ours, while invariant
        7 pinned the README to ours."""
        spec = {"info": {"version": "0.0.0-test"}, "paths": {
            "/thing": {
                "get": {"responses": {}},
                "head": {"responses": {}},
                "options": {"responses": {}},
            }
        }}
        self.assertEqual(
            csd.spec_ops(spec),
            {("GET", "/thing"), ("HEAD", "/thing"), ("OPTIONS", "/thing")},
        )

    def test_the_full_method_set_is_the_collectors(self):
        self.assertEqual(
            set(csd.HTTP_METHODS),
            {"get", "post", "put", "patch", "delete", "head", "options"},
        )

    def test_a_non_dict_path_item_is_skipped_not_crashed(self):
        """A `$ref` string or a hand-edited null used to raise AttributeError, so
        the checker reported nothing at all about the spec it was pointed at."""
        spec = {"info": {"version": "0.0.0-test"}, "paths": {
            "/good": {"get": {"responses": {}}},
            "/ref": "#/components/pathItems/Thing",
            "/null": None,
        }}
        self.assertEqual(csd.spec_ops(spec), {("GET", "/good")})
        self.assertEqual(csd.deprecated_ops(spec), set())

    def test_a_null_paths_member_is_tolerated(self):
        """`(spec.get("paths") or {})`, from the collector: an explicit null is not
        the same as an absent key, and only one of them used to work."""
        for paths in (None, {}):
            with self.subTest(paths=paths):
                self.assertEqual(csd.spec_ops({"paths": paths}), set())
                self.assertEqual(csd.deprecated_ops({"paths": paths}), set())

    def test_non_operation_members_are_still_ignored(self):
        """The control: `parameters` and `summary` are legal path-item members and
        must not become operations just because the method filter widened."""
        spec = {"info": {"version": "0.0.0-test"}, "paths": {
            "/thing": {
                "get": {"responses": {}},
                "parameters": [{"name": "id", "in": "path"}],
                "summary": "not an operation",
            }
        }}
        self.assertEqual(csd.spec_ops(spec), {("GET", "/thing")})


class TestInvariant7RefusesASpecThatIsNotThePinnedOne(unittest.TestCase):
    """The ratio and the tag must come from the same release.

    Invariant 7 builds its sentence from two independent sources: the ratio from
    whatever spec document the run was handed, and the tag from `.api-version`.
    Nothing tied them together, so pointing the checker at a spec for a DIFFERENT
    release produced a sentence pairing this pin with that release's numbers.

    In `--sync-coverage` mode that is the serious one: a false claim written into
    the README by the tool, at exit 0. In check mode it is a false "claim is stale"
    whose suggested remedy is the command that corrupts the sentence.

    It is also the normal case rather than a contrived one: `openapi.pinned.json` is
    gitignored and the README tells you to reuse that filename, so a stale local
    copy is exactly what a contributor will have on disk.
    """

    TARGETED = TestReadmeCoverageClaimGuard.TARGETED
    AVAILABLE = TestReadmeCoverageClaimGuard.AVAILABLE
    CLAIM = TestReadmeCoverageClaimGuard.CLAIM

    def _run(self, spec_version, write=False, claim=None):
        readme, pin = readme_claiming(self, self.CLAIM if claim is None else claim)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = csd.check_readme_coverage_claim(
                self.TARGETED, self.AVAILABLE, readme=readme,
                api_version_file=pin, write=write, spec_version=spec_version,
            )
        return errors, buf.getvalue(), readme

    def test_a_spec_from_another_release_fails(self):
        errors, out, _ = self._run("0.7.2")
        self.assertEqual(errors, 1)
        self.assertIn("0.7.2", out)
        self.assertIn("v0.8.1", out)
        # The message must name the mismatch, not the sentence: the README is not
        # what is wrong here, and "claim is stale" would send the reader to edit it.
        self.assertNotIn("claim is stale", out)

    def test_sync_coverage_refuses_to_write_from_the_wrong_spec(self):
        """The one that matters. This case used to exit 0 having written a false
        sentence — a guard turned into a corruption path."""
        before = None
        errors, out, readme = self._run(
            "0.7.2",
            write=True,
            claim="**0 of 2** spec operations (**0.0%**), measured against the "
                  "pinned `v0.7.2` spec",
        )
        self.assertEqual(errors, 1, out)
        self.assertNotIn("Rewrote", out)
        # And the file is untouched: refusing but writing anyway would be worse than
        # not checking, because the exit code would then contradict the diff.
        text = open(readme).read()
        self.assertIn("**0 of 2**", text)
        self.assertNotIn("**1 of 1**", text)
        del before

    def test_the_matching_release_still_passes(self):
        """The control. Without it every case above could pass by refusing
        everything, which is the failure mode a correspondence check invites."""
        errors, out, _ = self._run("0.8.1")
        self.assertEqual(errors, 0, out)

    def test_the_tag_v_prefix_is_not_load_bearing(self):
        """`.api-version` is a git tag (`v0.8.1`); `info.version` is bare semver
        (`0.8.1`). They name one release, and the guard must not read the `v` as a
        difference — that would fail every correct run."""
        for declared in ("0.8.1", "v0.8.1"):
            with self.subTest(declared=declared):
                errors, out, _ = self._run(declared)
                self.assertEqual(errors, 0, out)

    def test_a_spec_declaring_no_version_fails_closed(self):
        """Unknown is not "assume it matches". A spec with no `info.version` cannot
        be shown to be the pinned one, and a guard that cannot verify must not
        pass — the same rule the unparseable-sentence branch follows."""
        for missing in (None, ""):
            with self.subTest(missing=missing):
                errors, out, _ = self._run(missing)
                self.assertEqual(errors, 1)
                self.assertIn("not declared", out)

    def test_the_failure_names_the_command_that_fixes_it(self):
        """A remedy that is a fetch, not an edit: the spec on disk is wrong, so
        pointing at the README (or at --sync-coverage) would be the corruption."""
        _, out, _ = self._run("0.7.2")
        self.assertIn("openapi.pinned.json", out)
        self.assertIn("nexus-exchange-api/v0.8.1/openapi.json", out)


class TestInvariant7IsSkippedWhileInvariant1Fails(unittest.TestCase):
    """No cascading second error, and no double-counted exit status.

    Invariant 7 recomputes the numerator from `set(targeted) & available`, so any
    invariant-1 failure — a renamed or removed spec endpoint — also moves the ratio
    and made this check fire a second "the README's coverage claim is stale". That
    sends the reader to edit a sentence when the fix is the endpoint above, and the
    same root cause was counted twice in `failures`.

    Driven through `main()` as a subprocess, because the composition IS the
    behaviour under test: the two checks are individually right, and it was their
    wiring that produced the misleading message.

    NOT hermetic, despite the temp cwd: `CLI_SOURCES` and `SRC_DIR` are absolute, so
    invariants 2-4 read the real `src/` inside these runs and fail noisily there.
    That is harmless — every assertion below is on a specific substring or on
    `returncode`, neither of which those failures can forge — but the earlier
    docstring called this hermetic, and a test that misstates what it isolates is
    the kind of claim this PR exists to stop trusting. No network either way.
    """

    def _run(self, spec_paths, endpoints, claim):
        d = tempfile.TemporaryDirectory()
        self.addCleanup(d.cleanup)
        spec = os.path.join(d.name, "openapi.json")
        with open(spec, "w") as f:
            # Declares the release the `.api-version` written below pins: invariant 7
            # refuses a spec/pin mismatch, and that refusal is a different branch
            # from the skip-vs-check composition under test here.
            json.dump(spec_of(*spec_paths, version=PINNED_TAG.lstrip("v")), f)
        with open(os.path.join(d.name, "endpoints.txt"), "w") as f:
            f.write("\n".join(f"{m} {p}" for m, p in endpoints) + "\n")
        with open(os.path.join(d.name, ".api-version"), "w") as f:
            f.write(PINNED_TAG + "\n")
        with open(os.path.join(d.name, "README.md"), "w") as f:
            f.write(f"Coverage: the CLI currently exercises {claim}.\n")
        proc = subprocess.run(
            [sys.executable, os.path.join(HERE, "check_spec_drift.py"), spec],
            cwd=d.name, capture_output=True, text=True,
        )
        return proc

    # One operation in the spec, one targeted line naming something else.
    RENAMED = ([("GET", "/markets")], [("GET", "/markets-renamed")])
    CLAIM_1_OF_1 = ("**1 of 1** spec operations (**100.0%**), measured against the "
                    "pinned `v0.8.1` spec")

    def test_a_missing_endpoint_does_not_also_report_the_readme_as_stale(self):
        proc = self._run(*self.RENAMED, claim=self.CLAIM_1_OF_1)
        self.assertIn("are NOT in the spec", proc.stdout)
        self.assertIn("SKIPPED (invariant 7)", proc.stdout)
        self.assertNotIn("coverage claim is stale", proc.stdout)
        self.assertNotEqual(proc.returncode, 0, "invariant 1 must still fail the run")

    def test_the_skip_says_which_check_to_re_run_and_why(self):
        """A skipped guard that says nothing is indistinguishable from a guard that
        was quietly dropped."""
        proc = self._run(*self.RENAMED, claim=self.CLAIM_1_OF_1)
        self.assertIn("invariant 1 is failing", proc.stdout)
        self.assertIn("Re-run once the endpoint is resolved", proc.stdout)

    def test_with_invariant_1_green_the_claim_is_checked_for_real(self):
        """The skip must be conditional, not a way to stop checking: same tree, a
        targeted line that exists, and a stale claim is caught."""
        proc = self._run(
            [("GET", "/markets")], [("GET", "/markets")],
            claim="**0 of 1** spec operations (**0.0%**), measured against the "
                  "pinned `v0.8.1` spec",
        )
        self.assertNotIn("SKIPPED (invariant 7)", proc.stdout)
        self.assertIn("coverage claim is stale", proc.stdout)
        self.assertNotEqual(proc.returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
