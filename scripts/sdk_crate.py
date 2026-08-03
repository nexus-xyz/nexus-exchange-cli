#!/usr/bin/env python3
"""Read facts out of the `nexus-exchange` crate the CLI actually compiles against.

The CLI issues no HTTP of its own — every request goes through
`nexus_exchange::Client`, and the crate pins the spec version it targets and sends
it as `X-Nexus-Api-Version` on every request. So the CLI's own `.api-version` is
not an independent choice: it is a restatement of the crate's, and it is only
correct if it matches. This module is the shared plumbing for the two scripts that
care (ENG-7962):

  * check_sdk_parity.py  — is our pin (and manifest) consistent with the crate?
  * sync_sdk_version.py  — has a newer crate been published?

Everything here is tokenless. The crates.io API needs only a User-Agent, and the
published `.crate` tarball ships both `.api-version` and `endpoints.txt` — verified
for 0.6.0/0.6.1/0.7.0. Reading them from the *tarball* rather than from the rs repo
at a tag is deliberate: the tarball is the exact artifact cargo compiles, so there
is no assumption about how rs names its tags and no second repo in the path.

The resolved version comes from `Cargo.lock`, NOT `Cargo.toml`. `Cargo.toml` holds
a caret requirement — `"0.6.0"` means `^0.6.0`, which permits 0.6.1 — so the two
legitimately diverge and only the lockfile says what actually builds.
"""
import io
import json
import os
import re
import sys
import tarfile
import tomllib
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
CARGO_TOML = os.path.join(REPO, "Cargo.toml")
CARGO_LOCK = os.path.join(REPO, "Cargo.lock")

SDK_CRATE = "nexus-exchange"
CRATES_API = f"https://crates.io/api/v1/crates/{SDK_CRATE}"
# crates.io rejects requests without a User-Agent, so identify ourselves.
USER_AGENT = f"{SDK_CRATE}-cli-spec-tooling (+https://github.com/nexus-xyz/nexus-exchange-cli)"

# Where downloaded .crate tarballs land. Outside the repo by default so a local run
# leaves no untracked files; override for a warm cache in CI.
CACHE_DIR = os.environ.get("SDK_CRATE_CACHE") or os.path.join(
    os.environ.get("TMPDIR", "/tmp"), "nexus-sdk-crates"
)

# A crate version is plain semver (no `v`); the spec tag it pins carries one.
CRATE_VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
TAG_RE = re.compile(r"^v[0-9]+(\.[0-9]+){0,2}$")


def fail(msg):
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def version_key(v):
    """Comparable tuple for a crate version or a `vX.Y.Z` spec tag."""
    core = v[1:] if v.startswith("v") else v
    parts = core.split(".")
    if not all(p.isdigit() for p in parts) or not 1 <= len(parts) <= 3:
        fail(f"not a comparable version: {v!r}")
    return tuple(int(p) for p in parts) + (0,) * (3 - len(parts))


def locked_version():
    """The `nexus-exchange` version Cargo actually resolves to — the one that
    builds, and therefore the only one whose pin we can be consistent with."""
    try:
        with open(CARGO_LOCK, "rb") as f:
            lock = tomllib.load(f)
    except OSError as e:
        fail(f"cannot read {CARGO_LOCK}: {e}")
    except tomllib.TOMLDecodeError as e:
        fail(f"{CARGO_LOCK} is not valid TOML: {e}")
    found = [p for p in lock.get("package", []) if p.get("name") == SDK_CRATE]
    if not found:
        fail(f"{SDK_CRATE} is not in {CARGO_LOCK}; is it still a dependency?")
    if len(found) > 1:
        fail(
            f"{CARGO_LOCK} resolves {len(found)} versions of {SDK_CRATE} "
            f"({', '.join(sorted(p['version'] for p in found))}); the CLI must "
            f"link exactly one, or 'the pin' has no single meaning."
        )
    version = found[0]["version"]
    if not CRATE_VERSION_RE.match(version):
        fail(f"unexpected {SDK_CRATE} version in {CARGO_LOCK}: {version!r}")
    return version


def required_version():
    """The requirement string in Cargo.toml — what the autobump rewrites. Returns
    the bare version if it is a plain `"X.Y.Z"`, else fails: a range, git or path
    dependency means the pin is not derivable and a human must decide."""
    try:
        with open(CARGO_TOML, "rb") as f:
            manifest = tomllib.load(f)
    except OSError as e:
        fail(f"cannot read {CARGO_TOML}: {e}")
    except tomllib.TOMLDecodeError as e:
        fail(f"{CARGO_TOML} is not valid TOML: {e}")
    dep = manifest.get("dependencies", {}).get(SDK_CRATE)
    if dep is None:
        fail(f"{SDK_CRATE} is not in [dependencies] of {CARGO_TOML}")
    if isinstance(dep, dict):
        dep = dep.get("version")
    if not isinstance(dep, str) or not CRATE_VERSION_RE.match(dep):
        fail(
            f"{SDK_CRATE} dependency in {CARGO_TOML} is {dep!r}; this tooling only "
            f"handles a plain \"X.Y.Z\" requirement. A range/git/path dependency "
            f"means the spec pin cannot be derived — bump it by hand."
        )
    return dep


def _get(url, binary=False):
    req = urllib.request.Request(url)
    req.add_header("User-Agent", USER_AGENT)
    if not binary:
        req.add_header("Accept", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        fail(f"{url} returned {e.code}: {e.reason}")
    except urllib.error.URLError as e:
        fail(f"could not reach {url}: {e.reason}")


def latest_published():
    """The newest non-yanked `nexus-exchange` on crates.io."""
    data = json.loads(_get(CRATES_API))
    versions = [
        v["num"]
        for v in data.get("versions", [])
        if not v.get("yanked") and CRATE_VERSION_RE.match(v.get("num", ""))
    ]
    if not versions:
        fail(f"no usable published versions of {SDK_CRATE} on crates.io")
    # Don't trust max_version: it can name a yanked release. Sort ourselves.
    return max(versions, key=version_key)


def crate_path(version):
    """Download the `.crate` tarball for `version` (cached) and return its path."""
    os.makedirs(CACHE_DIR, exist_ok=True)
    path = os.path.join(CACHE_DIR, f"{SDK_CRATE}-{version}.crate")
    if not os.path.exists(path):
        blob = _get(f"{CRATES_API}/{version}/download", binary=True)
        tmp = path + ".part"
        with open(tmp, "wb") as f:
            f.write(blob)
        os.replace(tmp, path)
    return path


def crate_file(version, name):
    """Text content of `<crate>-<version>/<name>` from inside the published
    tarball. Fails loudly if absent — every consumer treats these files as
    load-bearing, so a silent "" would turn a real mismatch into a pass."""
    member = f"{SDK_CRATE}-{version}/{name}"
    with tarfile.open(crate_path(version), "r:gz") as tf:
        try:
            fh = tf.extractfile(member)
        except KeyError:
            fh = None
        if fh is None:
            fail(
                f"{member} is not in the published {SDK_CRATE} {version} tarball. "
                f"The crate stopped shipping it, so the CLI can no longer verify "
                f"its pin against the SDK — raise it upstream rather than skipping "
                f"the check."
            )
        return io.TextIOWrapper(fh, encoding="utf-8").read()


def crate_api_version(version):
    """The spec tag the crate pins — and sends as `X-Nexus-Api-Version`."""
    tag = crate_file(version, ".api-version").strip()
    if not TAG_RE.match(tag):
        fail(f"{SDK_CRATE} {version} ships an unparseable .api-version: {tag!r}")
    return tag


def crate_endpoints(version):
    """The SDK's own operations manifest, as a set of (METHOD, path). This is the
    ceiling on what the CLI can reach: the CLI calls named SDK methods, so an
    operation the SDK does not wrap is not reachable from a command."""
    ops = set()
    for raw in crate_file(version, "endpoints.txt").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        if len(parts) != 2:
            fail(f"{SDK_CRATE} {version} endpoints.txt: cannot parse {line!r}")
        ops.add((parts[0].upper(), parts[1]))
    if not ops:
        fail(f"{SDK_CRATE} {version} endpoints.txt parsed to zero operations")
    return ops
