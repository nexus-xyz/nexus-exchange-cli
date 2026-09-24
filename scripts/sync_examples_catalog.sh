#!/usr/bin/env bash
# Refresh src/examples_catalog.json, the copy of the examples catalog built into
# the binary, and pin it: src/examples_catalog.ref records the examples-repo tag
# it came from. `nexus examples list/show` fall back to this copy when the
# repository can't be reached and nothing is cached (ENG-17337).
#
# The examples repo tags every green commit on main `catalog-YYYY.MM.DD`
# (tag-catalog.yml). With no argument this takes the newest such tag; pass a tag
# to pin a specific one. Run it before merging a release PR.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="https://github.com/nexus-xyz/nexus-exchange-examples"
tag="${1:-$(git ls-remote --tags --refs "$repo" 'catalog-*' | sed 's#.*refs/tags/##' | sort -V | tail -1)}"
if [ -z "$tag" ]; then
  echo "no catalog-* tag in $repo yet; nothing to pin" >&2
  exit 1
fi
curl -fsSL "https://raw.githubusercontent.com/nexus-xyz/nexus-exchange-examples/${tag}/catalog.json" \
  -o src/examples_catalog.json.tmp
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["schema"]==1 and d["examples"]' \
  src/examples_catalog.json.tmp
mv src/examples_catalog.json.tmp src/examples_catalog.json
printf '%s\n' "$tag" > src/examples_catalog.ref
echo "pinned the built-in examples catalog to ${tag}"
