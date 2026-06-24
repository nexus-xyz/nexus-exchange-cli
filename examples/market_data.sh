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

# Indexer health snapshot. Useful as a connectivity check.   GET /health
nexus health

# Everything above also speaks JSON:
nexus --output json ticker "$MARKET"
