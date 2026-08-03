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

    def test_code_only_allowlist_suppresses_an_ahead_of_spec_op(self):
        """The legitimate use: the op is implemented but the pinned spec lacks it,
        so it stays out of endpoints.txt."""
        repo = self._repo({"main.rs": RUST_CALLING_BOTH}, ["GET /markets"])
        self.assertEqual(repo.check(spec_of(MARKETS), code_only={POSITIONS}), 0)

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

    def test_code_only_entry_no_command_calls_fails(self):
        repo = self._repo({"main.rs": RUST_CALLING_MARKETS}, ["GET /markets"])
        self.assertGreater(
            repo.check(spec_of(MARKETS), code_only={("GET", "/gone")}), 0
        )

    def test_code_only_entry_the_spec_caught_up_with_fails(self):
        """Same verb, same path: the op is no longer ahead of the pin, so the
        exemption now hides a covered operation and coverage under-reports."""
        repo = self._repo({"main.rs": RUST_CALLING_BOTH}, ["GET /markets"])
        self.assertGreater(
            repo.check(spec_of(MARKETS, POSITIONS), code_only={POSITIONS}), 0
        )

    def test_code_only_entry_with_a_wrong_verb_fails(self):
        """The ENG-7962 regression, reproduced in miniature. The mapping claims a
        verb the spec does not model, but the spec DOES define the path — so the
        entry is a mis-mapping, not an ahead-of-spec op. Matching the allowlist
        against the spec by (method, path) would pass this; matching by path
        catches it."""
        repo = self._repo({"main.rs": RUST_CALLING_MARKETS}, ["GET /markets"])
        # The spec has GET /markets, not PUT /markets.
        self.assertGreater(
            repo.check(spec_of(MARKETS), code_only={("PUT", "/markets")}), 0
        )

    def test_genuinely_ahead_of_spec_entry_still_passes(self):
        """The check must not fire on the legitimate case — a path the spec does
        not define at all."""
        repo = self._repo({"main.rs": RUST_CALLING_BOTH}, ["GET /markets"])
        self.assertEqual(repo.check(spec_of(MARKETS), code_only={POSITIONS}), 0)

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

    def test_no_code_only_entry_shares_a_path_with_endpoints_txt(self):
        """Cheap invariant on the real files: an op cannot be both claimed and
        exempted."""
        targeted = {(m, csd.normalize_path(p)) for m, p in csd.load_targeted()}
        overlap = sorted(csd.CODE_ONLY_OPS & targeted)
        self.assertEqual(overlap, [], f"claimed AND exempted: {overlap}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
