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

# Post-only: rest or be rejected, never cross. It is a time-in-force on this
# API, not a separate flag — so it is `--tif post-only`, not `--post-only`.
nexus order place \
  --market "$MARKET" --side buy --type limit \
  --price 84000 --quantity 0.01 --tif post-only --yes

# A market order ignores --price. --reduce-only never opens/flips a position.
nexus order place \
  --market "$MARKET" --side sell --type market --quantity 0.01 --reduce-only --yes

# List open orders, then fetch one by exchange id. By-id routes are routed per
# market, so get/amend/cancel-one all require --market. A client_order_id you
# assign at placement comes back in the placement response and on every order
# read, so scripts can correlate without a lookup route — see north_star.sh,
# which reads the id straight out of the batch response.
nexus orders                                          # GET /orders
nexus order get <ORDER_ID> --market "$MARKET"         # GET /orders/{id}

# Amend an open order in place (atomic cancel-replace); set only what changes.
nexus order amend <ORDER_ID> --market "$MARKET" --price 83500 --yes    # PUT /orders/{id}

# Submit several orders at once from a JSON array (see batch_orders.json).
nexus order batch examples/batch_orders.json --yes  # POST /orders/batch
cat examples/batch_orders.json | nexus order batch - --yes   # ...or from stdin

# Cancel: one order by id, every open order in ONE market (a per-market
# flatten), or everything.
nexus order cancel <ORDER_ID> --market "$MARKET" --yes    # DELETE /api/v1/orders/{id}
nexus order cancel --market "$MARKET" --yes               # DELETE /api/v1/orders?market_id=
nexus order cancel --all --yes                            # DELETE /api/v1/orders
# There is no by-client-id or multi-id cancel. `order cancel-by-client-id`,
# `order get-by-client-id` and `order cancel-batch` were withdrawn in ENG-12369:
# they targeted /orders/by-client-id/{id} and /orders/batch-cancel, which no
# published spec version defines (ENG-5487). Cancel by exchange id, or flatten
# the market in one request.

# ── account settings ──
nexus account deposit 1000 --yes                 # POST /account/deposit
nexus account credit                             # POST /account/credit (testnet faucet)
# No margin-mode example: `nexus account margin-mode` was withdrawn in ENG-7740
# because no endpoint accepts a margin-mode change. ENG-7614 tracks the engine
# work that has to land before the command can return.
# No leverage example either: `nexus account leverage` was withdrawn in
# ENG-12369. It sent POST /account/leverage, which no published spec version
# defines and nothing routes — the venue serves POST /leverage, and ENG-7318 is
# documenting it. The command returns once a released spec carries the route.
