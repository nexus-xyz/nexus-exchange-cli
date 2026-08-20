# nexus-exchange-cli

`nexus` — a command-line interface for the [Nexus Exchange](https://exchange.nexus.xyz)
API, built on the official [`nexus-exchange`](https://github.com/nexus-xyz/nexus-exchange-rs)
Rust SDK.

> **Status:** the full command surface is wired up — public market data, the
> authenticated account, order placement/cancellation, and live WebSocket
> streaming. It is a thin command/output layer over the SDK: every request goes
> through the SDK's `Client`, which owns request signing, the HTTP/WebSocket
> transport, retries, rate-limit pacing, and the wire types. The CLI adds no
> transport of its own.

## Install

The quickest way — download and run the installer for the latest release:

```sh
# macOS + Linux
curl https://cli.nexus.xyz | sh
```
```powershell
# Windows
irm https://cli.nexus.xyz | iex
```

`cli.nexus.xyz` serves the [cargo-dist](https://opensource.axo.dev/cargo-dist/)
installer for the most recent GitHub release; it picks the shell or PowerShell
variant automatically. The host itself lives in [`installer/`](./installer).

Or pin the invocation to a specific GitHub release artifact:

Prebuilt, checksummed binaries are published for every tagged release
(macOS arm64/x64, Linux x64/arm64, Windows x64) by
[cargo-dist](https://opensource.axo.dev/cargo-dist/). A Windows `.msi` is also
attached to each release. Every artifact carries a detached minisign signature
and a GitHub build-provenance attestation — see
[Verifying downloads](#verifying-downloads). Note that the binaries are **not**
OS code-signed (no Apple Developer ID, no Windows Authenticode); on Windows this
has a visible consequence, described [below](#windows-the-binaries-are-unsigned).

**macOS / Linux** — shell installer:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.sh | sh
```

**Windows** — PowerShell installer:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.ps1 | iex"
```

Windows will warn that the binary is unsigned — see
[Windows: the binaries are unsigned](#windows-the-binaries-are-unsigned).

**Homebrew**:

```sh
brew install nexus-xyz/tap/nexus
```

**cargo-binstall** (prebuilt binary, no compile) — the crate isn't on
crates.io yet, so point binstall at the repo directly:

```sh
cargo binstall --git https://github.com/nexus-xyz/nexus-exchange-cli nexus-exchange-cli
```

**From source**:

```sh
cargo install --path .
# or, from a checkout:
cargo build --release   # binary at target/release/nexus
```

### Windows: the binaries are unsigned

The Windows `.exe` and `.msi` are **not Authenticode-signed**. Windows will say
so, and what you see depends on how the machine is managed:

- **Ordinary Windows.** SmartScreen shows *"Windows protected your PC —
  Microsoft Defender SmartScreen prevented an unrecognized app from starting"*.
  Click **More info → Run anyway**. Annoying, but dismissible.
- **Managed or corporate Windows.** If WDAC or AppLocker is in enforcement, an
  unsigned binary can be **blocked outright**, with no click-through. There is
  no workaround on our side — it needs a policy exception from whoever
  administers the machine.

Unsigned is not unverifiable: both signatures under
[Verifying downloads](#verifying-downloads) cover the Windows artifacts, and the
minisign check in particular is a stronger guarantee of provenance than
Authenticode is. Prefer verifying the download over clicking through the prompt.

This is a deliberate, reversible choice — Windows is 0.2% of downloads and
signing requires a purchased certificate; see the rationale in
[`dist-workspace.toml`](./dist-workspace.toml). If a block affects you, please
open an issue: that is the signal we are watching for.

### Verifying downloads

Every release also ships per-artifact `*.sha256` files and a combined
`sha256.sum`. Beyond checksums, each binary artifact carries two independent
signatures:

**1. minisign signatures** (`.minisig`, offline — no service to trust). Verify
an artifact against the project's public key:

```sh
minisign -Vm nexus-exchange-cli-x86_64-unknown-linux-gnu.tar.gz \
  -P 'RWQ5th6qraoqAGncPLWGZthh5ObywWnTc8j0r1w8e0cX4kH9vuVc06ek'
```

**2. GitHub build-provenance attestations** (proves the artifact was built by
this repo's release workflow):

```sh
gh attestation verify nexus-exchange-cli-x86_64-unknown-linux-gnu.tar.gz \
  --repo nexus-xyz/nexus-exchange-cli
```

### Cutting a release

Releases are automated by [release-please](https://github.com/googleapis/release-please)
(`release-type: rust`), complementary to the cargo-dist pipeline (see
[EDR-003](https://github.com/nexus-xyz/nexus/blob/main/eng/decisions/003-cli-distribution.md)).
You do **not** bump the version or tag by hand.

1. Land changes on `main` using [Conventional Commits](https://www.conventionalcommits.org/)
   (`feat:`, `fix:`, `feat!:` / `BREAKING CHANGE:`).
2. The [`release-please`](./.github/workflows/release-please.yml) workflow keeps a
   standing **release PR** that bumps `version` in `Cargo.toml` + `Cargo.lock` and
   writes `CHANGELOG.md`. The CLI is **pre-1.0** and stays on `0.X.Y`: `feat!:` /
   `BREAKING CHANGE:` → `X` (`0.3.0` → `0.4.0`), `feat:` and `fix:` → `Y`
   (`0.3.0` → `0.3.1`). This is the `bump-minor-pre-major` +
   `bump-patch-for-minor-pre-major` pair in
   [`release-please-config.json`](./release-please-config.json); without them
   stock semver would cut a `1.0.0` off the first breaking change. Both flags are
   held in place by [`tests/release_config.rs`](./tests/release_config.rs), which
   also fails if a `release-as` is ever committed. Going 1.0 is a deliberate act:
   drop both flags *and* the `pre_1_0_bump_policy_stays_configured` test, and
   normal semver resumes. Leave the rest of that file alone — the other guards
   there protect distribution invariants (the `vX.Y.Z` tag shape binstall
   resolves, the draft Release `release.yml` uploads into, manifest/`Cargo.toml`
   lockstep) that hold at every version.
3. Merging that release PR creates the `v<version>` tag.
4. The tag fires the [`Release`](./.github/workflows/release.yml) workflow
   (generated by `dist generate`), which builds, checksums, attests, signs, and
   publishes the binaries and installers.

release-please only edits `Cargo.toml` / `Cargo.lock` / `CHANGELOG.md` / the
manifest — it never touches the generated `release.yml`, so it does not conflict
with the `dist generate --check` assertion in CI. The two pipelines stay
separate by design.

> **Do not edit `CHANGELOG.md` or bump the version manually** — release-please
> owns both and will overwrite or conflict with hand edits.

Builds also run on every PR as a dry-run (`pr-run-mode = "upload"`), so the full
cross-platform build is a blocking check — no release is created and no publish
jobs run on PRs.

#### First release (one-time bootstrap)

The very first cut (`v0.1.0`) needs two one-time nudges, because release-please
derives the next version by bumping the last *released* version — and there is
no prior release to bump from yet:

- `.release-please-manifest.json` is seeded at `0.0.0` (the "nothing released
  yet" baseline), so release-please proposes a brand-new version rather than
  treating the `0.1.0` already in `Cargo.toml` as shipped.
- `release-as: "0.1.0"` in [`release-please-config.json`](./release-please-config.json)
  pins that first proposal to exactly `0.1.0`, independent of how the conventional
  commits would otherwise bump a `0.x` baseline.

To cut it, in order:

1. **Complete the signing setup below first.** The minisign publish job fails
   closed, so a release cut before the key/variable exist would publish an
   *unsigned* `v0.1.0` — defeating the point. Verify `MINISIGN_PUBLIC_KEY` (the
   repo variable *and* the README key) plus the `MINISIGN_SECRET_KEY` /
   `MINISIGN_PASSWORD` / `RELEASE_BOT_APP_ID` / `RELEASE_BOT_PRIVATE_KEY` /
   `HOMEBREW_TAP_TOKEN` secrets are all set.
2. Merge this bootstrap to `main`. release-please opens a `release 0.1.0` PR.
3. Merge that PR → it tags `v0.1.0` and opens the draft Release → `release.yml`
   builds, attests, signs (minisign), uploads into the draft, and undrafts it.
4. **Remove `release-as` immediately afterwards.** It is sticky: left in place it
   pins *every* future release to `0.1.0`, so the next release PR would be stuck
   at the same version. Open a one-line follow-up PR deleting the
   `"release-as": "0.1.0"` line. The manifest will by then read `0.1.0`, so
   subsequent releases bump from there normally. (Leaving it in is fail-safe — it
   blocks the next release loudly rather than mis-versioning, but remove it so the
   flow self-advances.)

After undrafting, confirm the install path end-to-end (this is the actual
"done"):

```sh
# The cargo-dist "latest" installer asset now resolves (no longer 404s):
curl -fsSL https://github.com/nexus-xyz/nexus-exchange-cli/releases/latest/download/nexus-exchange-cli-installer.sh | head -5
# Once the cli.nexus.xyz Worker is deployed, the one-liner installs a working nexus:
curl https://cli.nexus.xyz | sh && nexus --version
```

#### Signing setup (maintainers)

Configure these repository **secrets** before the first signed release. Every
signing-related job fails closed — if a secret is missing the job errors rather
than quietly shipping a release without the signature/formula it promises.

> Do not assume a code-signing step *skips* when its credential is absent.
> `${{ secrets.X }}` expands a missing secret to the empty string, so dist sees
> a credential that is present-and-empty and tries to use it: the macOS path
> imports an empty certificate and fails the build. The Windows path behaved
> that way too, and did not always — it silently skipped until 2026-07-17, then
> began answering `invalid_grant` on every run with nothing in this repo having
> changed, which looked exactly like a certificate expiring and was misdiagnosed
> as one for a month (ENG-9357). Treat "absent credential" as "broken build",
> and do not trust that a signing step is a no-op just because it was yesterday.

| Secret | Used for |
|---|---|
| `RELEASE_BOT_APP_ID` | App ID of the `nexus-release-bot2` GitHub App. `release-please.yml` mints a short-lived installation token from it (`actions/create-github-app-token`) so release PRs / commits / tags are authored by the App (`nexus-release-bot2[bot]`), not a person. |
| `RELEASE_BOT_PRIVATE_KEY` | Private key (`.pem`) for `nexus-release-bot2`, used with the App ID to mint the token. Required so the tag it pushes on release-PR merge triggers the tag-listening `release.yml` — the default `GITHUB_TOKEN` cannot trigger downstream workflows. App scope: `contents:write` + `pull-requests:write` on this repo. |
| `MINISIGN_SECRET_KEY` | minisign secret key contents (per-artifact `.minisig`) |
| `MINISIGN_PASSWORD` | password for the minisign secret key |
| `HOMEBREW_TAP_TOKEN` | push access to the `nexus-xyz/homebrew-tap` repo |
| `CODESIGN_CERTIFICATE`, `CODESIGN_CERTIFICATE_PASSWORD`, `CODESIGN_IDENTITY` | Apple Developer ID code-signing (macOS) — only needed if `macos-sign` is turned on; see below |

Set the `MINISIGN_PUBLIC_KEY` repository **variable** (public keys are not
secret) to the published public key — the same one in "Verifying downloads"
above. This is **required**: the release workflow verifies every signature
against it before uploading and fails closed if it is unset, so the first signed
release is gated until both this variable and the README key are populated.

Generate the minisign keypair offline and keep the secret key off CI:

```sh
minisign -G -p minisign.pub -s minisign.key
# store minisign.key contents in MINISIGN_SECRET_KEY, its password in MINISIGN_PASSWORD,
# and paste minisign.pub into the "Verifying downloads" section above + the var.
```

> **macOS code-signing is currently disabled** (`macos-sign = false`). dist's
> macOS signing path does not gracefully skip a missing certificate — it fails
> the build — and since builds now run on every PR, enabling it without a cert
> would break all builds. To enable: add the `CODESIGN_*` secrets above and flip
> `macos-sign = true` in `dist-workspace.toml`. macOS **notarization** is a
> separate, further step (dist signs but does not notarize, and a loose CLI
> binary cannot be stapled), so it only matters behind a `.pkg`/`.dmg` installer.

> **Windows Authenticode signing is off** — deliberately, and there is no secret
> to configure for it. We never held an SSL.com eSigner credential, so enabling
> it means buying a certificate, not restoring one. The Windows artifacts still
> build and ship, with their `.minisig` and provenance attestation; what users
> see instead is
> [a SmartScreen prompt](#windows-the-binaries-are-unsigned). Full rationale and
> re-enable steps are in [`dist-workspace.toml`](./dist-workspace.toml).

## Usage

Runnable, copy-pasteable recipes for each flow live in [`examples/`](./examples).

```sh
nexus --help

# Public market data
nexus markets                       # tradable markets and their rules
nexus ticker BTC-USDX-PERP          # ticker for one market
nexus tickers                       # tickers for every market
nexus summaries                     # per-market 24h summaries
nexus mark-price BTC-USDX-PERP      # current mark price
nexus market-status BTC-USDX-PERP   # lifecycle / halt status
nexus funding-rates BTC-USDX-PERP --limit 50
nexus orderbook BTC-USDX-PERP       # bids/asks
nexus trades BTC-USDX-PERP --limit 50
nexus candles BTC-USDX-PERP --timeframe 1m --limit 100
nexus health                        # indexer health snapshot

# Per-market data
nexus market summary                       # 24h volume + halt state per market
nexus market status BTC-USDX-PERP          # lifecycle / halt status
nexus market mark-price BTC-USDX-PERP      # current mark price
nexus market adl-events BTC-USDX-PERP --limit 50   # ADL settlements (needs credentials)

# Authenticated account (see Credentials below)
nexus balance                       # balance, collateral, equity, margin
nexus account summary               # portfolio totals + withdrawable balance
nexus account state                 # summary + positions from ONE coherent read
nexus account fees                  # effective maker/taker bps, tier, 30d volume
nexus account portfolio-history --window week --limit 100   # equity/PnL/volume series
nexus account rate-limit            # current rate-limit tier / remaining / reset
nexus positions                     # open positions, with per-position risk detail
nexus fills --limit 50              # recent executions (server-side page, max 1000)
nexus withdrawals --limit 50        # withdrawal history
nexus orders                        # open orders
nexus funding-payments --limit 50   # funding booked against the account
nexus withdrawals                   # withdrawal history

# Trading (prompts for confirmation; pass --yes to skip)
nexus order place --market BTC-USDX-PERP --side buy --type limit \
  --price 84000 --quantity 0.01 --tif GTC
# By-id order commands are routed per market, so they require --market.
# (By-client-id commands are account-scoped and do not.)
nexus order get <ORDER_ID> --market BTC-USDX-PERP          # fetch one order
nexus order get-by-client-id <CLIENT_ORDER_ID>   # …or by your own id
nexus order amend <ORDER_ID> --market BTC-USDX-PERP --price 85000 --quantity 0.02
nexus order batch orders.json       # submit a JSON array of orders ('-' = stdin)
nexus order cancel <ORDER_ID> --market BTC-USDX-PERP
nexus order cancel-by-client-id <CLIENT_ORDER_ID>
nexus order cancel-batch <ORDER_ID> <ORDER_ID>   # several ids, one request
nexus order cancel --market BTC-USDX-PERP        # flatten one market
nexus order cancel --all

# Account management (see Credentials below)
nexus account summary               # equity, PnL, 24h volume, open counts, withdrawable
nexus account state                 # the summary above + every open position, one read
nexus account fees                  # fee schedule (a negative maker fee is a rebate)
nexus account portfolio-history --window day     # day | week | month | all
nexus account deposit 1000          # deposit collateral
nexus account credit --amount 500   # claim testnet USDX (omit --amount for the daily max)
nexus account rate-limit            # rate-limit tier / remaining tokens
nexus account leverage BTC-USDX-PERP 10
nexus account adl-history 0x<ADDRESS>   # ADL settlements touching an account
# Margin mode is NOT settable from the CLI. `nexus account margin-mode` was
# withdrawn (ENG-7740): no endpoint accepts a margin-mode change, so the command
# could only ever fail. Tracking: ENG-7614.

# Wallet-signed auth (EVM key; see Credentials below)
nexus auth login                    # EIP-191 sign-in; prompts for the key,
                                    # stores the session token (mode 0600)
nexus agents register --agent 0x<AGENT_ADDR>   # EIP-712; prompts for the key

# API keys, agents, transfers, sub-accounts
nexus keys list
nexus keys create                   # secret is shown ONCE — store it now
nexus keys delete <KEY_ID>
nexus agents list
nexus agents revoke <AGENT_ADDRESS>
nexus transfers list
nexus transfers create --from <ACCT> --to <SUBACCT> --amount 100
nexus sub-accounts list
nexus sub-accounts create "trading-bot-1"

# Live streaming over WebSocket (Ctrl-C to stop)
nexus ws trades --market BTC-USDX-PERP      # public channels need --market
nexus ws orders fills positions             # account channels (need credentials)

# First-time setup (interactive)
nexus setup
```

The `order batch` file is a JSON array of order objects mirroring the
`order place` flags, with string amounts to preserve precision:

```json
[
  {"market": "BTC-USDX-PERP", "side": "buy", "type": "limit",
   "price": "84000", "quantity": "0.01", "tif": "gtc"},
  {"market": "ETH-USDX-PERP", "side": "sell", "type": "market", "quantity": "1"}
]
```

Every subcommand supports `--help`.

### Reading portfolio data

Three rules apply to `account summary` / `account state` / `positions`, and they
matter before you act on a number:

- **`-` (JSON `null`) is not zero.** The server nulls a figure it cannot derive
  rather than fabricating one; for a position's risk fields it puts the reason in
  a companion `*_error`, which `nexus positions` lists under the table. An
  unreported aggregate rendered as `0` would make an underwater account look
  flat, so the CLI never substitutes one.
- **A failed read is not an empty account.** `account summary` and `account
  state` derive `withdrawable` from the exchange's authoritative margin view and
  fail closed (`502 authoritative_margin_unavailable`) instead of reporting an
  estimate. The CLI exits non-zero and says the balance is *unknown*; retry
  rather than treating it as a zero balance.
- **Prefer `account state` over `account summary` + `positions`.** Those are two
  independent requests, and a fill landing between them returns an aggregate that
  disagrees with the position list. `account state` gets both halves from one
  server-side read, so they cannot tear.

`withdrawable` is engine-authoritative free margin floored at zero — it already
nets initial margin and order reservations out of equity, so it is what can
actually leave the account. Prefer it over `available margin` when deciding how
much to withdraw.

### Network selection

The CLI targets a **network**, not a release channel. By default that is
**testnet** — play funds credited by the faucet — because the default must never
be a network that moves real money.

```sh
nexus markets                                    # testnet (the default)
nexus --network local markets
nexus --network dev markets                      # a custom network you declared
```

| Flag | Env | Default |
|---|---|---|
| `--network <mainnet\|testnet\|local\|LABEL>` | `NEXUS_NETWORK` | `testnet` |
| `--base-url <URL>` | `NEXUS_BASE_URL` | — (**deprecated**, see [below](#the-base-url-override-is-deprecated); overrides `--network`) |

| Network | Funds | Notes |
|---|---|---|
| `mainnet` | **real** | **Not reachable in this release.** The SDK refuses every request locally rather than guess a host — `api.nexus.xyz` does not resolve yet, and its base uses a different path layout than the one the SDK signs. Use a [custom network](#custom-networks) to target a host you control. |
| `testnet` | play | The default and the safe target. Served by the legacy `exchange.nexus.xyz` gateway. |
| `local` | play | A locally run indexer. A developer convenience, never a fallback. |
| *`LABEL`* | declared | A [custom network](#custom-networks) you describe in the config file — your own environment, a preview host, a sandbox. |

> **`stable` and `beta` were retired** in `nexus-exchange` 0.8.0 and are no
> longer accepted. They named *release channels*, which is how a play-funds host
> came to be labelled "production" — so `stable`'s replacement is **`testnet`**,
> not `mainnet`. That mapping is the correction, not a rename: reading `stable`
> as `mainnet` has it backwards. A config file still naming one keeps working
> (the default is the same host it pointed at) and prints a warning telling you
> what to change.

A config-file `network` the CLI cannot parse — a retired channel name, or a typo
like `mainet` — never changes the network silently: it warns on stderr (so
`--output json` stays clean on stdout), names the network actually used, and
falls back to the default. `--network` itself rejects an unknown value outright,
because you typed it and can retype it.

Credentials are minted **per network** and are invalid on any other, so an API
key configured for one network will not authenticate against another. The CLI
stores them per network too — see [Per-network credentials](#per-network-credentials).

#### Custom networks

A deployment this CLI does not name still has to be reachable from it. Enumerating
such hosts in a published binary would put them in the package permanently and
discoverably, and the list would need extending every time one was added — so
**you supply the URL and the CLI ships none**.

Declare each one under `custom_networks` in the config file, keyed by a label,
then select it by that label:

```jsonc
{
  "network": "dev",
  "custom_networks": {
    "dev": {
      "base_url": "https://exchange.example.com/api/exchange",
      "funds": "play",          // required: "real" | "play" | "unknown"
      "faucet": true,           // optional, assumed absent
      "ws_url": "wss://stream.example.com/ws",  // optional, never derived
      "direct_base_url": "...", // optional, defaults to base_url
      "chain_id": 393           // optional EIP-712 domain, never guessed
    }
  },
  "networks": {
    "dev": { "api_key": "nx_...", "api_secret": "..." }
  }
}
```

```sh
nexus --network dev markets
```

A custom network is **client-side only**: it is never a value the server accepts,
and nothing about it is transmitted.

- **`funds` is required and has no default.** Both booleans are wrong — `play`
  makes every guardrail lie in the direction that costs money, `real` makes
  development unusable — so the classification is a third thing the caller
  declares. It is read case-insensitively (`"Play"` is `play`); a missing or
  unreadable value warns and resolves to **unknown**, which *fails closed*: reads
  still work, and anything that moves or mints funds is refused rather than
  assumed safe.
- **The label, not the URL, is the credential namespace.** Two stages on one host
  keep separate credentials, and a stage keeps its credentials across a host
  move. Labels are limited to `A-Za-z0-9._-`, capped at 64 characters, and may
  not be `.`, `..`, or a built-in network's name — a label is a storage key, so
  one that could address another network's credentials is refused outright. A
  label is matched **exactly**, case included, and an entry the file declares but
  cannot select says so rather than reporting the network as unknown.
- **Nothing is inferred from the URL.** The WebSocket origin is a separate host
  and is never derived from the REST base, so `nexus ws` refuses rather than
  connecting to a guess. The EIP-712 signing domain is likewise absent until
  declared: a signature made under the wrong domain may be *valid on a different
  network*, so `agents register` against a stage that declares no `chain_id` is
  **refused** rather than signed under the exchange's own chain — pass
  `--chain-id`, or read it off that host's `GET /metadata` and declare it. (The
  built-in networks publish no `chain_id` either and still default to `393`,
  which is theirs; the refusal is for a target that could genuinely be elsewhere.)
- **URLs are validated** — `http(s)` scheme, a host, no `user:pass@` userinfo, no
  query or fragment, no whitespace. Each rejection is a URL that would otherwise
  build a *wrong* request rather than merely fail. The host itself is never
  checked against any list, which is the entire point.

#### The base-URL override is deprecated

`--base-url`, `NEXUS_BASE_URL` and the config file's top-level `base_url` are
**deprecated** (ENG-10956) in favour of a `custom_networks` entry selected with
`--network <label>`.

**Nothing about them has changed, and nothing is removed.** They still work, they
still take precedence over `--network`, and this release only adds a notice. A
future release may remove them; that is a separate, breaking change.

They are deprecated because a bare URL cannot carry the two things a declared
stage does:

- **It does not declare funds.** A URL says nothing about what its host moves, so
  the destination's funds are `unknown` and the fund-moving commands are refused
  rather than inheriting the named network's safety flags.
- **It does not namespace credentials.** The override redirects the request
  without changing which key is presented, so stored credentials stay filed under
  whichever network was selected — not under the host you pointed at.

Using any of the three prints a one-line notice on stderr naming which one
resolved the target. It stays on stderr even under `--output json`, so stdout
remains a clean document: scripted callers are exactly who needs to see it.

Migrating is a config-file edit:

```jsonc
// before — deprecated
{ "base_url": "https://exchange.example.com/api/exchange" }

// after
{
  "network": "dev",
  "custom_networks": {
    "dev": {
      "base_url": "https://exchange.example.com/api/exchange",
      "funds": "play"
    }
  }
}
```

> **Credentials do not carry over.** They are stored per label, so a stage that
> was reached via `base_url` while `testnet` was selected has its key filed under
> `testnet`, and the new `dev` label starts empty. Run `nexus setup` for the new
> stage, or move the section by hand. This is the one part of the migration that
> is not purely cosmetic — see [Per-network credentials](#per-network-credentials).

#### Real-funds guardrails

Three things stand between you and an accidental real-funds action. They are
local: each one fires before a request is built.

| Guardrail | Behavior |
|---|---|
| Active-network banner | Selecting a **real-funds** network prints a warning to **stderr** before the command runs, naming the network. Play-funds networks stay silent, so the banner keeps meaning something. |
| Faucet refusal | `nexus account credit` claims synthetic USDX and is **refused** anywhere it is not known to mint play funds: on a real-funds network (use `nexus account deposit <amount>`), on a target whose funds are undeclared, and on a play-funds stage that declares no faucet. |
| First-trade acknowledgement | The first `order place` / `order batch` on a real-funds network asks for a one-time confirmation, recorded per network in the config's `acknowledged_networks` so it is asked once per network, not on every order. |

These key off the target's declared **funds**, not off the name `mainnet`: a
custom network can declare `"funds": "real"`, so each guard matches *play funds
positively* rather than negating the real case — which is how an unclassified
target would otherwise slip through as safe. Acknowledging one real-funds network
says nothing about another.

The banner goes to stderr, so `--output json` stays machine-parseable.

`--yes` skips the acknowledgement, exactly as it skips every other confirmation
in this CLI — otherwise mainnet could not be scripted at all. It does not record
the acknowledgement, so a later interactive trade is still asked once. Without a
terminal and without `--yes`, the trade is refused rather than assumed.

> **Mainnet is not reachable in this release.** The guardrails above are in place
> ahead of the cutover that makes it reachable (ENG-8865); until then a mainnet
> request is refused by the SDK regardless. They are tested locally, not against
> a live real-funds host.

### Output format

By default commands print human-readable tables. Pass `--output json` (or set
`NEXUS_OUTPUT=json`) to emit pretty-printed JSON instead — handy for scripting
and piping into tools like `jq`. It works for every data command; `nexus ws`
emits one JSON object per line so it streams cleanly into `jq`.

```sh
nexus --output json markets
NEXUS_OUTPUT=json nexus ticker BTC-USDX-PERP
nexus --output json health | jq .
nexus --output json ws trades --market BTC-USDX-PERP | jq .payload
```

| Flag | Env | Default |
|---|---|---|
| `--output <human\|json>` | `NEXUS_OUTPUT` | `human` |

### Credentials

Authenticated commands (`balance`, `account …`, `positions`, `fills`,
`withdrawals`, `orders`, `order …`, and account WebSocket channels) HMAC-sign
each request. Public market-data commands don't need credentials.

Credentials resolve in this order, highest priority first:

1. `--api-key` / `--api-secret` flags
2. `NEXUS_API_KEY` / `NEXUS_API_SECRET` environment variables
3. the config file written by `nexus setup`

| Flag | Env |
|---|---|
| `--api-key <KEY>` | `NEXUS_API_KEY` |
| `--api-secret <SECRET>` | `NEXUS_API_SECRET` |

```sh
nexus setup                 # interactive; stores config at
                            # $XDG_CONFIG_HOME/nexus/config.json (mode 0600)

# …or per-shell:
export NEXUS_API_KEY=nx_...
export NEXUS_API_SECRET=...
nexus balance
```

Prefer `nexus setup` or the environment variables over `--api-secret`: flags are
visible in your shell history and in the process list. The secret is never
echoed during setup, never printed back, and the config file is created
owner-read/write only (`0600`).

#### Per-network credentials

A key is valid on **exactly one network**, so the config file stores credentials
under a `networks` map rather than one flat slot. Testnet, mainnet and any
[custom network](#custom-networks) keys coexist, and the CLI presents only the
section for the network you selected:

```json
{
  "network": "testnet",
  "networks": {
    "testnet": { "api_key": "nx_test...", "api_secret": "..." },
    "mainnet": { "api_key": "nx_main...", "api_secret": "..." },
    "dev": { "api_key": "nx_dev...", "api_secret": "..." }
  }
}
```

```sh
nexus setup                    # answer `testnet`, enter that network's key
nexus setup                    # run again, answer `mainnet` — the first is kept

nexus balance                  # uses the testnet section
nexus --network mainnet balance   # uses the mainnet section, or nothing
```

Which section is used follows the same order as network selection:
`--network` / `NEXUS_NETWORK`, then the config file's `network`, then the
default (`testnet`). Some consequences worth knowing:

- **A key is never offered to the wrong network.** If only `testnet` is
  configured, `--network mainnet` runs *unauthenticated* rather than sending a
  testnet key to a real-funds host. You get the "no credentials are configured"
  error, not a signature failure from the server.
- **A custom network is keyed by its label**, so two stages sharing one host
  still get separate slots. That is the collision the label exists to prevent,
  and it would be between environments with different funds semantics.
- **`--base-url` does not change the namespace.** It redirects the request; it
  does not change who you are. Pointing at a proxy or a tunnel in front of a
  network still presents that network's credentials. It *does* change what the
  target is known to move — see [Custom networks](#custom-networks). This is one
  of the two reasons it is [deprecated](#the-base-url-override-is-deprecated), and the reason
  migrating to a label means re-running `nexus setup` for it.
- **Flags and env are not namespaced.** `--api-key` / `NEXUS_API_KEY` (and the
  session-token equivalents) apply to whichever network is selected — they are a
  per-invocation override you just typed. Only the persisted layer, the one that
  can hold several networks at once, is sectioned.
- **`nexus auth login` stores its session token in the active network's
  section**, for the same reason: the token is minted against one network's
  indexer and authenticates nowhere else.

**Upgrading:** a config written by an earlier version keeps its credentials at
the top level. Those are read as belonging to the network that file names (or
the default, if it names none), so nothing breaks and — importantly — an old
testnet key is not promoted to mainnet. The file is rewritten into the layout
above the next time anything writes to it; you can also just re-run
`nexus setup`.

#### Wallet sign-in (session token)

As an alternative to an HMAC key pair, you can authenticate with an EVM wallet.
`nexus auth login` reads a raw private key, signs the fixed sign-in challenge
(EIP-191), and exchanges it for a **session token** stored in the same config
file (mode `0600`) under the active network's `session_token`. The session token
authenticates
session-scoped routes; the HMAC pair, when present, takes precedence as the
request signer.

The private key is read from `--private-key`, the `NEXUS_PRIVATE_KEY`
environment variable, or — when neither is set and you're at a terminal — a
hidden interactive prompt. It is used only to produce the signature and is
**never written to disk or echoed**.

| Flag | Env |
|---|---|
| `--private-key <KEY>` (on `auth login` / `agents register`) | `NEXUS_PRIVATE_KEY` |
| `--session-token <TOKEN>` | `NEXUS_SESSION_TOKEN` |

```sh
export NEXUS_PRIVATE_KEY=0x<your-evm-key>
nexus auth login            # stores the session token; prints the address
nexus balance               # now authenticated via the stored token

# Register an agent key, authorized by an EIP-712 signature from your wallet
# (unauthenticated request — the signature is the authorization):
nexus agents register --agent 0x<agent-address> --label my-bot
```

`agents register` defaults the expiry to 30 days out, the nonce to the current
Unix-ms timestamp, and the EIP-712 `chain-id` to the exchange chain (`393`);
override any with `--expires-at` / `--nonce` / `--chain-id`.

### Shell completions

Generate a completion script for your shell and source it:

```sh
# Bash
nexus completions bash > ~/.local/share/bash-completion/completions/nexus

# Zsh
nexus completions zsh > ~/.zfunc/_nexus   # ensure ~/.zfunc is in $fpath

# Fish
nexus completions fish > ~/.config/fish/completions/nexus.fish

# PowerShell
nexus completions powershell >> $PROFILE

# Elvish
nexus completions elvish >> ~/.elvish/rc.elv
```

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

CI runs the same three checks on every push and pull request.

### Direct-service base (`/api/v1`)

The gateway REST proxy is being eliminated: each backend service now serves its
own REST API directly, at the **host-root `/api/v1`** prefix (parent
[ENG-4740](https://linear.app/nexus-labs/issue/ENG-4740)). The migration is
**dual-stack** ([ENG-4751](https://linear.app/nexus-labs/issue/ENG-4751)) — the
legacy `/api/exchange` gateway paths stay live, so endpoints without an `/api/v1`
variant keep routing through the gateway.

The CLI is a thin layer over the [`nexus-exchange`](https://github.com/nexus-xyz/nexus-exchange-rs)
SDK and issues no path of its own: the SDK picks the base per request off the
`/api/v1/` prefix. Runtime routing therefore flips to `/api/v1` when this crate
bumps its `nexus-exchange` dependency to the regenerated SDK release
([ENG-4947](https://linear.app/nexus-labs/issue/ENG-4947), `nexus-exchange-rs`
PR #85). The `.api-version` / `endpoints.txt` bookkeeping below tracks that
surface so the two move together.

### API coverage

The CLI targets a specific released version of the Exchange API spec, pinned in
[`.api-version`](./.api-version) — matching the wrapped
[`nexus-exchange`](https://github.com/nexus-xyz/nexus-exchange-rs) SDK, which
pins and sends the same tag as `X-Nexus-Api-Version` on every request.

<!-- api-version-sync:start -->

Currently targets Exchange API spec **`v0.8.1`** — the version pinned and sent as `X-Nexus-Api-Version` by `nexus-exchange` **`0.9.1`**.

<!-- api-version-sync:end -->

That pin is a **derived value, not a choice**. The CLI issues no HTTP of its own, so
the crate decides which spec version is actually spoken; a pin that disagrees with
the crate's is simply a false statement about what the binary sends.
[`scripts/check_sdk_parity.py`](./scripts/check_sdk_parity.py) enforces that in CI.

[`endpoints.txt`](./endpoints.txt) lists the spec operations the CLI's commands
actually exercise, and [`scripts/check_spec_drift.py`](./scripts/check_spec_drift.py)
verifies — in the `spec-drift` CI workflow, on **every** pull request — four
invariants against the spec:

1. every endpoint in `endpoints.txt` exists in the pinned spec (no rename/typo/
   removal slips through);
2. that set **equals** the SDK methods the CLI actually calls (parsed from
   `src/main.rs` / `src/wsclient.rs` and mapped through `METHOD_OP`) — real
   equality in both directions, so an unlisted call and an uncalled listing both
   fail — modulo two documented allowlists: `CODE_ONLY_OPS` (implemented but ahead
   of the pinned spec) and `NON_REST_TARGETS` (reached without a named REST call,
   i.e. the WebSocket upgrade);
3. neither allowlist holds a stale exemption — an entry nothing calls, an entry
   the pinned spec has since caught up with, or one whose `METHOD_OP` verb doesn't
   match a path the spec does define;
4. no source file outside the two scanned ones reaches the SDK — anywhere under
   `src/`, at any depth — so moving a command handler into a new module (including
   a nested `src/commands/…`) can't silently under-count coverage.

Two more run in the same workflow, checking the CLI against the **crate** rather
than the spec — read straight out of the published `.crate` tarball, so no token is
needed and no assumption is made about how `nexus-exchange-rs` names its tags:

5. `.api-version` **equals** the pinned crate's own `.api-version`;
6. every `endpoints.txt` line is an operation the crate actually **wraps** — the
   CLI reaches the API only through named SDK methods, so the SDK's manifest is the
   ceiling. A subset, deliberately: the SDK wraps considerably more than the CLI
   exposes, and those are coverage gaps to report, not failures.

And two guard the README's own claims about all of the above, because both of them
had gone stale in silence while every check stayed green:

7. the coverage sentence below **equals** the ratio the checker just computed,
   against the pin it was measured with — it had claimed `38 of 98 (38.8%)` against
   `v0.7.2` for two spec releases while the pin was `v0.8.1`, because the checker
   printed a number and nothing compared it to the one committed beside it;
8. the bot-managed line above names the crate `Cargo.lock` actually resolves — which
   is the third of "the pin, the README line, and the crate" that nothing
   implemented, and is how the 0.9.0 → 0.9.1 bump left `main` claiming 0.9.0 while
   shipping 0.9.1.

Both fail on a claim that is wrong **or** unparseable: a guard that stops matching
must not silently stop guarding. Both are also repairable rather than hand-edited —
`check_spec_drift.py --sync-coverage <spec>` for 7, `sync_sdk_version.py --repair`
for 8 — which is what keeps a bot's pin bump mergeable without a human retyping
numbers into prose.

Invariant 6 is what makes the original ENG-7962 bug structurally impossible.
`amend_order` was mapped to `PUT /orders/{order_id}` while the SDK issues `PATCH`;
the spec defines a PATCH there and PUT operations elsewhere, so no spec-level check
could settle it — but the SDK's manifest lists no PUT on that path, which fails
immediately and names the real verb.

Both checkers have their own self-tests —
[`test_check_spec_drift.py`](./scripts/test_check_spec_drift.py) and
[`test_sdk_parity.py`](./scripts/test_sdk_parity.py) — which defeat each invariant
in turn and assert the check goes red. They run ahead of the checks they cover,
because a green run only means something if a green run *can* fail.

The check also prints a coverage number: the CLI currently exercises **38 of 68**
spec operations (**55.9%**), measured against the pinned `v0.8.1` spec.

**The denominator counts operations, not paths.** The spec dual-mounts most
operations — `GET /account` and `GET /api/v1/account` are one operation at two
mounts — and the CLI, like every other client surface, targets exactly one mount
per operation. Counting both put every mount in the denominator while the numerator
could only ever hold one of each, so a surface covering everything perfectly still
scored well under 100% and the number could never read full. That is
[ENG-10035](https://linear.app/nexus-labs/issue/ENG-10035); the twins are now
collapsed. At `v0.8.1` the literal count was `38 of 101 (37.6%)` against the same
38 commands.

The remainder are genuinely untargeted operations, not bookkeeping — the admin,
stats, bridge and funding surfaces, `orders/preview`, `orders/history`,
`positions/closed`, `cancel-on-disconnect`, the auth/token endpoints, and more.
`check_spec_drift.py` prints the full list on every run, under
`Not covered by the CLI`; that output is the enumeration, deliberately not a copy
of it here. A hand-maintained list is exactly the kind of claim invariants 7 and 8
exist to stop trusting: nothing checks it, and it goes stale on the next spec
release.

Operations the spec marks `deprecated` leave **both** sides of the ratio, matching
the dashboard collector. `v0.8.1` deprecates nothing, so that filter is a no-op
today; it is here because deprecating the legacy gateway mounts
([ENG-4740](https://linear.app/nexus-labs/issue/ENG-4740)) is the stated direction,
and that is the moment an unported filter would start disagreeing with the
dashboard while invariant 7 held the disagreement in place. A deprecated operation
the CLI still targets is reported — it is a wrapper the CLI is going to lose — but
it is not a failure, because the mount is still served.

Collapsing is used **only** to key the ratio. Existence is always matched
literally, and uncovered operations are reported as their literal mounts, because
a canonical label can name a mount the spec never documents — the bridge domain is
`/api/v1`-native — and acting on one would ship a command that 404s
([ENG-8463](https://linear.app/nexus-labs/issue/ENG-8463)).

The interfaces dashboard does not scrape this line, or any other output of this
workflow: it reads `endpoints.txt` and the spec and computes the ratio itself, in
the monorepo's `collect-interfaces-metrics.py`. So agreement between the two is not
something either side can observe — it has to be built in. Both of that collector's
rules are ported here rather than reimplemented: `canonical_op` from its
`normalise_op` (`/api/v1` stripped only as a prefix, method upper-cased), and the
deprecated split from its `_spec_ops`. The self-tests pin the same edge cases that
collector's own agreement test pins, because two collectors with opposite
conventions is the root cause ENG-10035 documents. The cross-check against the live
collector is manual: this repo's CI has no monorepo checkout to import.

Run it locally with a fetched spec:

```sh
curl -fsSL https://raw.githubusercontent.com/nexus-xyz/nexus-exchange-api/$(cat .api-version)/openapi.json -o openapi.pinned.json
python3 scripts/check_spec_drift.py openapi.pinned.json
python3 scripts/test_check_spec_drift.py   # no network needed
python3 scripts/test_sdk_parity.py         # no network needed
```

Coverage is **structurally capped by the SDK**: the CLI is a thin layer over
`nexus_exchange::Client` and issues no path of its own, so its 38 is a subset of
what that crate wraps, not an independent number. Two consequences worth knowing
before reading the figure as a CLI decision: the CLI cannot reach an operation the
crate has no wrapper for (bridge deposits, for instance, are wrapped by the SDK
but have no CLI command yet — a real gap, and one the drift check cannot see,
since it can only flag a *mismatched* manifest, never a *missing* one), and a
spec bump that needs new operations can't be satisfied here until
`nexus-exchange-rs` ships them.

See [`examples/`](./examples) for copy-pasteable recipes covering each flow.

### Keeping the pin current

The CLI follows the **crate**, not the spec — `api → rs → cli`, never `api → cli`.
This is the one place the CLI diverges from the rest of the client fleet, and it
follows from wrapping an SDK instead of implementing the spec: `nexus-exchange-py`,
`-ts` and `-mcp` generate against the spec directly, so a spec release is
immediately actionable for them. Here it is not. New operations are unreachable
until the crate wraps them, and advancing `.api-version` without a crate release
would make the pin claim a version the binary never sends. So this repo is
deliberately **not** a target of the api repo's `spec-released` fan-out
([ENG-8464](https://linear.app/nexus-labs/issue/ENG-8464)).

| | question | mechanism |
| -- | -- | -- |
| **Verification** | does the pin still match the code? | `spec-drift.yml` → `check_spec_drift.py` |
| **Verification** | is the pin even the right one? | `spec-drift.yml` → `check_sdk_parity.py` |
| **Detection** | has a newer crate been published? | `sdk-autobump.yml` → `sync_sdk_version.py` |

`sdk-autobump.yml` polls crates.io daily (and on demand). When a newer
`nexus-exchange` exists it bumps `Cargo.toml`/`Cargo.lock`, copies the crate's own
pin into `.api-version`, updates the managed README block, classifies any spec delta
with [oasdiff](https://github.com/oasdiff/oasdiff), and opens a labelled PR. The
poll needs no secret and no cross-repo permission: crates.io wants only a
User-Agent, and the crate's `.api-version` and `endpoints.txt` come from the
published tarball.

Everything else — `endpoints.txt`, `METHOD_OP`, the coverage numbers — stays
human-owned. Note that a crate bump can change the SDK's **Rust API**, not just its
paths (0.7.0 added a `limit` parameter to `fetch_my_trades`), which no spec-level
check would notice; the workflow therefore runs `cargo check` itself and reports the
result in the PR. Check the lag by hand with:

```sh
python3 scripts/sync_sdk_version.py --check   # newer crate available?
python3 scripts/check_sdk_parity.py           # pin + manifest vs the crate
```

Three things gate such a PR landing unattended, none in this repo's gift:
`allow_auto_merge` is disabled here (the workflow probes it and says so in the PR
body rather than silently no-opping); a PR opened with the default `GITHUB_TOKEN`
does not trigger `spec-drift`/CI, so a `SDK_DISPATCH_TOKEN` secret is needed for the
checks to run at all; and
[ENG-4149](https://linear.app/nexus-labs/issue/ENG-4149) provisions the ruleset
bypass. Until those are resolved a human merges the PR — which the body says
plainly rather than implying otherwise.

See [`examples/`](./examples) for copy-pasteable recipes covering each flow.

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE), at
your option.
