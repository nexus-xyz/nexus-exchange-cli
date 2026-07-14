# cli.nexus.xyz — installer host

A tiny Cloudflare Worker that makes

```sh
curl https://cli.nexus.xyz | sh                 # macOS + Linux
```
```powershell
irm https://cli.nexus.xyz | iex                  # Windows
```

install the **latest** released `nexus` CLI. (ENG-3454)

## How it works

cargo-dist attaches an installer to every GitHub release under a stable
"latest" URL:

```
https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.sh
https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.ps1
```

That URL **redirects** to the actual asset, and `curl <url> | sh` does **not**
follow redirects without `-L`. So instead of redirecting, this Worker **proxies**
the script body back with a `200`, choosing the variant from the request:

| Request | Served |
|---|---|
| default / `curl` / `wget` | exchange `…-installer.sh` |
| `User-Agent` contains `PowerShell`, or path ends `.ps1`, or `?powershell` | exchange `…-installer.ps1` |
| path ends `.sh` | exchange `…-installer.sh` (forced) |
| path is `/compute` (or `/compute.sh`) | **legacy compute CLI** installer (see below) |

## `/compute` — the legacy compute CLI (ENG-3937)

`cli.nexus.xyz/compute` serves the prover/compute CLI (`nexus-compute-cli`,
formerly `nexus-cli`) so it stays installable from the same host after the bare
root flips to the exchange CLI:

```sh
curl https://cli.nexus.xyz/compute | sh        # macOS + Linux (POSIX sh only)
```

That CLI publishes a single rendered `install.sh` to **Firebase Hosting**
(`https://nexus-cli.web.app/install.sh` — produced by nexus-cli's
`firebase-hosting-release.yml`). The `/compute` route proxies that pinned URL
through the **same** security scaffold as the default route, with two
differences: the asserted upstream origin is `https://nexus-cli.web.app` (not
`github.com`), and there is **no PowerShell variant** — a `.ps1`/PowerShell
request to `/compute` fails closed with a `404` error script rather than handing
an sh body to `iex`.

Those release artifacts (the `…-installer.{sh,ps1}` scripts, plus the
cross-platform binaries they fetch — including the Windows build behind the
PowerShell variant) are produced by the signed `dist` release pipeline added in
**#4 (ENG-3432)**. This PR is scoped to the Worker + README snippet only and
carries no release config of its own, so it should merge **after #4**; until
then the `releases/latest/download/…` URLs the Worker proxies won't exist yet.

See [`src/installer.mjs`](src/installer.mjs) for the full security rationale.
Highlights: the upstream URL is built only from pinned constants (no SSRF), the
origin is asserted to be `github.com`, responses are `text/plain; nosniff`, and
**any** failure returns a tiny valid script that errors out cleanly rather than
piping an HTML error page into a shell.

## Trust root & integrity

This Worker does **not** add a signature of its own; it relays the installer
script verbatim. The integrity of `curl … | sh` therefore rests on, in order:

1. **TLS to `cli.nexus.xyz`** (Cloudflare-terminated) and **TLS from the Worker
   to `github.com`** — the upstream origin is asserted to be exactly
   `https://github.com`, so the script can only come from this repo's releases.
2. **GitHub release integrity** — only repo maintainers can publish the
   `…-installer.{sh,ps1}` assets the Worker fetches.
3. **cargo-dist `sha256` verification** — the script the Worker serves itself
   verifies the SHA256 of the binary tarball it downloads, so the binary chain
   is cryptographically checked even though the script body is not.
4. **minisign signing (#4 / ENG-3432)** — the release pipeline this Worker
   depends on signs its artifacts; that is the signature root of trust for the
   whole chain. This Worker carries no competing/unsigned dist config and serves
   only what that signed pipeline publishes.

The script body piped to the shell is guarded structurally (shebang/HTML sniff,
size cap, fail-closed error script) but is **not** independently signature-checked
here — its authenticity derives from (1)+(2)+(4) above. Pinning a per-release
script signature would require distributing #4's public key to the Worker and is
tracked as a follow-up.

## Develop & test

No dependencies required for tests — they run on the Node built-in test runner:

```sh
cd installer
node --test          # or: npm test
```

Local run / deploy use Wrangler:

```sh
npm install
npm run dev          # local preview
npm run deploy       # deploy to Cloudflare (needs account access)
```

## Deploying (cutover — ENG-3938)

⚠️ **Outward-facing, prover-community blast radius.** Flipping `cli.nexus.xyz`
from the legacy compute installer to the exchange CLI strands provers if done
early. Deploy only after the gates below are all green, and coordinate timing.

**Gates (ENG-3938):** signed exchange release verified (ENG-3936 ✅), `/compute`
route merged (ENG-3937), compute rename *released* (ENG-3920), and DNS ownership
reconciled (ENG-3922).

**DNS + route are Terraform-owned, not Wrangler-owned.** Per EDR-003 / ENG-3922,
Terraform owns the `cli.nexus.xyz` record *and* the Worker route binding; Wrangler
only uploads the script. So `wrangler.toml` declares no `routes` and never sets
`custom_domain` (that conflicts with the Terraform record — nexus#2270).

Cutover steps:

1. **Upload the script (CI, preferred):** run the **Deploy installer Worker**
   workflow (`.github/workflows/installer-deploy.yml`) via *Run workflow* on
   `main`. It authenticates to Google Cloud through Workload Identity Federation
   (no personal Cloudflare access needed), reads the scoped deploy token from
   Secret Manager, and runs `wrangler deploy`. The job is gated on the
   `production` environment (main-only + required reviewer). Manual fallback,
   if you have Cloudflare access to the `nexus.xyz` zone: `wrangler login` (or
   set `CLOUDFLARE_API_TOKEN`), then `cd installer && wrangler deploy`. Either
   path uploads the Worker only; neither touches DNS.
2. **Bind traffic in Terraform (monorepo):** add a `cloudflare_workers_route`
   for `cli.nexus.xyz` → this Worker (`nexus-cli-installer`) and repoint the
   `module "cli"` record (`proxied = true`) off the Firebase CNAME. Atlantis
   plan/apply.
3. **Verify both paths:**
   ```sh
   curl -fsS https://cli.nexus.xyz | head -5            # exchange installer (#!/bin/sh)
   curl -fsS -A 'PowerShell/7.4' https://cli.nexus.xyz | head -5   # exchange PowerShell variant
   curl -fsS https://cli.nexus.xyz/compute | head -5    # compute installer
   ```
4. **Comms/docs:** update any public doc that points compute users at the bare
   one-liner to use `/compute` (or the alias) — the default now installs the
   exchange CLI.

## Configuration

`wrangler.toml` `[vars]` pin which repo/app to install. The Worker validates
each value against `^[A-Za-z0-9._-]+$` before building a URL, so a bad value
fails closed (HTTP 500, no network call) rather than redirecting traffic
elsewhere.
