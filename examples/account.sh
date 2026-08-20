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

# Portfolio totals, including the withdrawable balance (engine-authoritative
# free margin, floored at zero — what can actually leave the account).
nexus account summary       # GET /account/summary

# The summary above PLUS every open position, from one coherent server-side
# read. Prefer this over `account summary` + `positions`: those are two separate
# requests, so a fill landing between them returns a mismatched pair.
nexus account state         # GET /account/state

# Effective fee schedule. A NEGATIVE maker fee is a rebate paid to you.
nexus account fees          # GET /account/fees

# Portfolio time series: equity, cumulative PnL, cumulative volume, oldest
# first. --window picks the span and the sample cadence (day 5m / week 1h /
# month 6h / all 1d); --limit caps the points (1..=366).
nexus account portfolio-history --window week --limit 100   # GET /account/portfolio-history

# Open positions (with per-position notional, margin used, ROE, max leverage and
# funding paid) and recent fills (executions).
nexus positions             # GET /positions
nexus fills --limit 50      # GET /fills

# Open orders and withdrawal history.
nexus orders                # GET /orders
nexus withdrawals           # GET /withdrawals

# Funding booked against the account.
nexus funding-payments --limit 50   # GET /funding-payments

# `nexus transfers list` and `nexus sub-accounts list` used to be here. Removed
# (ENG-8123): /transfers and /sub-accounts 404 on the live venue where authenticated
# routes 401, so this script — which is meant to be pasted and run against a funded
# account — failed halfway through with a raw 404. The commands still exist and now say
# so themselves; whether they ship or are withdrawn is ENG-7800.

# ADL settlements that touched an account (as bankrupt target or closed
# counterparty).   GET /account/{address}/adl-history
nexus account adl-history 0x<ADDRESS> --limit 20

# Caller's rate-limit status (tier, remaining, reset).   GET /account/rate-limit
nexus account rate-limit

# JSON for scripting/piping into jq:
nexus --output json balance | jq '.equity'

# In JSON, an unreported figure is `null`, never 0 — "not reported" is not
# "zero", and a position's risk field carries its reason in a companion
# `*_error`. Handle the null rather than defaulting it:
nexus --output json account summary | jq '.withdrawable // "not reported"'
nexus --output json positions | jq '.[] | {market_id, roe, roe_error}'
