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
| [`market_data.sh`](./market_data.sh) | Browse markets, tickers, book, trades, candles, funding, mark price, status, ADL events, health | `GET /markets`, `/tickers`, `/markets/{id}/{ticker,orderbook,trades,candles,funding,mark-price,status,adl-events}`, `/markets/summary`, `/health` |
| [`account.sh`](./account.sh) | Inspect a funded account: balance, positions, fills, open orders, withdrawals, funding payments, transfers, sub-accounts, ADL history, rate limit | `GET /account`, `/positions`, `/fills`, `/orders`, `/withdrawals`, `/funding-payments`, `/transfers`, `/sub-accounts`, `/account/{address}/adl-history`, `/account/rate-limit` |
| [`trading.sh`](./trading.sh) | Full order lifecycle: place, fetch (by id / client id), amend, batch, and every cancel variant (one, by client id, batch, per-market, all) | `POST /orders`, `/orders/batch`, `/orders/batch-cancel`, `GET /orders/{id}`, `/orders/by-client-id/{id}`, `PUT /orders/{id}`, `DELETE /orders/{id}`, `/orders/by-client-id/{id}`, `DELETE /orders` |
| [`keys_and_agents.sh`](./keys_and_agents.sh) | Onboarding: wallet sign-in → HMAC API-key management → agent registration and revocation | `POST /auth/login`, `GET/POST /keys`, `DELETE /keys/{id}`, `POST /agents/register`, `GET /agents`, `DELETE /agents/{address}` |
| [`streaming.sh`](./streaming.sh) | Stream live public and account channels over WebSocket, incl. mixed subscriptions and `--since` resume | `POST /ws/token`, `GET /ws` |
| [`north_star.sh`](./north_star.sh) | **North star, unassisted:** faucet-fund → place a resting order → inspect → cancel, top to bottom against testnet with no editing | `POST /account/credit`, `GET /account`, `/markets/{id}/mark-price`, `POST /orders/batch`, `GET /orders`, `/orders/by-client-id/{id}`, `/positions`, `/fills`, `DELETE /orders/by-client-id/{id}` |
| [`batch_orders.json`](./batch_orders.json) | Sample input for `nexus order batch` | — |

> Every command takes a global `--output json` flag for machine-readable output,
> and `--network <stable\|beta\|local>` / `--base-url <url>` to choose the target.
> `north_star.sh` is the one script meant to run end to end unedited; the others
> are recipes with `<PLACEHOLDER>` ids to run a line at a time.
