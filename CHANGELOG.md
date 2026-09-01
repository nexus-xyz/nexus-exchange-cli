# Changelog

## [0.5.0](https://github.com/nexus-xyz/nexus-exchange-cli/compare/v0.4.0...v0.5.0) (2026-09-01)


### ⚠ BREAKING CHANGES

* **cli:** delete the phantom code-only ops, seal the allowlist (ENG-12369) ([#67](https://github.com/nexus-xyz/nexus-exchange-cli/issues/67))

### Features

* **cli:** delete the phantom code-only ops, seal the allowlist (ENG-12369) ([#67](https://github.com/nexus-xyz/nexus-exchange-cli/issues/67)) ([13930f1](https://github.com/nexus-xyz/nexus-exchange-cli/commit/13930f1919d37170f51fe78adfda487e43b83a33))


### Bug Fixes

* **drift:** count operations, not path-ops, in the coverage ratio (ENG-10035) ([#62](https://github.com/nexus-xyz/nexus-exchange-cli/issues/62)) ([7e56089](https://github.com/nexus-xyz/nexus-exchange-cli/commit/7e560894fd4466309c2fd42c5587ed3b50c65fe0))
* **drift:** fail on an SDK call METHOD_OP has no row for (ENG-12786) ([#70](https://github.com/nexus-xyz/nexus-exchange-cli/issues/70)) ([1994fa7](https://github.com/nexus-xyz/nexus-exchange-cli/commit/1994fa7029f85d61b4f27550704d1ed4e12a70ea))
* **release:** stop the once-per-release phantom release PR (ENG-3921) ([#64](https://github.com/nexus-xyz/nexus-exchange-cli/issues/64)) ([0a8b181](https://github.com/nexus-xyz/nexus-exchange-cli/commit/0a8b1817b0f614b8e13a2a2e208f45e23132cb31))

## [0.4.0](https://github.com/nexus-xyz/nexus-exchange-cli/compare/v0.3.0...v0.4.0) (2026-08-18)


### ⚠ BREAKING CHANGES

* **networks:** `--network` accepts a custom label, so it is no longer a clap `ValueEnum` and `--help` no longer enumerates its values. `--base-url` now reports undeclared funds rather than inheriting the named network's classification, so `account credit` is refused under an override; declare the stage under `custom_networks` to restore it. The config file's `mainnet_acknowledged` bool is replaced by the per-network `acknowledged_networks` set — an existing flag is migrated to `mainnet` on load, and covers only mainnet. `agents register --chain-id` defaults from the selected target's declared signing domain instead of always 393, and a custom network that declares no signing domain is refused rather than defaulted — pass `--chain-id` or declare `chain_id` on its entry.
* **cli:** a stored key is scoped to one network. A config written before this change keeps working — its credentials are read as belonging to the network that file names, and are rewritten into the namespaced layout on the next write — but it no longer authenticates a *different* network. Someone relying on one key across `--network` values must now configure each network with `nexus setup`. That is the intended correction: such a key was already invalid server-side, and the old behavior meant a testnet key was offered to a real-funds host.
* **cli:** withdraw `account margin-mode` — no endpoint backs it (ENG-7740) ([#45](https://github.com/nexus-xyz/nexus-exchange-cli/issues/45))
* **cli:** `--network stable` and `--network beta` are rejected. They named release channels, and the replacement for `stable` is `testnet`, NOT `mainnet` — reading it the intuitive way points a real-funds label at a faucet host, which is the precise error the axis exists to prevent. Rejected at parse time rather than aliased: for a CLI, an error is the only way to force the re-decide the SDK gets from the compiler, and a silent remap is what must not happen on a real-funds axis.
* **cli:** verify spec+SDK parity and autobump the SDK dependency (ENG-7962) ([#51](https://github.com/nexus-xyz/nexus-exchange-cli/issues/51))
* **cli:** `nexus fills --limit` is now the server-side page size rather than a client-side truncation of a fixed 100-fill page — a value above 100 now actually returns more fills, and clap rejects anything outside the API's 1..=1000 (previously any u32 was accepted and silently truncated). Forced by nexus-exchange 0.7.0, where `fetch_my_trades` takes the limit.
* **cli:** `nexus order get`, `nexus order cancel <ORDER_ID>`, and `nexus order amend` now require `--market <MARKET>`, because the exchange routes single-order-by-id requests per market. `nexus order cancel --all` is unchanged.

### Features

* **cli:** accept `--tif post-only` ([#47](https://github.com/nexus-xyz/nexus-exchange-cli/issues/47)) ([fb9e857](https://github.com/nexus-xyz/nexus-exchange-cli/commit/fb9e857f86e8f9340603c62a4c243e35194449bf))
* **cli:** adopt the {mainnet, testnet, local} network axis (ENG-6455) ([#52](https://github.com/nexus-xyz/nexus-exchange-cli/issues/52)) ([e910889](https://github.com/nexus-xyz/nexus-exchange-cli/commit/e910889c418d81edf7d31ffda9ce4ea8b0a61390))
* **cli:** bump spec to v0.7.1 via nexus-exchange 0.6.0; surface spec tag in `nexus --version` (ENG-6039) ([#43](https://github.com/nexus-xyz/nexus-exchange-cli/issues/43)) ([7b3a6ad](https://github.com/nexus-xyz/nexus-exchange-cli/commit/7b3a6ad7e71dcc3c924f8b65d6675b8de5300fa1))
* **cli:** flip runtime routing to /api/v1 via nexus-exchange 0.5.1 (ENG-5190) ([#37](https://github.com/nexus-xyz/nexus-exchange-cli/issues/37)) ([2c78e3b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2c78e3bf0498c0adb73c5ab8949d5cbbfb9b8dc8))
* **cli:** per-network credentials and real-funds guardrails (ENG-6462) ([#55](https://github.com/nexus-xyz/nexus-exchange-cli/issues/55)) ([49f4198](https://github.com/nexus-xyz/nexus-exchange-cli/commit/49f419836b600aa82369269fbc120d9a98aa76f3))
* **cli:** surface portfolio-parity data in `nexus account` / `positions` (ENG-6460) ([#48](https://github.com/nexus-xyz/nexus-exchange-cli/issues/48)) ([ab59a87](https://github.com/nexus-xyz/nexus-exchange-cli/commit/ab59a87bf0666b64438e9829b19b856c541f3015))
* **cli:** target the /api/v1 direct-indexer surface (ENG-4949) ([#34](https://github.com/nexus-xyz/nexus-exchange-cli/issues/34)) ([4dd59c6](https://github.com/nexus-xyz/nexus-exchange-cli/commit/4dd59c6899ebfc63048debc3293fd3ed0c043ad5))
* **cli:** verify spec+SDK parity and autobump the SDK dependency (ENG-7962) ([#51](https://github.com/nexus-xyz/nexus-exchange-cli/issues/51)) ([a27091f](https://github.com/nexus-xyz/nexus-exchange-cli/commit/a27091f3785ad5a26aff7f705fe1bf0125e1eb4a))
* close the 0.3.0-era command-surface gaps vs the nexus-exchange SDK (ENG-5487) ([#41](https://github.com/nexus-xyz/nexus-exchange-cli/issues/41)) ([a0cba3e](https://github.com/nexus-xyz/nexus-exchange-cli/commit/a0cba3e3bed933d49291405809b4c2364299186e))
* **networks:** add a caller-supplied custom network to the CLI (ENG-9827) ([3825d5e](https://github.com/nexus-xyz/nexus-exchange-cli/commit/3825d5e1580d2c254310d3633dbe5852c749bc5f))
* **networks:** deprecate the base-URL override for a declared stage (ENG-10956) ([#60](https://github.com/nexus-xyz/nexus-exchange-cli/issues/60)) ([de6a5fc](https://github.com/nexus-xyz/nexus-exchange-cli/commit/de6a5fcc68e6c25b293ae81d87d8c54583db96fa))


### Bug Fixes

* **cli:** atomic credential-file writes for safe auth persistence (ENG-3816) ([#40](https://github.com/nexus-xyz/nexus-exchange-cli/issues/40)) ([43a2455](https://github.com/nexus-xyz/nexus-exchange-cli/commit/43a24556e1da20a2d73b81a5b1b6d5221dfd4cf3))
* **cli:** make the spec-drift allowlist prove its claims (ENG-7927) ([#46](https://github.com/nexus-xyz/nexus-exchange-cli/issues/46)) ([29f0312](https://github.com/nexus-xyz/nexus-exchange-cli/commit/29f0312771112ab870d7ee083f62b2089f8c8942))
* **cli:** withdraw `account margin-mode` — no endpoint backs it (ENG-7740) ([#45](https://github.com/nexus-xyz/nexus-exchange-cli/issues/45)) ([f15021f](https://github.com/nexus-xyz/nexus-exchange-cli/commit/f15021f74ae137e502c5b6de2d743f85b51d9974))
* **release:** stop Authenticode-signing the Windows artifact (ENG-9357) ([#59](https://github.com/nexus-xyz/nexus-exchange-cli/issues/59)) ([9999829](https://github.com/nexus-xyz/nexus-exchange-cli/commit/9999829169fe05468350477e4594765e046c5693))

## [0.3.0](https://github.com/nexus-xyz/nexus-exchange-cli/compare/v0.2.0...v0.3.0) (2026-07-06)


### Features

* **cli:** add `account rate-limit` command ([#11](https://github.com/nexus-xyz/nexus-exchange-cli/issues/11)) ([e4eb725](https://github.com/nexus-xyz/nexus-exchange-cli/commit/e4eb7250576b356a86c99ac3fd62a4f81a2cf2b6))
* **cli:** add `nexus completions <shell>` subcommand (ENG-3554) ([8d16c78](https://github.com/nexus-xyz/nexus-exchange-cli/commit/8d16c78df92c317e93f5ff618ae2d133989266c9))
* **cli:** add authenticated `withdrawals` command ([#10](https://github.com/nexus-xyz/nexus-exchange-cli/issues/10)) ([ac5bdc4](https://github.com/nexus-xyz/nexus-exchange-cli/commit/ac5bdc4ff0b75f1914dd4a2da3b8f6fa345cc64c))
* **cli:** add global --output &lt;human|json&gt; flag (ENG-3552) ([971ac42](https://github.com/nexus-xyz/nexus-exchange-cli/commit/971ac42f0b7fb4759bfe5bfd531af32fdbd03d94))
* **cli:** add global --output &lt;human|json&gt; flag (ENG-3552) ([c1abcbd](https://github.com/nexus-xyz/nexus-exchange-cli/commit/c1abcbd064df910159fa7c5dc63496505d5c8c2b))
* **cli:** add nexus completions &lt;shell&gt; subcommand (ENG-3554) ([7654605](https://github.com/nexus-xyz/nexus-exchange-cli/commit/765460540929fc6846477b0b5be32e3e45f72f64))
* **cli:** add read-only `market` subcommands (summary/status/mark-price) ([#9](https://github.com/nexus-xyz/nexus-exchange-cli/issues/9)) ([2e6ff0f](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2e6ff0f3928488db3c20a0812b5b34cffc3d35d6))
* **cli:** implement full command surface over the SDK (ENG-3449) ([#5](https://github.com/nexus-xyz/nexus-exchange-cli/issues/5)) ([d6d6537](https://github.com/nexus-xyz/nexus-exchange-cli/commit/d6d65373891aec585d80501bc6bd59152a3c0a2c))
* **cli:** measurable API coverage (.api-version + endpoints.txt + drift) + examples/tests (ENG-4108) ([#18](https://github.com/nexus-xyz/nexus-exchange-cli/issues/18)) ([ee40ecc](https://github.com/nexus-xyz/nexus-exchange-cli/commit/ee40ecc1fb1fe7f9a81e0a0e5226f67f6aae06af))
* **cli:** send descriptive User-Agent for traffic attribution (ENG-3446) ([#7](https://github.com/nexus-xyz/nexus-exchange-cli/issues/7)) ([8d60fd3](https://github.com/nexus-xyz/nexus-exchange-cli/commit/8d60fd352343f288252045b26eee252a7ad2e2d5))
* **cli:** wallet-signed auth — auth login + agents register (ENG-4046) ([#17](https://github.com/nexus-xyz/nexus-exchange-cli/issues/17)) ([f693eaa](https://github.com/nexus-xyz/nexus-exchange-cli/commit/f693eaabd63624e30728d59bf10740e34a477992))
* **installer:** add /compute route for the legacy compute CLI (ENG-3937) ([#27](https://github.com/nexus-xyz/nexus-exchange-cli/issues/27)) ([224acca](https://github.com/nexus-xyz/nexus-exchange-cli/commit/224acca5f656c185766581b73baac29b9b189d7b))
* **install:** serve cargo-dist installer at cli.nexus.xyz (ENG-3454) ([#6](https://github.com/nexus-xyz/nexus-exchange-cli/issues/6)) ([3f0082b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/3f0082b5746011d0a4c54f6ed952652006442fe1))
* **release:** minisign + msi + Windows signing on top of dist pipeline (ENG-3432) ([#4](https://github.com/nexus-xyz/nexus-exchange-cli/issues/4)) ([2eb167b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2eb167b9d3bb3c8b968ae4973f205a4dfea14bbc))
* wrap remaining spec endpoints as commands (ENG-3885) ([#15](https://github.com/nexus-xyz/nexus-exchange-cli/issues/15)) ([1b73ad9](https://github.com/nexus-xyz/nexus-exchange-cli/commit/1b73ad99d124c77d1be9741f7cd44587487d56f3))

## [0.2.0](https://github.com/nexus-xyz/nexus-exchange-cli/compare/v0.1.0...v0.2.0) (2026-07-02)


### Features

* **installer:** add /compute route for the legacy compute CLI (ENG-3937) ([#27](https://github.com/nexus-xyz/nexus-exchange-cli/issues/27)) ([224acca](https://github.com/nexus-xyz/nexus-exchange-cli/commit/224acca5f656c185766581b73baac29b9b189d7b))

## 0.1.0 (2026-06-26)


### Features

* **cli:** add `account rate-limit` command ([#11](https://github.com/nexus-xyz/nexus-exchange-cli/issues/11)) ([e4eb725](https://github.com/nexus-xyz/nexus-exchange-cli/commit/e4eb7250576b356a86c99ac3fd62a4f81a2cf2b6))
* **cli:** add `nexus completions <shell>` subcommand (ENG-3554) ([8d16c78](https://github.com/nexus-xyz/nexus-exchange-cli/commit/8d16c78df92c317e93f5ff618ae2d133989266c9))
* **cli:** add authenticated `withdrawals` command ([#10](https://github.com/nexus-xyz/nexus-exchange-cli/issues/10)) ([ac5bdc4](https://github.com/nexus-xyz/nexus-exchange-cli/commit/ac5bdc4ff0b75f1914dd4a2da3b8f6fa345cc64c))
* **cli:** add global --output &lt;human|json&gt; flag (ENG-3552) ([971ac42](https://github.com/nexus-xyz/nexus-exchange-cli/commit/971ac42f0b7fb4759bfe5bfd531af32fdbd03d94))
* **cli:** add global --output &lt;human|json&gt; flag (ENG-3552) ([c1abcbd](https://github.com/nexus-xyz/nexus-exchange-cli/commit/c1abcbd064df910159fa7c5dc63496505d5c8c2b))
* **cli:** add nexus completions &lt;shell&gt; subcommand (ENG-3554) ([7654605](https://github.com/nexus-xyz/nexus-exchange-cli/commit/765460540929fc6846477b0b5be32e3e45f72f64))
* **cli:** add read-only `market` subcommands (summary/status/mark-price) ([#9](https://github.com/nexus-xyz/nexus-exchange-cli/issues/9)) ([2e6ff0f](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2e6ff0f3928488db3c20a0812b5b34cffc3d35d6))
* **cli:** implement full command surface over the SDK (ENG-3449) ([#5](https://github.com/nexus-xyz/nexus-exchange-cli/issues/5)) ([d6d6537](https://github.com/nexus-xyz/nexus-exchange-cli/commit/d6d65373891aec585d80501bc6bd59152a3c0a2c))
* **cli:** measurable API coverage (.api-version + endpoints.txt + drift) + examples/tests (ENG-4108) ([#18](https://github.com/nexus-xyz/nexus-exchange-cli/issues/18)) ([ee40ecc](https://github.com/nexus-xyz/nexus-exchange-cli/commit/ee40ecc1fb1fe7f9a81e0a0e5226f67f6aae06af))
* **cli:** send descriptive User-Agent for traffic attribution (ENG-3446) ([#7](https://github.com/nexus-xyz/nexus-exchange-cli/issues/7)) ([8d60fd3](https://github.com/nexus-xyz/nexus-exchange-cli/commit/8d60fd352343f288252045b26eee252a7ad2e2d5))
* **cli:** wallet-signed auth — auth login + agents register (ENG-4046) ([#17](https://github.com/nexus-xyz/nexus-exchange-cli/issues/17)) ([f693eaa](https://github.com/nexus-xyz/nexus-exchange-cli/commit/f693eaabd63624e30728d59bf10740e34a477992))
* **install:** serve cargo-dist installer at cli.nexus.xyz (ENG-3454) ([#6](https://github.com/nexus-xyz/nexus-exchange-cli/issues/6)) ([3f0082b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/3f0082b5746011d0a4c54f6ed952652006442fe1))
* **release:** minisign + msi + Windows signing on top of dist pipeline (ENG-3432) ([#4](https://github.com/nexus-xyz/nexus-exchange-cli/issues/4)) ([2eb167b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2eb167b9d3bb3c8b968ae4973f205a4dfea14bbc))
* wrap remaining spec endpoints as commands (ENG-3885) ([#15](https://github.com/nexus-xyz/nexus-exchange-cli/issues/15)) ([1b73ad9](https://github.com/nexus-xyz/nexus-exchange-cli/commit/1b73ad99d124c77d1be9741f7cd44587487d56f3))
