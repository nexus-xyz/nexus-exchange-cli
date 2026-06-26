#!/usr/bin/env bash
# Place, inspect, amend, and cancel orders.
#
# Requires credentials (see account.sh). Mutating commands prompt for
# confirmation; pass --yes to skip the prompt (required in non-interactive
# contexts). DOUBLE-CHECK the market/price/quantity before running with --yes.
set -euo pipefail

MARKET="${MARKET:-BTC-USDX-PERP}"

# Place a resting limit buy.   POST /orders
nexus order place \
  --market "$MARKET" --side buy --type limit \
  --price 84000 --quantity 0.01 --tif gtc --yes

# A market order ignores --price. --reduce-only never opens/flips a position.
nexus order place \
  --market "$MARKET" --side sell --type market --quantity 0.01 --reduce-only --yes

# List open orders, then fetch one by id.
nexus orders                       # GET /orders
nexus order get <ORDER_ID>         # GET /orders/{id}

# Amend an open order in place (atomic cancel-replace); set only what changes.
nexus order amend <ORDER_ID> --price 83500 --yes    # PUT /orders/{id}

# Submit several orders at once from a JSON array (see batch_orders.json).
nexus order batch examples/batch_orders.json --yes  # POST /orders/batch
cat examples/batch_orders.json | nexus order batch - --yes   # ...or from stdin

# Cancel one order, or every open order.
nexus order cancel <ORDER_ID> --yes   # DELETE /orders/{id}
nexus order cancel --all --yes        # DELETE /orders

# ── account settings (ahead of the pinned spec; see endpoints.txt) ──
nexus account deposit 1000 --yes                 # POST /account/deposit
nexus account credit                             # POST /account/credit (testnet faucet)
nexus account leverage "$MARKET" 10              # POST /account/leverage
nexus account margin-mode "$MARKET" isolated     # POST /account/margin-mode
