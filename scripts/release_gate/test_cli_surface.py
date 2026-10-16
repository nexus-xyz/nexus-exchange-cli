#!/usr/bin/env python3
"""Tests for cli_surface.py (ENG-18798). Stdlib unittest.

No binary needed: the help text is synthetic, and walk() reads it through a stubbed run_help(), so
the parser, the walk and the fold are tested on exactly the input they are given. public_api.sh runs
this before it lists the real binary, so `prepublish-surface` fails on a broken generator before it
can fail, or pass, on the listing.

Run: python3 scripts/release_gate/test_cli_surface.py
"""

import importlib.util
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("cli_surface", HERE / "cli_surface.py")
S = importlib.util.module_from_spec(spec)
spec.loader.exec_module(S)

# A three-command tree shaped like clap's `-h` output: a flag at indent 6 with its description on
# the next line, deeper. `--output` and `-h` are on every command, as `global = true` flags are;
# `--quiet` is on `nexus` and `nexus market` but not on `nexus market summary` or `nexus orders`.
ROOT = """\
Command-line interface for a test exchange

Usage: nexus [OPTIONS] <COMMAND>

Commands:
  market  Per-market data
  orders  List your open orders
  help    Print this message or the help of the given subcommand(s)

Options:
      --output <OUTPUT>
          Output format [env: NEXUS_OUTPUT=json] [default: human] [possible values: human, json]
      --quiet
          Less output
  -h, --help
          Print help
"""

MARKET = """\
Per-market data

Usage: nexus market [OPTIONS] <COMMAND>

Commands:
  summary  Per-market summaries
  help     Print this message or the help of the given subcommand(s)

Options:
      --output <OUTPUT>
          Output format [env: NEXUS_OUTPUT=] [default: human] [possible values: human, json]
      --quiet
          Less output
  -h, --help
          Print help
"""

SUMMARY = """\
Per-market summaries

Usage: nexus market summary [OPTIONS]

Options:
      --output <OUTPUT>
          Output format [env: NEXUS_OUTPUT=] [default: human] [possible values: human, json]
  -h, --help
          Print help
"""

ORDERS = """\
List your open orders

Usage: nexus orders [OPTIONS] <MARKET>

Arguments:
  <MARKET>  Market id

Options:
      --output <OUTPUT>
          Output format [env: NEXUS_OUTPUT=] [default: human] [possible values: human, json]
  -h, --help
          Print help
"""

TREE = {
    (): ROOT,
    ("market",): MARKET,
    ("market", "summary"): SUMMARY,
    ("orders",): ORDERS,
}

OUTPUT_FLAG = "--output <OUTPUT> [env: NEXUS_OUTPUT] [default: human] [possible values: human, json]"


def listing(tree):
    """The lines cli_surface.py prints for `tree`, a {command path: help text} map."""
    with mock.patch.object(S, "run_help", lambda binary, path, home: tree[tuple(path)]):
        entries = S.walk("nexus", "/nonexistent")
    return [line for _, line in sorted(S.fold_common(entries))]


class ParseHelp(unittest.TestCase):
    def test_reads_usage_sections_and_spec_values(self):
        usage, sections = S.parse_help(ROOT, "`nexus -h`")
        self.assertEqual(usage, ["nexus [OPTIONS] <COMMAND>"])
        self.assertEqual([spec for spec, _ in sections["Commands"]], ["market", "orders", "help"])
        self.assertIn(
            ("--output <OUTPUT>",
             ["[env: NEXUS_OUTPUT]", "[default: human]", "[possible values: human, json]"]),
            sections["Options"],
        )

    def test_a_line_outside_any_section_fails(self):
        text = "Usage: nexus [OPTIONS]\n\nsomething clap never prints\n"
        with self.assertRaisesRegex(S.HelpFormatError, "a line outside any section: 'something clap"):
            S.parse_help(text, "`nexus -h`")

    def test_an_indented_line_before_any_section_fails(self):
        text = "Usage: nexus [OPTIONS]\n\n  --stray\n"
        with self.assertRaisesRegex(S.HelpFormatError, "a line outside any section"):
            S.parse_help(text, "`nexus -h`")

    def test_no_usage_line_fails(self):
        text = "Command-line interface\n\nOptions:\n  -h, --help  Print help\n"
        with self.assertRaisesRegex(S.HelpFormatError, r"`nexus -h`: no `Usage:` line"):
            S.parse_help(text, "`nexus -h`")


class Walk(unittest.TestCase):
    def test_an_item_it_cannot_place_fails(self):
        tree = dict(TREE)
        tree[("orders",)] = ORDERS.replace("Options:\n", "Options:\n  verbose  Not a flag\n")
        with self.assertRaisesRegex(
            S.HelpFormatError, r"`nexus orders -h`: cannot read 'verbose' in section 'Options'"
        ):
            listing(tree)


class Aliases(unittest.TestCase):
    # `visible_alias = "sum"` on `market summary`: clap appends `[aliases: sum]` to its line in the
    # parent's `Commands:` section. A plain `alias` prints nothing, so the listing cannot see it.
    ALIASED = {**TREE, ("market",): MARKET.replace(
        "  summary  Per-market summaries\n", "  summary  Per-market summaries [aliases: sum]\n")}

    def test_an_alias_is_listed_on_its_command_line(self):
        lines = listing(self.ALIASED)
        self.assertIn("nexus market summary [aliases: sum]", lines)
        self.assertNotIn("nexus market summary", lines)

    def test_an_alias_is_not_walked_as_a_command_of_its_own(self):
        walked = []

        def run_help(binary, path, home):
            walked.append(tuple(path))
            return self.ALIASED[tuple(path)]

        with mock.patch.object(S, "run_help", run_help):
            S.walk("nexus", "/nonexistent")
        self.assertEqual(sorted(walked), sorted(TREE))
        # Nothing else moves: the alias changes the one line and adds none.
        self.assertEqual(
            [line for line in listing(self.ALIASED) if line != "nexus market summary [aliases: sum]"],
            [line for line in listing(TREE) if line != "nexus market summary"],
        )


class FoldCommon(unittest.TestCase):
    def test_a_flag_on_every_command_is_listed_once(self):
        lines = listing(TREE)
        self.assertEqual(lines.count(f"nexus (every command)  {OUTPUT_FLAG}"), 1)
        self.assertIn("nexus (every command)  -h, --help", lines)
        # ...and on no command of its own.
        self.assertEqual([line for line in lines if "--output" in line],
                         [f"nexus (every command)  {OUTPUT_FLAG}"])

    def test_a_flag_missing_from_one_command_does_not_fold(self):
        lines = listing(TREE)
        self.assertNotIn("nexus (every command)  --quiet", lines)
        self.assertEqual([line for line in lines if "--quiet" in line],
                         ["nexus  --quiet", "nexus market  --quiet"])

    def test_the_whole_listing(self):
        self.assertEqual(listing(TREE), [
            f"nexus (every command)  {OUTPUT_FLAG}",
            "nexus (every command)  -h, --help",
            "nexus",
            "nexus  --quiet",
            "nexus  usage: [OPTIONS] <COMMAND>",
            "nexus market",
            "nexus market  --quiet",
            "nexus market  usage: [OPTIONS] <COMMAND>",
            "nexus market summary",
            "nexus market summary  usage: [OPTIONS]",
            "nexus orders",
            "nexus orders  <MARKET>",
            "nexus orders  usage: [OPTIONS] <MARKET>",
        ])


if __name__ == "__main__":
    unittest.main()
