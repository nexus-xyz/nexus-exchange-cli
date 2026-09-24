#!/usr/bin/env bash
# Refresh src/examples_catalog.json, the copy of the examples catalog built into
# the binary. `nexus examples list/show` fall back to it when the repository
# can't be reached and nothing is cached (ENG-17337). Run before a release.
set -euo pipefail
cd "$(dirname "$0")/.."
ref="${1:-main}"
curl -fsSL "https://raw.githubusercontent.com/nexus-xyz/nexus-exchange-examples/${ref}/catalog.json" \
  -o src/examples_catalog.json.tmp
python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); assert d["schema"]==1 and d["examples"]' \
  src/examples_catalog.json.tmp
mv src/examples_catalog.json.tmp src/examples_catalog.json
echo "src/examples_catalog.json refreshed from nexus-exchange-examples@${ref}"
