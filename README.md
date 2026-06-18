# nexus-exchange-cli

`nexus` — a command-line interface for the [Nexus Exchange](https://exchange.nexus.xyz)
API, built on the official [`nexus-exchange`](https://github.com/nexus-xyz/nexus-exchange-rs)
Rust SDK.

> **Status: early.** Tracks the SDK; commands land as SDK endpoints do. Today
> the public market-data endpoints are wired up.

## Install

```sh
cargo install --path .
# or, from a checkout:
cargo build --release   # binary at target/release/nexus
```

## Usage

```sh
nexus --help
nexus markets                 # list tradable markets and their rules
nexus ticker BTC-USDX-PERP    # ticker for one market
nexus health                  # indexer health snapshot
```

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
and piping into tools like `jq`. This works for `markets`, `ticker`, and
`health`.

```sh
nexus --output json markets
NEXUS_OUTPUT=json nexus ticker BTC-USDX-PERP
nexus --output json health | jq .
```

| Flag | Env | Default |
|---|---|---|
| `--output <human\|json>` | `NEXUS_OUTPUT` | `human` |

### Credentials

API credentials are read from flags or the environment. The market-data
commands above are unauthenticated, so they are optional today; they are wired
up for the authenticated endpoints the SDK adds in follow-ups.

| Flag | Env |
|---|---|
| `--api-key <KEY>` | `NEXUS_API_KEY` |
| `--api-secret <SECRET>` | `NEXUS_API_SECRET` |

```sh
export NEXUS_API_KEY=...
export NEXUS_API_SECRET=...
nexus markets
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
