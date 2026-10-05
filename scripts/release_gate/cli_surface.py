#!/usr/bin/env python3
"""List the public surface of a built `nexus` binary (ENG-18798): its commands, arguments and flags.

    python3 scripts/release_gate/cli_surface.py <path to nexus>

Prints one line per item, sorted by command path and then by item, so `public_api.sh` can diff it
against the committed `public-api.txt`. Each command contributes:

    nexus account margin add                    the command exists (plus its aliases, if any)
    nexus account margin add  --yes             a flag, with its value placeholder if it takes one
    nexus account margin add  <AMOUNT>          a positional argument
    nexus account margin add  usage: [OPTIONS] <MARKET_ID> <AMOUNT>

A flag or argument keeps what a caller depends on: `[env: NAME]`, `[default: ...]`,
`[possible values: ...]` and `[aliases: ...]`. The usage line carries what the per-item lines
cannot: which flags are required and the order of the positionals. So a flag that starts taking a
value, a default that moves or a flag that becomes required fails the check, not only a dropped
command.

A flag that every command accepts (the `global = true` ones, and `-h`) is listed once, as
`nexus (every command)  <flag>`, instead of once per command, where it was most of the file and
turned one global flag into a diff of a hundred lines. Nothing is lost: if a flag stops being
global, that line goes and the commands that still take it list it themselves.

WHY THE HELP TEXT AND NOT `nexus completions`. The binary is the published artifact, so the surface
comes from running it, not from reading src/cli.rs. The completion scripts were the first choice,
but clap_complete's fish script stops at two levels (nothing under `account margin add`) and lists
no positional arguments. `-h` on every command reaches the whole tree and prints all of it, one
line per item (no `wrap_help` feature, so nothing wraps; a continuation line is folded in anyway).

WHAT IS LEFT OUT, ON PURPOSE. The descriptions, which are prose and would make every wording
change fail the check. The `help` subcommands, which clap generates from the tree. Commands marked
`hide = true` (today `bridge deposit-address`), which the help does not print because they are
not advertised, so they are not public surface. The value of an `[env: NAME=value]`, which is
whatever the caller's environment holds; the binary runs with an empty one anyway.

Anything in the help it cannot place fails loudly instead of being skipped. Stdlib only.
"""

import re
import subprocess
import sys
import tempfile

SECTION = re.compile(r"^([A-Z][A-Za-z ]*):$")
# The bracketed facts clap appends to a description. Prose like `[deprecated]` has no `: `.
SPEC_VALUE = re.compile(r"\[(?:env|default|possible values|aliases|short aliases): [^\]]*\]")
ENV_VALUE = re.compile(r"^\[env: ([^=\]]+)=[^\]]*\]$")
# A flag or argument line is indented at most 6 (`  -h, --help`, `      --yes`); deeper is the
# description of the line above, when clap puts it on the next line.
ENTRY_INDENT = 6


class HelpFormatError(Exception):
    pass


def run_help(binary, path, home):
    proc = subprocess.run(
        [binary, *path, "-h"],
        capture_output=True,
        text=True,
        # No inherited environment: the help prints the value of every `env = ...` variable it has,
        # and the listing must not depend on, or show, the caller's.
        env={"PATH": "/usr/bin:/bin", "HOME": home},
        check=False,
    )
    if proc.returncode != 0:
        raise HelpFormatError(f"`nexus {' '.join(path)} -h` exited {proc.returncode}: {proc.stderr.strip()}")
    return proc.stdout


def parse_help(text, where):
    """(usage lines, {section: [(spec, [spec values])]}) from one command's `-h` output."""
    usage, sections, current = [], {}, None
    in_usage = False
    for line in text.splitlines():
        if line.startswith("Usage: "):
            usage.append(line[len("Usage: "):].strip())
            in_usage = True
            continue
        if in_usage and line.startswith("       ") and line.strip():
            usage.append(line.strip())
            continue
        in_usage = False
        if not usage or not line.strip():
            # Above `Usage:` is the command's description; blank lines separate sections.
            continue
        heading = SECTION.match(line)
        if heading:
            current = heading.group(1)
            sections.setdefault(current, [])
            continue
        indent = len(line) - len(line.lstrip(" "))
        if current is None or indent == 0:
            raise HelpFormatError(f"{where}: a line outside any section: {line!r}")
        if indent > ENTRY_INDENT and sections[current]:
            spec, desc = sections[current][-1]
            sections[current][-1] = (spec, f"{desc} {line.strip()}")
            continue
        spec, _, desc = line.strip().partition("  ")
        sections[current].append((spec.strip(), desc))
    if not usage:
        raise HelpFormatError(f"{where}: no `Usage:` line")
    return usage, {
        name: [(spec, spec_values(desc)) for spec, desc in entries] for name, entries in sections.items()
    }


def spec_values(desc):
    values = []
    for value in SPEC_VALUE.findall(desc):
        env = ENV_VALUE.match(value)
        values.append(f"[env: {env.group(1)}]" if env else value)
    return values


def walk(binary, home, path=(), command_values=(), out=None):
    out = [] if out is None else out
    name = " ".join(("nexus", *path))
    out.append(((path, 0, ""), " ".join((name, *command_values))))
    usage, sections = parse_help(run_help(binary, list(path), home), f"`{name} -h`")
    for line in usage:
        rest = line[len(name):].strip() if line == name or line.startswith(name + " ") else line
        out.append(((path, 1, "usage"), f"{name}  usage: {rest}"))
    for section, entries in sections.items():
        for spec, values in entries:
            if section == "Commands":
                if spec != "help":
                    walk(binary, home, (*path, spec), values, out)
                continue
            if not spec.startswith(("-", "<", "[")):
                raise HelpFormatError(f"`{name} -h`: cannot read {spec!r} in section {section!r}")
            item = " ".join((spec, *values))
            out.append(((path, 1, item), f"{name}  {item}"))
    return out


def fold_common(entries):
    """List once the flags and arguments every command has, under `nexus (every command)`."""
    items = {}
    for (path, rank, item), _ in entries:
        items.setdefault(path, set())
        if rank == 1 and item != "usage":
            items[path].add(item)
    common = set.intersection(*items.values())
    kept = [entry for entry in entries if not (entry[0][1] == 1 and entry[0][2] in common)]
    return kept + [(((), -1, item), f"nexus (every command)  {item}") for item in common]


def main(argv):
    if len(argv) != 2:
        print(f"usage: {argv[0]} <path to the nexus binary>", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as home:
        try:
            entries = walk(argv[1], home)
        except HelpFormatError as err:
            print(f"cli_surface.py: {err}", file=sys.stderr)
            return 1
    for _, line in sorted(fold_common(entries)):
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
