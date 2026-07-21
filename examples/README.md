# `nexus` CLI examples

Copy-pasteable recipes for the core flows the CLI supports. Each example is a
small shell script under this directory; they are plain `nexus` invocations, so
you can run a line at a time. They double as documentation for the command
surface that [`endpoints.txt`](../endpoints.txt) measures against the pinned
spec ([`.api-version`](../.api-version)).

Build the binary first (or install a release — see the top-level
[README](../README.md)):

```sh
cargo build --release    # produces target/release/nexus
export PATH="$PWD/target/release:$PATH"
```

The public market-data commands need no credentials. Authenticated commands read
credentials from `nexus setup`, from `--api-key`/`--api-secret`, or from the
`NEXUS_API_KEY` / `NEXUS_API_SECRET` environment variables.

| Example | Flow | Spec ops exercised |
| --- | --- | --- |
| [`market_data.sh`](./market_data.sh) | Browse markets, tickers, book, trades, candles, funding, mark price, status, health | `GET /markets`, `/tickers`, `/markets/{id}/{ticker,orderbook,trades,candles,funding,mark-price,status}`, `/markets/summary`, `/status` |
| [`account.sh`](./account.sh) | Inspect a funded account: balance, positions, fills, open orders, withdrawals, rate limit | `GET /account`, `/positions`, `/fills`, `/orders`, `/withdrawals`, `/account/rate-limit` |
| [`trading.sh`](./trading.sh) | Place, fetch, amend, batch, and cancel orders | `POST /orders`, `/orders/batch`, `GET /orders/{id}`, `PUT /orders/{id}`, `DELETE /orders/{id}`, `DELETE /orders` |
| [`keys_and_agents.sh`](./keys_and_agents.sh) | Manage HMAC API keys and registered agent keys | `GET/POST /keys`, `DELETE /keys/{id}`, `GET /agents`, `DELETE /agents/{address}` |
| [`streaming.sh`](./streaming.sh) | Stream live public and account channels over WebSocket | `POST /ws/token`, `GET /ws` |
| [`batch_orders.json`](./batch_orders.json) | Sample input for `nexus order batch` | — |

> Every command takes a global `--output json` flag for machine-readable output,
> and `--network <stable\|beta\|local>` / `--base-url <url>` to choose the target.
