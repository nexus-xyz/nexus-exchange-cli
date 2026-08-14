#!/usr/bin/env bash
# Stream live data over WebSocket.
#
# Public channels (trades, book, candles) need --market and no credentials.
# Account channels (orders, fills, positions, balances) are scoped to your key:
# the CLI mints a short-lived single-use token (POST /ws/token) and opens the
# upgrade (GET /ws). Press Ctrl-C to stop.
set -euo pipefail

MARKET="${MARKET:-BTC-USDX-PERP}"

# Public: live trades, order-book updates, and candles for one market.
nexus ws trades --market "$MARKET"
nexus ws book --market "$MARKET"
nexus ws candles --market "$MARKET"

# Subscribe to several channels at once; --since resumes every subscribed
# channel from a sequence number (useful to catch up after a disconnect).
nexus ws trades book --market "$MARKET" --since 0

# Account channels (require credentials):   POST /ws/token then GET /ws
nexus ws orders fills positions balances

# Mix public and account channels in one stream — the market scopes only the
# public ones; account channels stay scoped to your key.
nexus ws trades orders fills --market "$MARKET"

# JSON mode emits one compact envelope per line
# ({"op":"event","channel":...,"seq":...,"payload":{...}}) — jq-friendly:
nexus --output json ws trades --market "$MARKET"
nexus --output json ws trades --market "$MARKET" | jq 'select(.op == "event") | .payload'
