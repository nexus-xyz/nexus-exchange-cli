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

The counterpart in nexus-exchange-rs is the same-named script. Each of the four
invariants gets at least one test that defeats it and asserts a non-zero error
count, plus regression tests pinning the ENG-7962 fix in the real repo files so
the synthetic fixtures cannot pass while reality drifts out from under them.

Hermetic: no network, no pinned-spec download, no writes outside a temp dir.

Run: python3 scripts/test_check_spec_drift.py   (stdlib unittest; no pytest needed)
"""
import contextlib
import io
import os
import sys
import tempfile
import textwrap
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check_spec_drift as csd  # noqa: E402


def _quiet(fn, *args, **kwargs):
    """Run a check fn, swallowing its stdout; return its error count."""
    with contextlib.redirect_stdout(io.StringIO()):
        return fn(*args, **kwargs)


def spec_of(*ops):
    """Build a minimal OpenAPI doc covering exactly `ops` (METHOD, path) pairs."""
    paths = {}
    for method, path in ops:
        paths.setdefault(path, {})[method.lower()] = {"responses": {}}
    return {"info": {"version": "0.0.0-test"}, "paths": paths}


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


if __name__ == "__main__":
    unittest.main(verbosity=2)
