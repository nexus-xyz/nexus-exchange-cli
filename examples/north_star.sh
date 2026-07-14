#!/usr/bin/env bash
# North star: fund via the faucet → trade → inspect positions — end to end,
# unassisted, against testnet. Unlike the other examples (recipes to run a line
# at a time, with <PLACEHOLDERS>), this script is designed to run top to bottom
# with no editing: `bash examples/north_star.sh`.
#
# Required credentials (and nothing else):
#   export NEXUS_API_KEY=nx_...       # HMAC key pair for a testnet account —
#   export NEXUS_API_SECRET=...       # mint one with `nexus keys create`
#                                     # (see keys_and_agents.sh)
#
# Network: defaults to the beta (testnet) channel, where `account credit` is a
# synthetic-USDX faucet and no real funds ever move. NEVER point this script at
# production: it places (and then cancels) a real resting order.
#
# Needs `jq` and the `nexus` binary on PATH (cargo build --release, then add
# target/release to PATH).
set -euo pipefail

MARKET="${MARKET:-BTC-USDX-PERP}"
NETWORK="${NETWORK:-beta}"

command -v nexus >/dev/null || { echo "error: nexus not on PATH" >&2; exit 1; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 1; }
: "${NEXUS_API_KEY:?export NEXUS_API_KEY (see header comment)}"
: "${NEXUS_API_SECRET:?export NEXUS_API_SECRET (see header comment)}"

nx() { nexus --network "$NETWORK" "$@"; }

echo "── 1. fund: claim synthetic USDX from the testnet faucet ──"
# POST /account/credit. Omitting --amount claims the remaining daily allowance;
# a second run the same day claims 0 more, so this stays idempotent-ish. Don't
# abort the walkthrough if today's allowance is already spent.
nx account credit || echo "(faucet claim failed — daily allowance likely spent; continuing)"

echo "── 2. account before trading ──"
nx balance # GET /account

echo "── 3. trade: place a deep resting limit buy ──"
# Quote off the live mark price and bid 50% below it, so the order rests (this
# is a plumbing walkthrough, not a fill-seeking strategy). Rounding to a whole
# number keeps the price on the market's tick grid.
MARK=$(nx --output json mark-price "$MARKET" | jq -r .mark_price)
PRICE=$(awk -v m="$MARK" 'BEGIN { printf "%d", m * 0.5 }')
echo "mark ${MARK}, bidding ${PRICE}"

# Tag the order with our own client_order_id so the rest of the flow never has
# to scrape the exchange-assigned id. `order batch` (a JSON array on stdin) is
# the placement path that carries client_order_id; --yes skips the interactive
# confirmation, which is required when running as a script.
CLIENT_ID="north-star-$$-$(date +%s)"
nx order batch - --yes <<EOF
[{"market": "$MARKET", "side": "buy", "type": "limit",
  "price": "$PRICE", "quantity": "0.001", "tif": "gtc",
  "client_order_id": "$CLIENT_ID"}]
EOF

echo "── 4. inspect: the order, open orders, positions ──"
nx order get-by-client-id "$CLIENT_ID" # GET /orders/by-client-id/{id}
nx orders                              # GET /orders
nx positions                           # GET /positions (empty until a fill)

echo "── 5. clean up: cancel the resting order ──"
nx order cancel-by-client-id "$CLIENT_ID" --yes # DELETE /orders/by-client-id/{id}

echo "── 6. account after ──"
nx fills --limit 5 # GET /fills (a deep bid normally never fills)
nx balance

echo "north star complete: funded, traded, inspected, cleaned up."
