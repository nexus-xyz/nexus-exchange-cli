#!/usr/bin/env bash
# Manage HMAC API keys and registered agent keys.
#
# Requires credentials (see account.sh). Mutating commands prompt for
# confirmation unless --yes is passed.
set -euo pipefail

# ── API keys ──
nexus keys list                 # GET /keys
# Create a key. The secret is shown ONCE — store it immediately.
nexus keys create --yes         # POST /keys
nexus keys delete <KEY_ID> --yes   # DELETE /keys/{id}

# ── agent keys ──
# List registered agent keys for the authenticated wallet, then revoke one.
nexus agents list                       # GET /agents
nexus agents revoke 0x<ADDRESS> --yes   # DELETE /agents/{address}
# (Registering a new agent is wallet-signed; see `nexus auth` once it ships.)
