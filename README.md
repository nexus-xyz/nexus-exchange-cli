# nexus-exchange-cli

`nexus` — a command-line interface for the [Nexus Exchange](https://exchange.nexus.xyz)
API, built on the official [`nexus-exchange`](https://github.com/nexus-xyz/nexus-exchange-rs)
Rust SDK.

> **Status:** the full command surface is wired up — public market data, the
> authenticated account, order placement/cancellation, and live WebSocket
> streaming. The upstream SDK is still a skeleton (it ships only the public
> market-data methods), so the CLI carries its own HMAC-signed HTTP client and
> WebSocket transport — mirroring the SDK's `reqwest`/`tokio-tungstenite` stack —
> until those endpoints land in the SDK proper.

## Install

```sh
cargo install --path .
# or, from a checkout:
cargo build --release   # binary at target/release/nexus
```

## Usage

```sh
nexus --help

# Public market data
nexus markets                       # tradable markets and their rules
nexus ticker BTC-USDX-PERP          # ticker for one market
nexus orderbook BTC-USDX-PERP       # bids/asks
nexus trades BTC-USDX-PERP --limit 50
nexus candles BTC-USDX-PERP --timeframe 1m --limit 100
nexus health                        # indexer health snapshot

# Authenticated account (see Credentials below)
nexus balance                       # balance, collateral, equity, margin
nexus positions                     # open positions
nexus fills --limit 50              # recent executions
nexus orders                        # open orders

# Trading (prompts for confirmation; pass --yes to skip)
nexus order place --market BTC-USDX-PERP --side buy --type limit \
  --price 84000 --quantity 0.01 --tif GTC
nexus order cancel <ORDER_ID> --market BTC-USDX-PERP
nexus order cancel --all --market BTC-USDX-PERP

# Live streaming over WebSocket (Ctrl-C to stop)
nexus ws trades --market BTC-USDX-PERP      # public channels need --market
nexus ws orders fills positions             # account channels (need credentials)

# First-time setup (interactive)
nexus setup
```

Every subcommand supports `--help`.

> **Heads up:** the live API spec (`v0.3.3`) and the deployed server differ in a
> couple of places, and the CLI follows the *server*: order responses use
> `limit_price` and a byte-array `account_id` (rendered as `0x…`), single-order
> cancel requires `--market`, and there is no single-order `GET` (so there is no
> `order get` command — list with `nexus orders`).

### Network selection

By default the CLI targets the **stable** network. Override per-invocation:

```sh
nexus --network beta markets
nexus --network local markets
nexus --base-url http://127.0.0.1:9090 markets   # any custom base URL
```

| Flag | Env | Default |
|---|---|---|
| `--network <stable\|beta\|local>` | `NEXUS_NETWORK` | `stable` |
| `--base-url <URL>` | `NEXUS_BASE_URL` | — (overrides `--network`) |

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

Authenticated commands (`balance`, `positions`, `fills`, `orders`, `order …`,
and account WebSocket channels) HMAC-sign each request. Public market-data
commands don't need credentials.

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

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache-2.0](./LICENSE-APACHE), at
your option.
