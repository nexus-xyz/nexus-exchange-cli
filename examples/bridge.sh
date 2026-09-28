#!/usr/bin/env bash
# Cross-chain deposits (bridge Phase A).
#
# `bridge assets` is public. Everything else is account-scoped: configure
# credentials once with `nexus setup`, or export:
#   export NEXUS_API_KEY=nx_...
#   export NEXUS_API_SECRET=...
# Authenticated commands refuse to run (non-zero exit) when no credentials are
# configured, rather than sending an unsigned request.
set -euo pipefail

# 1. What can be bridged, and from where. Public — no credentials.
#    Each row is one asset on one chain, with the minimum deposit, the
#    confirmations required before crediting, and the token contract.
#    GET /api/v1/bridge/assets
nexus bridge assets

# `withdraw` rows describe the eventual capability. The bridge serves deposits
# only today, so there is no `nexus bridge withdraw` to pair with them.

# There is no step for getting a deposit address: `nexus bridge deposit-address`
# is deprecated, because no server implements /api/v1/bridge/deposit-addresses
# (ENG-11460).

# 2. Track what has arrived. `CONF` is `seen/required`, and `-` on either half
#    means the server has not reported it — a deposit not yet seen on chain has
#    neither, which is not the same as zero.
#    Lifecycle: detected -> confirming -> credited | failed.
#    GET /api/v1/bridge/deposits
nexus bridge deposits

# 3. One deposit in full, including the source-chain tx hash (kept out of the
#    table above, where 66 characters would push every other column off screen).
#    Take the id from the list rather than typing one, so this runs unedited.
#    GET /api/v1/bridge/deposits/{id}
DEPOSIT_ID=$(nexus --output json bridge deposits | jq -r '.[0].id // empty')
nexus bridge deposits --id "$DEPOSIT_ID"

# JSON for scripting/piping into jq. Amounts are decimal strings so no precision
# is lost, and timestamps are ISO-8601 UTC.
nexus --output json bridge assets | jq '.chains[] | {chain, deposits: [.deposit_assets[].symbol]}'
nexus --output json bridge deposits | jq '.[] | select(.status != "credited")'

# Poll one deposit until it credits:
#   until [ "$(nexus --output json bridge deposits --id "$DEPOSIT_ID" | jq -r .status)" = credited ]
#   do sleep 15; done
