#!/usr/bin/env python3
"""Tests for render_catalog_pin_body.py. Run: python3 scripts/test_render_catalog_pin_body.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from render_catalog_pin_body import render  # noqa: E402


def entry(path, summary="s"):
    return {"path": path, "summary": summary}


class RenderTest(unittest.TestCase):
    def test_lists_added_removed_and_changed(self):
        old = {"a/x": entry("a/x"), "a/y": entry("a/y"), "a/z": entry("a/z")}
        new = {"a/x": entry("a/x"), "a/y": entry("a/y", "new"), "a/w": entry("a/w")}
        body = render("catalog-2026.09.01", "catalog-2026.09.24", old, new, "will-run")
        self.assertIn("**Added:** `a/w`", body)
        self.assertIn("**Removed:** `a/z`", body)
        self.assertIn("**Changed entries:** `a/y`", body)
        self.assertIn("tree/catalog-2026.09.24", body)
        self.assertIn("PR checks run", body)

    def test_says_when_only_the_pin_moved(self):
        same = {"a/x": entry("a/x")}
        body = render("untagged", "catalog-2026.09.24", same, same, "will-not-run")
        self.assertIn("only the pin moved", body)
        self.assertIn("PR checks did not run", body)


if __name__ == "__main__":
    unittest.main()
