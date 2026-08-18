#!/usr/bin/env python3
"""Self-test for the SDK-parity checks: prove they go RED when defeated (ENG-7962).

`check_sdk_parity.py` is what makes the spec pin a fact rather than a claim, so it
gets the same treatment as the drift checker: every invariant is defeated here and
asserted to fail. Companion to `test_check_spec_drift.py`.

The headline case is `test_wrong_verb_is_caught` — the ENG-7962 bug reproduced
against a synthetic SDK manifest. That bug survived because the only checker looked
at the *spec*, which defines both PATCH and PUT operations in general, and neither
file agreed with the other. Comparing against the SDK's own manifest kills it: the
SDK lists no PUT on that path, so the mapping cannot be right.

Hermetic: the check functions take the SDK's data as arguments, so nothing here
touches crates.io. `TestSdkCrateParsers` covers the plumbing that does, using
temp files rather than the network.

Run: python3 scripts/test_sdk_parity.py   (stdlib unittest; no pytest needed)
"""
import contextlib
import io
import os
import sys
import tempfile
import textwrap
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import check_sdk_parity as parity  # noqa: E402
import sdk_crate  # noqa: E402


def _quiet(fn, *args, **kwargs):
    with contextlib.redirect_stdout(io.StringIO()):
        return fn(*args, **kwargs)


# A stand-in for the SDK's endpoints.txt: what the crate wraps.
SDK_OPS = {
    ("GET", "/markets"),
    ("GET", "/api/v1/positions"),
    ("GET", "/orders/{order_id}"),
    ("PATCH", "/orders/{order_id}"),
    ("GET", "/api/v1/bridge/assets"),  # wrapped by the SDK, no CLI command
}


class TestPinParity(unittest.TestCase):
    """Invariant 5: our pin == the crate's pin."""

    def test_equal_pins_pass(self):
        self.assertEqual(_quiet(parity.check_pin, "0.6.0", "v0.7.1", "v0.7.1"), 0)

    def test_pin_ahead_of_the_crate_fails(self):
        """The case that motivated the whole redesign: a spec release tempts you to
        advance the pin, but the binary still sends the crate's tag, so the claim is
        false."""
        self.assertEqual(_quiet(parity.check_pin, "0.6.0", "v0.7.1", "v0.7.2"), 1)

    def test_pin_behind_the_crate_fails(self):
        """Equally wrong in the other direction — the dependency moved and the pin
        didn't follow."""
        self.assertEqual(_quiet(parity.check_pin, "0.7.0", "v0.7.2", "v0.7.1"), 1)

    def test_the_two_directions_give_different_advice(self):
        """Ahead and behind need opposite fixes, so the message must distinguish
        them — 'wait for a crate release' vs 'run the sync script'."""
        ahead = io.StringIO()
        with contextlib.redirect_stdout(ahead):
            parity.check_pin("0.6.0", "v0.7.1", "v0.7.2")
        behind = io.StringIO()
        with contextlib.redirect_stdout(behind):
            parity.check_pin("0.7.0", "v0.7.2", "v0.7.1")
        self.assertIn("AHEAD", ahead.getvalue())
        self.assertIn("BEHIND", behind.getvalue())
        self.assertNotIn("AHEAD", behind.getvalue())


class TestManifestSubset(unittest.TestCase):
    """Invariant 6: endpoints.txt is a subset of what the crate wraps."""

    def test_subset_passes(self):
        ours = [("GET", "/markets"), ("PATCH", "/orders/{order_id}")]
        self.assertEqual(
            _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours), 0
        )

    def test_wrong_verb_is_caught(self):
        """THE ENG-7962 REGRESSION. `amend_order` was mapped to PUT; the SDK wraps
        PATCH on that path and no PUT. A spec-only check could not see this, because
        the spec defines PATCH there and PUT elsewhere — only the SDK's own manifest
        settles it."""
        ours = [("GET", "/markets"), ("PUT", "/orders/{order_id}")]
        errs = _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours)
        self.assertEqual(errs, 1)

    def test_wrong_verb_message_names_the_real_verb(self):
        """The error has to be actionable — say what the SDK actually issues, or the
        reader has to go read the SDK source (which is how the bug lasted)."""
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            parity.check_manifest_subset(
                "0.6.0", SDK_OPS, [("PUT", "/orders/{order_id}")]
            )
        msg = buf.getvalue()
        self.assertIn("PATCH", msg)
        self.assertIn("not PUT", msg)

    def test_path_the_sdk_does_not_wrap_at_all_fails(self):
        ours = [("GET", "/markets"), ("GET", "/invented")]
        errs = _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours)
        self.assertEqual(errs, 1)

    def test_path_not_wrapped_message_differs_from_wrong_verb(self):
        """Two different fixes: 'fix the verb' vs 'remove the line or land the
        wrapper upstream'. Collapsing them into one message would send a reader to
        the wrong place."""
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            parity.check_manifest_subset("0.6.0", SDK_OPS, [("GET", "/invented")])
        self.assertIn("does not wrap this path at all", buf.getvalue())

    def test_placeholder_names_do_not_matter(self):
        """The CLI writes `{order_id}` and the SDK might write `{id}`; comparing
        literally would produce a phantom failure."""
        ours = [("PATCH", "/orders/{id}")]
        self.assertEqual(
            _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours), 0
        )

    def test_websocket_upgrade_is_exempt(self):
        """`GET /ws` is opened by the streaming client, not a REST wrapper, so it is
        legitimately absent from the SDK's REST manifest. It must not be reported as
        unreachable — that would be a permanent false failure."""
        ours = [("GET", "/markets"), ("GET", "/ws")]
        self.assertEqual(
            _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours), 0
        )

    def test_sdk_extras_are_not_failures(self):
        """The SDK wraps more than the CLI exposes. That is a coverage gap to
        report, never an error — equality here would force the CLI to surface
        everything the SDK can do."""
        ours = [("GET", "/markets")]
        self.assertEqual(
            _quiet(parity.check_manifest_subset, "0.6.0", SDK_OPS, ours), 0
        )


class TestSdkCrateParsers(unittest.TestCase):
    """The plumbing that reads Cargo.lock / Cargo.toml. Uses temp files, no network."""

    @contextlib.contextmanager
    def _files(self, lock=None, toml=None):
        with tempfile.TemporaryDirectory() as d:
            orig_lock, orig_toml = sdk_crate.CARGO_LOCK, sdk_crate.CARGO_TOML
            if lock is not None:
                sdk_crate.CARGO_LOCK = os.path.join(d, "Cargo.lock")
                with open(sdk_crate.CARGO_LOCK, "w") as f:
                    f.write(textwrap.dedent(lock))
            if toml is not None:
                sdk_crate.CARGO_TOML = os.path.join(d, "Cargo.toml")
                with open(sdk_crate.CARGO_TOML, "w") as f:
                    f.write(textwrap.dedent(toml))
            try:
                yield
            finally:
                sdk_crate.CARGO_LOCK, sdk_crate.CARGO_TOML = orig_lock, orig_toml

    def test_locked_version_read_from_the_lockfile(self):
        with self._files(
            lock="""
            [[package]]
            name = "anyhow"
            version = "1.0.0"

            [[package]]
            name = "nexus-exchange"
            version = "0.7.0"
            """
        ):
            self.assertEqual(sdk_crate.locked_version(), "0.7.0")

    def test_lock_is_authoritative_not_the_toml_requirement(self):
        """`"0.6.0"` in Cargo.toml means `^0.6.0`, which permits 0.6.1 — so the two
        legitimately differ and only the lockfile says what builds. Reading the
        requirement instead would compare the pin against a version we don't use."""
        with self._files(
            lock="""
            [[package]]
            name = "nexus-exchange"
            version = "0.6.1"
            """,
            toml="""
            [dependencies]
            nexus-exchange = "0.6.0"
            """,
        ):
            self.assertEqual(sdk_crate.locked_version(), "0.6.1")
            self.assertEqual(sdk_crate.required_version(), "0.6.0")

    def test_missing_dependency_fails_closed(self):
        with self._files(lock='[[package]]\nname = "anyhow"\nversion = "1.0.0"\n'):
            with self.assertRaises(SystemExit):
                sdk_crate.locked_version()

    def test_two_resolved_versions_fails_closed(self):
        """If the graph ever links two majors, 'the pin' has no single meaning."""
        with self._files(
            lock="""
            [[package]]
            name = "nexus-exchange"
            version = "0.6.0"

            [[package]]
            name = "nexus-exchange"
            version = "0.7.0"
            """
        ):
            with self.assertRaises(SystemExit):
                sdk_crate.locked_version()

    def test_table_form_dependency_is_read(self):
        with self._files(
            toml="""
            [dependencies]
            nexus-exchange = { version = "0.7.0", features = ["ws"] }
            """
        ):
            self.assertEqual(sdk_crate.required_version(), "0.7.0")

    def test_non_plain_requirement_fails_closed(self):
        """A range, git or path dependency means the pin is not derivable; guessing
        would produce a confident wrong answer."""
        for dep in ('">=0.6, <0.8"', '{ git = "https://example.com/x" }',
                    '{ path = "../nexus-exchange-rs" }'):
            with self.subTest(dep=dep):
                with self._files(
                    toml=f"[dependencies]\nnexus-exchange = {dep}\n"
                ):
                    with self.assertRaises(SystemExit):
                        sdk_crate.required_version()

    def test_version_key_orders_correctly(self):
        k = sdk_crate.version_key
        self.assertLess(k("0.6.0"), k("0.7.0"))
        self.assertLess(k("v0.7.1"), k("v0.7.2"))
        self.assertLess(k("0.6.0"), k("0.6.1"))
        # Padding: v0.7 and v0.7.0 are the same version.
        self.assertEqual(k("v0.7"), k("v0.7.0"))


class TestRealRepoParity(unittest.TestCase):
    """Against the committed files. Reads Cargo.lock/Cargo.toml only — no network."""

    def test_lock_and_requirement_are_consistent_today(self):
        """Not an invariant the checker enforces (a caret bump is legitimate), but if
        they diverge it is worth knowing while reading test output."""
        self.assertEqual(
            sdk_crate.version_key(sdk_crate.locked_version()),
            sdk_crate.version_key(sdk_crate.required_version()),
            "Cargo.lock has floated away from the Cargo.toml requirement; harmless, "
            "but check_sdk_parity compares the pin against the LOCKED version",
        )


class TestReadmePinGuard(unittest.TestCase):
    """ENG-10956: AGENTS.md promised this guard; nothing implemented it.

    `check_pin` compares `.api-version` to the crate's own pin. Nothing compared
    the README's *crate version* to `Cargo.lock`, so a hand-bump that skipped
    `sync_sdk_version.py --write` shipped a README a release behind, green.
    """

    LINE = (
        "Currently targets Exchange API spec **`{tag}`** — the version pinned and "
        "sent as `X-Nexus-Api-Version` by `nexus-exchange` **`{crate}`**."
    )

    def _run(self, body, locked="0.9.1", our_tag="v0.8.1"):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "README.md")
            with open(path, "w") as f:
                f.write(body)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                errors = parity.check_readme_pin(locked, our_tag, readme=path)
            return errors, buf.getvalue()

    def test_an_agreeing_line_passes(self):
        errors, out = self._run(self.LINE.format(tag="v0.8.1", crate="0.9.1"))
        self.assertEqual(errors, 0)
        self.assertIn("matches the tree", out)

    def test_a_stale_crate_version_fails(self):
        """The exact #60 shape: crate bumped, README left behind."""
        errors, out = self._run(self.LINE.format(tag="v0.8.1", crate="0.9.0"))
        self.assertEqual(errors, 1)
        self.assertIn("disagrees with the tree", out)
        self.assertIn("sync_sdk_version.py --write", out)

    def test_a_stale_spec_tag_fails(self):
        errors, out = self._run(self.LINE.format(tag="v0.7.2", crate="0.9.1"))
        self.assertEqual(errors, 1)
        self.assertIn("disagrees with the tree", out)

    def test_a_missing_line_fails_rather_than_skips(self):
        """A guard that stops matching must not silently stop guarding."""
        errors, out = self._run("# README\n\nNo managed block here.\n")
        self.assertEqual(errors, 1)
        self.assertIn("no parseable api-version-sync line", out)

    def test_the_real_readme_matches_the_real_tree(self):
        """So the synthetic cases above cannot stay green while the tree drifts."""
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            errors = parity.check_readme_pin(
                sdk_crate.locked_version(),
                open(os.path.join(parity.csd.REPO, ".api-version")).read().strip(),
            )
        self.assertEqual(errors, 0, buf.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
