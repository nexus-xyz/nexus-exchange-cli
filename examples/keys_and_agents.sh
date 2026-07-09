#!/usr/bin/env bash
# Onboarding walkthrough: wallet sign-in → HMAC API-key management → agent
# registration. The full path from "I have an EVM wallet" to "my bot trades
# with its own key".
#
# Credentials by step:
#   * `auth login` / `agents register` — wallet-signed (EIP-191 / EIP-712).
#     They read the raw private key from --private-key, NEXUS_PRIVATE_KEY, or a
#     hidden interactive prompt; the key signs locally and is never persisted.
#   * `keys …` — needs the session token stored by `auth login` (or an existing
#     HMAC pair from `nexus setup` / NEXUS_API_KEY + NEXUS_API_SECRET).
#   * `agents list` / `agents revoke` — same authenticated session.
# Mutating commands prompt for confirmation unless --yes is passed.
set -euo pipefail

# ── 1. wallet sign-in (EIP-191) ──
# Signs the fixed login challenge and stores the 24h session token (mode 0600).
# Run interactively for a hidden key prompt, or export NEXUS_PRIVATE_KEY.
nexus auth login                # POST /auth/login

# ── 2. HMAC API keys ──
# The session token authenticates key management; trading itself signs with an
# HMAC key pair.
nexus keys list                 # GET /keys
# Create a key. The secret is shown ONCE — store it immediately, e.g.:
#   export NEXUS_API_KEY=nx_...
#   export NEXUS_API_SECRET=...
nexus keys create --yes         # POST /keys
nexus keys delete <KEY_ID> --yes   # DELETE /keys/{id}

# ── 3. agent keys ──
# Authorize a separate agent key to trade on the wallet's behalf (EIP-712,
# signed by the OWNING wallet's key; the request itself is unauthenticated —
# the signature is the authorization). Defaults: 30-day expiry, current-ms
# nonce, exchange chain id.
nexus agents register --agent 0x<AGENT_ADDR> --label "trading-bot-1" --yes   # POST /agents/register

# List the wallet's registered agents, then revoke one by address.
nexus agents list                       # GET /agents
nexus agents revoke 0x<ADDRESS> --yes   # DELETE /agents/{address}
