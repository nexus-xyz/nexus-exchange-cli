#!/usr/bin/env bash
# Inspect an authenticated account (read-only).
#
# Requires credentials. Configure them once with `nexus setup`, or export:
#   export NEXUS_API_KEY=nx_...
#   export NEXUS_API_SECRET=...
# Authenticated commands refuse to run (non-zero exit) when no credentials are
# configured, rather than sending an unsigned request.
set -euo pipefail

# Account summary: balance, collateral, equity, margin.   GET /account
nexus balance

# Open positions and recent fills (executions).
nexus positions             # GET /positions
nexus fills --limit 50      # GET /fills

# Open orders and withdrawal history.
nexus orders                # GET /orders
nexus withdrawals           # GET /withdrawals

# Funding booked against the account, collateral transfers, sub-accounts.
nexus funding-payments --limit 50   # GET /funding-payments
nexus transfers list                # GET /transfers
nexus sub-accounts list             # GET /sub-accounts

# ADL settlements that touched an account (as bankrupt target or closed
# counterparty).   GET /account/{address}/adl-history
nexus account adl-history 0x<ADDRESS> --limit 20

# Caller's rate-limit status (tier, remaining, reset).   GET /account/rate-limit
nexus account rate-limit

# JSON for scripting/piping into jq:
nexus --output json balance | jq '.equity'
