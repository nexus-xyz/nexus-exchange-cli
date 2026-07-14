#!/usr/bin/env bash
# Public market data — no credentials required.
#
# Run line by line, or `bash examples/market_data.sh` after building the binary
# (`cargo build --release` then add target/release to PATH). Set MARKET to any
# listed market id.
set -euo pipefail

MARKET="${MARKET:-BTC-USDX-PERP}"

# List every tradable market and its trading rules.   GET /markets
nexus markets

# Per-market 24h summaries (mark price, volume, status).   GET /markets/summary
nexus summaries

# Tickers for every market, then one market's ticker.
nexus tickers                       # GET /tickers
nexus ticker "$MARKET"              # GET /markets/{id}/ticker

# Order book, recent trades, and OHLCV candles.
nexus orderbook "$MARKET"          # GET /markets/{id}/orderbook
nexus trades "$MARKET" --limit 20  # GET /markets/{id}/trades
nexus candles "$MARKET" --timeframe 1m --limit 50   # GET /markets/{id}/candles

# Funding-rate history, current mark price, and lifecycle/halt status.
nexus funding-rates "$MARKET" --limit 24   # GET /markets/{id}/funding
nexus mark-price "$MARKET"                 # GET /markets/{id}/mark-price
nexus market-status "$MARKET"              # GET /markets/{id}/status

# The same per-market reads also live under the `market` group:
nexus market summary               # GET /markets/summary
nexus market status "$MARKET"      # GET /markets/{id}/status
nexus market mark-price "$MARKET"  # GET /markets/{id}/mark-price

# Indexer health snapshot. Useful as a connectivity check.   GET /health
nexus health

# ADL settlement events for a market (auto-deleveraging history). The one
# market-scoped read that needs credentials — the endpoint is HMAC-gated.
nexus market adl-events "$MARKET" --limit 20   # GET /markets/{id}/adl-events

# Everything above also speaks JSON:
nexus --output json ticker "$MARKET"
