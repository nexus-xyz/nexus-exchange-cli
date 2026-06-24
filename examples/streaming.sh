#!/usr/bin/env bash
# Stream live data over WebSocket.
#
# Public channels (trades, book, candles) need --market and no credentials.
# Account channels (orders, fills, positions, balances) are scoped to your key:
# the CLI mints a short-lived single-use token (POST /ws/token) and opens the
# upgrade (GET /ws). Press Ctrl-C to stop.
set -euo pipefail

MARKET="${MARKET:-BTC-USDX-PERP}"

# Public: live trades and order-book updates for one market.
nexus ws trades --market "$MARKET"
nexus ws book --market "$MARKET"

# Subscribe to several channels at once; --since resumes from a sequence number.
nexus ws trades book --market "$MARKET" --since 0

# Account channels (require credentials):   POST /ws/token then GET /ws
nexus ws orders fills positions balances

# JSON frames for machine consumption:
nexus --output json ws trades --market "$MARKET"
