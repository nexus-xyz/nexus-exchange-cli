# Changelog

## [1.0.0](https://github.com/nexus-xyz/nexus-exchange-cli/compare/v0.3.0...v1.0.0) (2026-07-21)


### ⚠ BREAKING CHANGES

* **cli:** `nexus order get`, `nexus order cancel <ORDER_ID>`, and `nexus order amend` now require `--market <MARKET>`, because the exchange routes single-order-by-id requests per market. `nexus order cancel --all` is unchanged.

### Features

* **cli:** bump spec to v0.7.1 via nexus-exchange 0.6.0; surface spec tag in `nexus --version` (ENG-6039) ([#43](https://github.com/nexus-xyz/nexus-exchange-cli/issues/43)) ([7b3a6ad](https://github.com/nexus-xyz/nexus-exchange-cli/commit/7b3a6ad7e71dcc3c924f8b65d6675b8de5300fa1))
* **cli:** flip runtime routing to /api/v1 via nexus-exchange 0.5.1 (ENG-5190) ([#37](https://github.com/nexus-xyz/nexus-exchange-cli/issues/37)) ([2c78e3b](https://github.com/nexus-xyz/nexus-exchange-cli/commit/2c78e3bf0498c0adb73c5ab8949d5cbbfb9b8dc8))
* **cli:** target the /api/v1 direct-indexer surface (ENG-4949) ([#34](https://github.com/nexus-xyz/nexus-exchange-cli/issues/34)) ([4dd59c6](https://github.com/nexus-xyz/nexus-exchange-cli/commit/4dd59c6899ebfc63048debc3293fd3ed0c043ad5))


### Bug Fixes

* **cli:** atomic credential-file writes for safe auth persistence (ENG-3816) ([#40](https://github.com/nexus-xyz/nexus-exchange-cli/issues/40)) ([43a2455](https://github.com/nexus-xyz/nexus-exchange-cli/commit/43a24556e1da20a2d73b81a5b1b6d5221dfd4cf3))

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
