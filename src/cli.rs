//! Command-line argument parsing and config/credential resolution.

use clap::{Parser, Subcommand, ValueEnum};
use nexus_exchange::rest::{MAX_FILLS_LIMIT, MAX_PORTFOLIO_HISTORY_LIMIT};
use nexus_exchange::types::{OrderType, PortfolioWindow, Side, TimeInForce};
use nexus_exchange::{Config, Network};

use crate::credentials::FileConfig;

// Re-export for use in main.rs.
pub use clap_complete::Shell;

/// `User-Agent` the CLI reports to the API so the indexer can attribute traffic
/// to the CLI specifically (vs. the Rust SDK, the web frontend, or raw callers).
///
/// The version is the crate version baked in at compile time, so it can never
/// drift from `Cargo.toml`. The string is a fixed constant with no user- or
/// environment-supplied input, so it carries no HTTP-header-injection risk; the
/// SDK additionally falls back to its own default UA if a value ever contained
/// bytes illegal in a header.
const USER_AGENT: &str = concat!("nexus-cli/", env!("CARGO_PKG_VERSION"));

/// Version string for `nexus --version` / `-V`: the CLI crate version, the spec
/// tag the CLI is compiled against, and the resolved `nexus-exchange` SDK
/// version it links. The CLI carries no transport of its own — it speaks the
/// API through the SDK — so surfacing all three makes it clear which spec the
/// CLI targets (the same tag the SDK sends as `X-Nexus-Api-Version`) and which
/// SDK build backs it. The spec tag and SDK version are injected at build time
/// by `build.rs` (from `.api-version` and `Cargo.lock`), so they can't drift.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (spec ",
    env!("NEXUS_SPEC_TAG"),
    ", nexus-exchange ",
    env!("NEXUS_SDK_VERSION"),
    ")"
);

/// Command-line interface for the Nexus Exchange API.
#[derive(Debug, Parser)]
#[command(name = "nexus", version = LONG_VERSION, about, long_about = None)]
pub struct Cli {
    /// Which network to target (default: testnet, or the `nexus setup` value).
    ///
    /// `stable` and `beta` named release channels, not networks, and were
    /// retired in `nexus-exchange` 0.8.0. `stable` pointed at a **play-funds**
    /// host, so its replacement is `testnet` — *not* `mainnet`, which is real
    /// funds and is not reachable in this release.
    #[arg(long, value_enum, global = true, env = "NEXUS_NETWORK")]
    pub network: Option<NetworkArg>,

    /// Override the API base URL (takes precedence over `--network`).
    #[arg(long, global = true, env = "NEXUS_BASE_URL")]
    pub base_url: Option<String>,

    /// Output format: human-readable tables or pretty JSON.
    #[arg(long, value_enum, global = true, default_value_t = OutputFormat::Human, env = "NEXUS_OUTPUT")]
    pub output: OutputFormat,

    #[command(flatten)]
    pub credentials: Credentials,

    #[command(subcommand)]
    pub command: Command,
}

/// API credentials. Read from flags, the corresponding environment variables,
/// or the config file written by `nexus setup` (in that order of precedence).
/// Authenticated commands sign requests when both halves are present.
///
/// `Debug` is implemented by hand so the secret never lands in logs.
#[derive(clap::Args)]
pub struct Credentials {
    /// API key id (e.g. `nx_...`).
    #[arg(long, global = true, env = "NEXUS_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// API secret. Prefer the env var or `nexus setup` over the flag, since
    /// flags are visible in your shell history and process list.
    #[arg(long, global = true, env = "NEXUS_API_SECRET", hide_env_values = true)]
    pub api_secret: Option<String>,

    /// Wallet session token from `nexus auth login`. Authenticates
    /// session-scoped routes when no HMAC key/secret pair is configured. Prefer
    /// the env var or the stored login over the flag (flags are visible in your
    /// shell history and process list).
    #[arg(
        long,
        global = true,
        env = "NEXUS_SESSION_TOKEN",
        hide_env_values = true
    )]
    pub session_token: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("api_key", &self.api_key)
            .field(
                "api_secret",
                &self.api_secret.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Which Nexus Exchange network to target.
///
/// This is the **network** axis the spec formalizes (ENG-6442), not a release
/// channel. The distinction is the whole point: the retired `stable`/`beta`
/// values named deployment channels, which is how a play-funds host came to be
/// labelled "production" and how the SDK's real-funds guard ended up pointed at
/// the wrong network (ENG-6452).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum NetworkArg {
    /// **Real funds.** Not reachable in this release: the SDK refuses every
    /// request locally rather than guess a host or sign an unverifiable path.
    Mainnet,
    /// **Play funds** credited by the faucet — the default, and the safe target.
    Testnet,
    /// A locally run indexer. Play funds, and never a fallback.
    Local,
}

/// Retired release-channel names, mapped to the network each actually pointed
/// at. `--network` rejects these outright (clap only accepts the variants
/// above); this table exists solely so a **config file** written before the
/// rename gets a real diagnostic instead of a silent fallback.
///
/// The mapping is the part worth stating: `stable` was a *play-funds* host, so
/// it becomes `testnet`. Reading it as `mainnet` — the intuitive guess — has it
/// exactly backwards and is the mislabel this axis exists to correct.
const RETIRED_NETWORKS: &[(&str, &str)] = &[("stable", "testnet"), ("beta", "testnet")];

/// The CLI's name for an SDK network, for diagnostics.
///
/// The wildcard arm is not defensive padding: [`Network`] is `#[non_exhaustive]`,
/// so a downstream crate *cannot* match it exhaustively, and a variant added
/// upstream would otherwise have to be misreported as one of the three names the
/// CLI knows. Falling back to the SDK's own `Debug` says something true about a
/// network this build has no vocabulary for.
fn network_name(n: Network) -> String {
    match n {
        Network::Mainnet => "mainnet".to_string(),
        Network::Testnet => "testnet".to_string(),
        Network::Local => "local".to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

/// The diagnostic for a config-file `network` this build cannot parse, or `None`
/// when there is nothing to report. Split out of [`Cli::config`] so the text —
/// the part a user actually reads — is testable without capturing stderr.
///
/// `landing` is where the invocation really ends up, read from the *resolved*
/// default config rather than restated from [`RETIRED_NETWORKS`]. Those are two
/// independent facts that merely coincide today: `stable` named a play-funds host
/// no matter what the SDK default later becomes. Deriving both from the table
/// would let a future default change silently turn this warning into a false
/// statement.
///
/// A blank value reports nothing. `nexus setup` normalizes an empty answer to
/// "no preference", and a hand-edited `"network": ""` means the same thing — it
/// selects nothing, so there is nothing stale to fix.
fn stale_network_warning(name: &str, landing: Option<&str>) -> Option<String> {
    if name.trim().is_empty() {
        return None;
    }
    // Every interpolation of `name` uses `{name:?}`, never `{name}`: the value
    // comes from a file and is echoed to a terminal, and `Debug` escapes control
    // bytes, so a config cannot smuggle ESC sequences into this line.
    let landing = match landing {
        Some(network) => format!("the default network ({network})"),
        None => "the default network".to_string(),
    };
    Some(match NetworkArg::retired_replacement(name) {
        Some(replacement) => format!(
            "warning: config-file network {name:?} was a release channel, not a network, and no \
             longer exists; it named a play-funds host, which is now `{replacement}`. Using \
             {landing}. Run `nexus setup`, or set \"network\": \"{replacement}\" in the config."
        ),
        None => format!(
            "warning: config-file network {name:?} is not a known network (valid: mainnet, \
             testnet, local); using {landing}. Run `nexus setup`, or fix \"network\" in the \
             config."
        ),
    })
}

/// The network an invocation targets when nothing selects one.
///
/// The SDK's own [`Config::default`] is the authority; this is the CLI's
/// transcription of it, needed because credential namespacing has to name a
/// section (a string key) before any [`Config`] exists, and because
/// [`Network`] is `#[non_exhaustive]` so the SDK default cannot be mapped back
/// to a [`NetworkArg`] infallibly. `the_default_network_matches_the_sdk` is what
/// notices if the two ever disagree — pinning an upstream fact without a check
/// is the failure mode ENG-6452 was.
pub(crate) const DEFAULT_NETWORK: NetworkArg = NetworkArg::Testnet;

impl NetworkArg {
    /// The canonical lowercase name, used as the config file's section key and
    /// in diagnostics. Exhaustive, so a new network must choose its own name
    /// rather than inherit a wrong one.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Local => "local",
        }
    }

    /// Whether this network moves real money — the one bit every guardrail keys
    /// off. Asserted against the SDK's own `is_mainnet` by
    /// `real_funds_matches_the_sdk`, rather than assumed to stay in sync.
    pub(crate) fn is_real_funds(self) -> bool {
        matches!(self, Self::Mainnet)
    }

    /// Parse a network name from the config file. Returns `None` for unknown
    /// values so a stale config can't crash the CLI.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mainnet" => Some(Self::Mainnet),
            "testnet" => Some(Self::Testnet),
            "local" => Some(Self::Local),
            _ => None,
        }
    }

    /// The current name for a retired release-channel value, if `s` is one.
    /// Used only to turn a stale config into an actionable warning.
    pub(crate) fn retired_replacement(s: &str) -> Option<&'static str> {
        let name = s.trim().to_ascii_lowercase();
        RETIRED_NETWORKS
            .iter()
            .find(|(retired, _)| *retired == name)
            .map(|(_, replacement)| *replacement)
    }
}

/// How command results are rendered to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable, aligned tables (the default).
    Human,
    /// Pretty-printed JSON.
    Json,
}

/// Order side. Maps onto the SDK's [`Side`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideArg {
    Buy,
    Sell,
}

impl From<SideArg> for Side {
    fn from(s: SideArg) -> Self {
        match s {
            SideArg::Buy => Side::Buy,
            SideArg::Sell => Side::Sell,
        }
    }
}

/// Order type. Maps onto the SDK's [`OrderType`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderTypeArg {
    Limit,
    Market,
}

impl From<OrderTypeArg> for OrderType {
    fn from(t: OrderTypeArg) -> Self {
        match t {
            OrderTypeArg::Limit => OrderType::Limit,
            OrderTypeArg::Market => OrderType::Market,
        }
    }
}

/// Time in force. Maps onto the SDK's [`TimeInForce`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TifArg {
    /// Good-til-cancelled.
    Gtc,
    /// Immediate-or-cancel.
    Ioc,
    /// Fill-or-kill.
    Fok,
    /// Post-only: rest or be rejected, never cross the spread.
    ///
    /// Spelled `post-only` on the command line (clap kebab-cases the variant).
    /// Post-only is a time-in-force on this API rather than a boolean flag, so
    /// it belongs here rather than as `--post-only` — which is what made it
    /// look absent: the capability is in the spec and the engine, and only the
    /// CLI could not reach it.
    PostOnly,
}

impl From<TifArg> for TimeInForce {
    fn from(t: TifArg) -> Self {
        match t {
            TifArg::Gtc => TimeInForce::Gtc,
            TifArg::Ioc => TimeInForce::Ioc,
            TifArg::Fok => TimeInForce::Fok,
            TifArg::PostOnly => TimeInForce::PostOnly,
        }
    }
}

/// The `nexus-exchange` release whose `Network` axis the mapping below was read
/// against.
///
/// This constant is the signal, because the `match` cannot be. `Network` is
/// `#[non_exhaustive]`, so a variant added upstream compiles straight through a
/// mapping that only matches on the CLI's own enum — and `#[non_exhaustive]` also
/// forbids a downstream crate from matching `Network` exhaustively, so there is
/// no compile-time guard to be had. Pinning the version instead means an SDK bump
/// goes red until someone re-reads the axis.
///
/// Transcribing an upstream fact with nothing to notice when upstream moves is
/// what pointed the real-funds guard at the wrong network to begin with
/// (ENG-6452), so the transcription is pinned here rather than trusted.
///
/// Checked by `the_network_axis_was_checked_against_the_pinned_sdk`, hence
/// `#[cfg(test)]`: it is a claim about the mapping below, not a value the binary
/// has any use for at runtime.
#[cfg(test)]
const NETWORK_AXIS_VERIFIED_AGAINST: &str = "0.8.0";

impl From<NetworkArg> for Network {
    fn from(n: NetworkArg) -> Self {
        // Exhaustive over `NetworkArg` — the CLI's own enum — so adding a value
        // there without mapping it is a compile error. It is deliberately NOT a
        // claim about the SDK's axis growing: see
        // `NETWORK_AXIS_VERIFIED_AGAINST`, which is what notices that.
        match n {
            NetworkArg::Mainnet => Network::Mainnet,
            NetworkArg::Testnet => Network::Testnet,
            NetworkArg::Local => Network::Local,
        }
    }
}

impl Cli {
    /// Resolve the SDK [`Config`], layering: `--base-url` > `--network`/env >
    /// config-file `base_url` > config-file `network` > the SDK default
    /// (testnet — play funds; the default must never be a real-funds network).
    /// Every resolved config carries the CLI's [`USER_AGENT`].
    pub fn config(&self, file: &FileConfig) -> Config {
        let config = if let Some(url) = &self.base_url {
            Config::with_base_url(url.clone())
        } else if let Some(net) = self.network {
            Config::new(net.into())
        } else if let Some(url) = &file.base_url {
            Config::with_base_url(url.clone())
        } else if let Some(net) = file.network.as_deref().and_then(NetworkArg::parse) {
            Config::new(net.into())
        } else {
            // Falling through with a *set* config-file network means it did not
            // parse, whatever the reason: a retired release-channel name, or a
            // typo like "mainet". Both get a diagnostic — a silent fallback on
            // the network axis is the failure class this change exists to remove,
            // and the typo case is the durable one (`stable`/`beta` age out of
            // configs; misspellings never will).
            //
            // Still a fallback rather than a hard error: the default is a
            // play-funds network, so the request is safe to make — it is the
            // stale name that needs fixing, not this invocation.
            let config = Config::default();
            if let Some(name) = file.network.as_deref() {
                let landing = config.network().map(network_name);
                if let Some(warning) = stale_network_warning(name, landing.as_deref()) {
                    eprintln!("{warning}");
                }
            }
            config
        };
        config.with_user_agent(USER_AGENT)
    }

    /// Which network's stored credentials this invocation uses:
    /// `--network`/`NEXUS_NETWORK` > config-file `network` > [`DEFAULT_NETWORK`].
    ///
    /// Deliberately **not** affected by `--base-url`. A base-URL override
    /// changes where the request goes, not who you are — pointing at a proxy,
    /// a tunnel or a staging host in front of a network should still present
    /// that network's key, and an override cannot be mapped back to a network
    /// name anyway. So the namespace is always well-defined, and `--base-url`
    /// callers keep the credentials they had before namespacing existed.
    ///
    /// An unparseable config-file network resolves to the default, matching
    /// where [`Cli::config`] sends the traffic; that path is also what prints
    /// the diagnostic, so this stays silent rather than warning twice.
    pub fn credential_network(&self, file: &FileConfig) -> NetworkArg {
        self.network
            .or_else(|| file.network.as_deref().and_then(NetworkArg::parse))
            .unwrap_or(DEFAULT_NETWORK)
    }

    /// Resolve an API key/secret pair, layering flags/env over the config file's
    /// section for the resolved network. Returns `None` when no usable pair is
    /// configured. Warns (and still returns `None`) when only one half is
    /// present, since that is almost always a mistake.
    ///
    /// Flags and env are **not** namespaced: they are a per-invocation override
    /// the user has just typed for the network they just selected, so scoping
    /// them would mean inventing `NEXUS_TESTNET_API_KEY`-style variables for no
    /// gain. Only the persisted layer — the one that outlives the invocation and
    /// can hold several networks at once — is sectioned.
    ///
    /// The pair is handed to [`Config::api_key`] so the SDK signs authenticated
    /// requests; the CLI never touches the secret beyond passing it through.
    pub fn credentials(&self, file: &FileConfig) -> Option<(String, String)> {
        let stored = file.credentials_for(self.credential_network(file));
        let key = self
            .credentials
            .api_key
            .clone()
            .or_else(|| stored.and_then(|c| c.api_key.clone()));
        let secret = self
            .credentials
            .api_secret
            .clone()
            .or_else(|| stored.and_then(|c| c.api_secret.clone()));

        match (key, secret) {
            (Some(k), Some(s)) => Some((k, s)),
            (Some(_), None) => {
                eprintln!(
                    "warning: API key set without a matching API secret; requests will be unsigned"
                );
                None
            }
            (None, Some(_)) => {
                eprintln!(
                    "warning: API secret set without a matching API key; requests will be unsigned"
                );
                None
            }
            (None, None) => None,
        }
    }

    /// Resolve a wallet session token, layering flag/env over the config file's
    /// section for the resolved network (the same precedence, and the same
    /// namespacing, as the HMAC pair). Returns `None` when none is configured.
    /// Handed to [`Config::session_token`] only when no HMAC key pair is
    /// present, so the HMAC pair takes precedence as the request signer.
    pub fn session_token(&self, file: &FileConfig) -> Option<String> {
        self.credentials.session_token.clone().or_else(|| {
            file.credentials_for(self.credential_network(file))
                .and_then(|c| c.session_token.clone())
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List all tradable markets and their trading rules.
    Markets,

    /// Per-market data: summaries, lifecycle status, mark price.
    Market {
        #[command(subcommand)]
        action: MarketCommand,
    },

    /// Fetch the ticker for a single market, e.g. `BTC-USDX-PERP`.
    Ticker {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// List tickers for every market.
    Tickers,

    /// List per-market 24h summaries (mark price, volume, status).
    Summaries,

    /// Show the current mark price for a market.
    MarkPrice {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the lifecycle/halt status for a market.
    MarketStatus {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the funding-rate history for a market.
    FundingRates {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Maximum number of samples to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Show the order book (bids/asks) for a market.
    Orderbook {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show recent trades for a market.
    Trades {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Maximum number of trades to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Show OHLCV candles for a market.
    Candles {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Candle interval.
        #[arg(long, default_value = "1m")]
        timeframe: String,
        /// Maximum number of candles to return.
        #[arg(long, default_value_t = 200)]
        limit: u32,
    },

    /// Show the aggregate service health snapshot (`GET /status`).
    Health,

    /// Show your account summary (balance, collateral, equity, margin).
    Balance,

    /// List your open positions.
    Positions,

    /// List your recent fills (executions).
    Fills {
        /// Maximum number of fills to return (the server's page size, 1..=1000).
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=MAX_FILLS_LIMIT as i64))]
        limit: u32,
    },

    /// List your withdrawal history.
    Withdrawals {
        /// Maximum number of withdrawals to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// List your open orders.
    Orders,

    /// Your funding payments (perp funding booked against the account).
    FundingPayments {
        /// Maximum number of payments to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },

    /// Place, amend, cancel, or fetch orders.
    Order {
        #[command(subcommand)]
        action: OrderCommand,
    },

    /// Manage account settings (deposit, credit, leverage, rate-limit).
    Account {
        #[command(subcommand)]
        action: AccountCommand,
    },

    /// Wallet-signed authentication (EIP-191 sign-in for a session token).
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },

    /// Manage HMAC API keys (list/create/delete).
    Keys {
        #[command(subcommand)]
        action: KeysCommand,
    },

    /// Manage registered agent keys (list/revoke).
    Agents {
        #[command(subcommand)]
        action: AgentsCommand,
    },

    /// Manage collateral transfers (list/create).
    Transfers {
        #[command(subcommand)]
        action: TransfersCommand,
    },

    /// Manage sub-accounts (list/create).
    SubAccounts {
        #[command(subcommand)]
        action: SubAccountsCommand,
    },

    /// Stream live data over WebSocket. Public channels (`trades`, `book`,
    /// `candles`) need `--market`; account channels (`orders`, `fills`,
    /// `positions`, `balances`) are scoped to your key.
    Ws {
        /// One or more channels to subscribe to.
        #[arg(required = true, num_args = 1..)]
        channels: Vec<String>,
        /// Market for public channels, e.g. `BTC-USDX-PERP`.
        #[arg(long)]
        market: Option<String>,
        /// Resume from this sequence number (per channel).
        #[arg(long)]
        since: Option<i64>,
    },

    /// Interactively configure network and credentials.
    Setup,

    /// Print shell-completion script to stdout.
    Completions {
        /// Target shell.
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum MarketCommand {
    /// Per-market summaries with 24h volume and halt state.
    Summary,

    /// Show the lifecycle/halt status for a single market.
    Status {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the current mark price for a single market.
    MarkPrice {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum OrderCommand {
    /// Submit a new order.
    Place {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        #[arg(long)]
        market: String,
        /// Order side.
        #[arg(long, value_enum)]
        side: SideArg,
        /// Order type.
        #[arg(long = "type", value_enum)]
        order_type: OrderTypeArg,
        /// Limit price (required for `--type limit`).
        #[arg(long)]
        price: Option<String>,
        /// Order quantity (base units).
        #[arg(long)]
        quantity: String,
        /// Time in force.
        #[arg(long, value_enum, default_value_t = TifArg::Gtc)]
        tif: TifArg,
        /// Only reduce an existing position; never open or flip one.
        #[arg(long)]
        reduce_only: bool,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Cancel a single order by id (requires `--market`), or all open orders
    /// with `--all`.
    Cancel {
        /// Order id to cancel. Requires `--market` (by-id cancels are routed
        /// per market).
        #[arg(requires = "market")]
        order_id: Option<String>,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required when
        /// cancelling a single order: the engine routes by-id cancels per
        /// market. Not used with `--all`.
        #[arg(long, conflicts_with = "all")]
        market: Option<String>,
        /// Cancel all open orders.
        #[arg(long, conflicts_with = "order_id")]
        all: bool,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Fetch a single order by id.
    Get {
        /// Order id.
        order_id: String,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required: the engine
        /// routes by-id lookups per market.
        #[arg(long)]
        market: String,
    },

    /// Amend an open order in place (atomic cancel-replace). Set only the
    /// fields you want to change.
    Amend {
        /// Order id to amend.
        order_id: String,
        /// Market the order is on, e.g. `BTC-USDX-PERP`. Required: the engine
        /// routes by-id amends per market.
        #[arg(long)]
        market: String,
        /// New limit price.
        #[arg(long)]
        price: Option<String>,
        /// New order quantity (base units).
        #[arg(long)]
        quantity: Option<String>,
        /// New time in force.
        #[arg(long, value_enum)]
        tif: Option<TifArg>,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Submit a batch of orders from a JSON file (an array of order objects),
    /// or `-` to read the array from stdin.
    Batch {
        /// Path to a JSON file containing an array of order requests, or `-`
        /// for stdin.
        file: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Portfolio summary: equity, PnL, 24h volume, open counts, margin, and the
    /// withdrawable balance.
    ///
    /// The server reports every field optionally; an absent one renders `-`
    /// (JSON `null`) and never `0` — "not reported" is not "zero".
    Summary,

    /// Consolidated account snapshot: the portfolio summary *and* every open
    /// position, from one coherent server-side read.
    ///
    /// Prefer this over running `account summary` and `positions` separately:
    /// those are two independent requests, so a fill landing between them
    /// returns an aggregate that disagrees with the position list.
    State,

    /// Effective fee schedule for the account (maker/taker bps, tier, 30d
    /// volume). A negative maker fee is a rebate paid to you.
    Fees,

    /// Portfolio time series: equity, cumulative PnL, and cumulative traded
    /// volume, oldest first.
    PortfolioHistory {
        /// Window to report over. Also fixes the server-side sample cadence and
        /// point capacity (day 5m/288, week 1h/168, month 6h/120, all 1d/366).
        /// Omit for the server's `day` default.
        #[arg(long, value_enum)]
        window: Option<PortfolioWindowArg>,
        /// Maximum number of points to return. The API schema allows 1..=366
        /// (the widest window's capacity); the server additionally clamps to the
        /// selected window's own capacity, so fewer points may come back.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=MAX_PORTFOLIO_HISTORY_LIMIT as i64))]
        limit: Option<u32>,
    },

    /// Deposit collateral into the account.
    Deposit {
        /// Amount to deposit (quote asset).
        amount: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Claim synthetic (testnet) USDX credit from the faucet. Omit `--amount`
    /// to claim the full remaining daily allowance.
    Credit {
        /// Amount to claim; defaults to the remaining daily allowance.
        #[arg(long)]
        amount: Option<String>,
    },

    /// Show the caller's rate-limit status.
    RateLimit,

    /// Set the leverage for a market.
    Leverage {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Leverage multiplier (e.g. 10 for 10x). Must be at least 1.
        leverage: u32,
    },
    // There is deliberately no `margin-mode` subcommand: no endpoint backs one.
    // `margin_mode` appears nowhere in the pinned spec, and the only isolated-
    // margin route (`POST /account/margin`) requires a position that is already
    // isolated. It was withdrawn in ENG-7740; ENG-7614 tracks the engine work
    // that has to land before it can come back. Don't re-add it against a guessed
    // request shape — add it when the spec defines one.
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Sign in with an EVM wallet (EIP-191) and store the session token.
    ///
    /// The raw private key is read from `--private-key`, the
    /// `NEXUS_PRIVATE_KEY` environment variable, or — when neither is set and
    /// stdin is a terminal — a hidden interactive prompt. It is used only to
    /// produce the sign-in signature and is never written to disk or echoed.
    Login {
        /// Raw EVM private key (`0x`-prefix optional). Prefer the env var or the
        /// hidden prompt over the flag, which is visible in your shell history
        /// and process list.
        #[arg(long, env = "NEXUS_PRIVATE_KEY", hide_env_values = true)]
        private_key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum KeysCommand {
    /// List the API keys on the authenticated session.
    List,
    /// Create a new API key. The secret is shown once — store it immediately.
    Create {
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
    /// Delete an API key by id.
    Delete {
        /// Key id to delete.
        key_id: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentsCommand {
    /// List registered agent keys for the authenticated wallet.
    List,
    /// Register an agent key, authorized by an EIP-712 signature from the
    /// owning wallet. The wallet's raw private key produces the signature and
    /// is never written to disk or echoed; the request itself is unauthenticated
    /// (the signature is the authorization), so no API key or session token is
    /// required.
    Register {
        /// Agent address to authorize (`0x`-prefixed, 20 bytes).
        #[arg(long)]
        agent: String,
        /// Owning wallet's raw EVM private key (`0x`-prefix optional). Prefer the
        /// env var or the hidden prompt over the flag, which is visible in your
        /// shell history and process list.
        #[arg(long, env = "NEXUS_PRIVATE_KEY", hide_env_values = true)]
        private_key: Option<String>,
        /// Authorization expiry, Unix milliseconds. The spec expects expiry in
        /// `[now+1d, now+90d]`; defaults to 30 days from now when omitted.
        #[arg(long)]
        expires_at: Option<u64>,
        /// Monotonic nonce; defaults to the current Unix-ms timestamp (a safe
        /// starting value, per the spec).
        #[arg(long)]
        nonce: Option<u64>,
        /// EIP-712 domain chain id (the exchange's chain id). Part of the signed
        /// payload, so it must match what the server verifies against.
        #[arg(long, default_value_t = 393)]
        chain_id: u64,
        /// Optional human-readable label for the agent.
        #[arg(long)]
        label: Option<String>,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
    /// Revoke a registered agent by address.
    Revoke {
        /// Agent address (0x-prefixed).
        address: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TransfersCommand {
    /// List collateral transfers.
    List,
    /// Create a transfer between accounts (e.g. to/from a sub-account).
    Create {
        /// Source account id to debit.
        #[arg(long)]
        from: String,
        /// Destination account id to credit.
        #[arg(long)]
        to: String,
        /// Amount of collateral to move; must be positive.
        #[arg(long)]
        amount: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SubAccountsCommand {
    /// List sub-accounts of the authenticated master account.
    List,
    /// Create a new sub-account with a label.
    Create {
        /// Human-readable label for the sub-account.
        label: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },
}

/// Window for the portfolio time series. Maps onto the SDK's
/// [`PortfolioWindow`], which is the closed set of values the API accepts — so
/// the `window` query parameter can only ever carry one of these four, never
/// caller-shaped text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PortfolioWindowArg {
    /// Trailing 24 hours, sampled every 5 minutes (the server's default).
    Day,
    /// Trailing 7 days, sampled hourly.
    Week,
    /// Trailing 30 days, sampled every 6 hours.
    Month,
    /// Full retained history (~1 year), sampled daily.
    All,
}

impl From<PortfolioWindowArg> for PortfolioWindow {
    fn from(w: PortfolioWindowArg) -> Self {
        match w {
            PortfolioWindowArg::Day => PortfolioWindow::Day,
            PortfolioWindowArg::Week => PortfolioWindow::Week,
            PortfolioWindowArg::Month => PortfolioWindow::Month,
            PortfolioWindowArg::All => PortfolioWindow::All,
        }
    }
}

// `MarginModeArg` and its `From<MarginModeArg> for MarginMode` impl stood
// here and are removed with the command they existed for (ENG-7740): the
// API has no margin-mode endpoint, so the arg had nothing to map onto.
// `PortfolioWindowArg` above is unrelated and stays.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::NetworkCredentials;
    use clap::CommandFactory;
    use nexus_exchange::Client;

    fn base_url(cli: &Cli) -> String {
        Client::new(cli.config(&FileConfig::default()))
            .base_url()
            .to_string()
    }

    /// Catches conflicting flags, bad arg specs, etc. at test time.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_testnet_network() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.network, None);
        assert_eq!(base_url(&cli), Network::Testnet.base_url());
    }

    /// The invariant behind the default, not just its current value: omitting
    /// `--network` must never reach a network that moves real money.
    #[test]
    fn the_default_network_is_not_real_funds() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        let network = cli
            .config(&FileConfig::default())
            .network()
            .expect("the default config targets a named network, not a bare base URL");
        assert!(
            !network.is_mainnet(),
            "the default network must be play funds, got {network:?}"
        );
    }

    #[test]
    fn base_url_overrides_network() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "testnet",
            "--base-url",
            "http://x:1",
            "health",
        ])
        .unwrap();
        assert_eq!(base_url(&cli), "http://x:1");
    }

    #[test]
    fn config_file_is_a_fallback_below_flags() {
        let file = FileConfig {
            network: Some("local".into()),
            ..Default::default()
        };
        // No flag → file network wins.
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(
            Client::new(cli.config(&file)).base_url(),
            Network::Local.base_url()
        );
        // Flag beats the file.
        let cli = Cli::try_parse_from(["nexus", "--network", "testnet", "markets"]).unwrap();
        assert_eq!(
            Client::new(cli.config(&file)).base_url(),
            Network::Testnet.base_url()
        );
    }

    /// ENG-6455. `stable`/`beta` were release channels, not networks; the axis is
    /// now the spec's. Rejected at parse time so no invocation can keep using a
    /// name whose meaning was wrong.
    #[test]
    fn retired_release_channel_names_are_rejected_by_the_flag() {
        for retired in ["stable", "beta"] {
            let parsed = Cli::try_parse_from(["nexus", "--network", retired, "markets"]);
            assert!(
                parsed.is_err(),
                "--network {retired} must not parse; it is not a network"
            );
            // clap names the values that *are* valid, which is the migration hint.
            let rendered = parsed.unwrap_err().to_string();
            for valid in ["mainnet", "testnet", "local"] {
                assert!(
                    rendered.contains(valid),
                    "the error for `{retired}` should list `{valid}`; got: {rendered}"
                );
            }
        }
    }

    /// A config file written before the rename must not silently change which
    /// network is used, and must not be fatal either: the default *is* the host
    /// `stable` named, so the invocation is unaffected and only the stale name
    /// needs fixing.
    #[test]
    fn retired_network_in_the_config_file_falls_back_to_the_default() {
        for retired in ["stable", "beta", "STABLE", "  beta  "] {
            let file = FileConfig {
                network: Some(retired.into()),
                ..Default::default()
            };
            let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
            assert_eq!(
                Client::new(cli.config(&file)).base_url(),
                Network::Testnet.base_url(),
                "a config naming {retired:?} should land on the default network"
            );
        }
    }

    /// What the table claims is what the retired names *were*: `stable`/`beta`
    /// pointed at a play-funds host. That stays true no matter what the SDK
    /// default later becomes, so it is asserted on its own terms — every
    /// replacement must be a real network, and must not be real funds.
    ///
    /// Deliberately *not* asserted against `Config::default()`. The two facts
    /// coincide today, and an earlier version of this test conflated them, which
    /// would have made a future default change look like a table error. Where a
    /// stale config actually lands is covered by
    /// `the_stale_network_warning_names_where_it_actually_lands` and
    /// `retired_network_in_the_config_file_falls_back_to_the_default`.
    #[test]
    fn every_retired_name_maps_to_a_play_funds_network() {
        for (retired, replacement) in RETIRED_NETWORKS {
            let mapped = NetworkArg::parse(replacement)
                .unwrap_or_else(|| panic!("{retired}'s replacement {replacement:?} must parse"));
            assert!(
                !Network::from(mapped).is_mainnet(),
                "{retired:?} named a play-funds host, so its replacement must not be real \
                 funds, got {replacement:?}"
            );
        }
    }

    /// Pins the transcription. `From<NetworkArg> for Network` mirrors an axis the
    /// SDK owns, and because `Network` is `#[non_exhaustive]` no match here can
    /// fail when a variant is added upstream — so the pinned version is the only
    /// thing that can notice.
    ///
    /// When an SDK bump trips this: re-read `Network`'s variants, map any new one
    /// (or decide the CLI should not expose it), then move the constant. Expect
    /// `sdk-autobump.yml`'s PR to land on this deliberately.
    #[test]
    fn the_network_axis_was_checked_against_the_pinned_sdk() {
        // `NEXUS_SDK_VERSION` is injected by `build.rs` from `Cargo.lock`, so this
        // is the version actually linked rather than the caret requirement in
        // `Cargo.toml` — a patch release picked up by `cargo update` trips it too,
        // which is right: a `#[non_exhaustive]` enum can gain a variant in one.
        let linked = env!("NEXUS_SDK_VERSION");
        assert_eq!(
            linked, NETWORK_AXIS_VERIFIED_AGAINST,
            "this build links nexus-exchange {linked}, but the {{mainnet, testnet, local}} \
             mapping was last read against {NETWORK_AXIS_VERIFIED_AGAINST}. `Network` is \
             #[non_exhaustive], so the compiler cannot flag a new variant: re-read it, map any \
             new network (or decide not to expose it), then update \
             NETWORK_AXIS_VERIFIED_AGAINST."
        );
    }

    /// The fix for the case the review caught: a config-file network that is
    /// neither valid nor retired used to fall through in complete silence, so the
    /// user believed they had selected a network while the CLI used the default.
    /// Typos are the durable version of this — `stable`/`beta` age out of configs,
    /// misspellings never do.
    #[test]
    fn an_unparseable_config_network_is_always_reported() {
        for name in ["mainet", "prod", "Testnet2", "main net"] {
            let warning = stale_network_warning(name, Some("testnet"))
                .unwrap_or_else(|| panic!("{name:?} must produce a warning, not silence"));
            assert!(
                warning.contains(name),
                "the warning should quote the offending value; got: {warning}"
            );
            assert!(
                warning.contains("not a known network"),
                "the warning should say the value is unknown; got: {warning}"
            );
            for valid in ["mainnet", "testnet", "local"] {
                assert!(
                    warning.contains(valid),
                    "the warning should list `{valid}` as valid; got: {warning}"
                );
            }
            // No retired-channel explanation for a value that never was one.
            assert!(
                !warning.contains("release channel"),
                "unexpected retired-name story for {name:?}: {warning}"
            );
        }
    }

    /// A retired name keeps its own sentence — the migration story is the whole
    /// reason the table exists — and still points at `testnet`, never `mainnet`.
    #[test]
    fn a_retired_config_network_keeps_its_migration_hint() {
        for retired in ["stable", "beta", "STABLE", "  beta  "] {
            let warning = stale_network_warning(retired, Some("testnet"))
                .unwrap_or_else(|| panic!("{retired:?} must produce a warning"));
            assert!(
                warning.contains("release channel"),
                "the warning for {retired:?} should explain what it was; got: {warning}"
            );
            assert!(
                warning.contains("testnet"),
                "the warning for {retired:?} must point at testnet; got: {warning}"
            );
        }
    }

    /// The "where you actually land" clause is read from the resolved default, not
    /// restated from `RETIRED_NETWORKS`, so the warning cannot outlive a change to
    /// the SDK default. Asserted for a retired *and* an unknown name, since the
    /// two take different branches.
    #[test]
    fn the_stale_network_warning_names_where_it_actually_lands() {
        let default_network = Config::default()
            .network()
            .expect("the SDK default targets a named network");
        let landing = network_name(default_network);
        for name in ["stable", "mainet"] {
            let warning = stale_network_warning(name, Some(&landing)).expect("a warning");
            assert!(
                warning.contains(&format!("the default network ({landing})")),
                "the warning should name the resolved default {landing:?}; got: {warning}"
            );
        }
        // With no named default (a base-URL-only config) the claim is dropped
        // rather than guessed.
        let warning = stale_network_warning("mainet", None).expect("a warning");
        assert!(
            warning.contains("using the default network;")
                || warning.contains("using the default network."),
            "with no named default the warning should not invent one; got: {warning}"
        );
    }

    /// A blank value selects nothing, which is not a stale name — `nexus setup`
    /// normalizes an empty answer to "no preference" and a hand-edited
    /// `"network": ""` means the same. Warning there would be noise.
    #[test]
    fn a_blank_config_network_is_not_reported_as_stale() {
        for blank in ["", "   ", "\t"] {
            assert!(
                stale_network_warning(blank, Some("testnet")).is_none(),
                "a blank network ({blank:?}) should be silent, not a warning"
            );
        }
    }

    /// `network_name` is what the warning interpolates, so it has to agree with
    /// the vocabulary `--network` accepts — otherwise the CLI would suggest a
    /// value it cannot parse.
    #[test]
    fn network_name_round_trips_through_the_flag_vocabulary() {
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            let name = network_name(arg.into());
            assert_eq!(
                NetworkArg::parse(&name),
                Some(arg),
                "network_name produced {name:?}, which --network does not accept"
            );
        }
    }

    #[test]
    fn network_args_map_one_to_one_onto_the_sdk_axis() {
        assert_eq!(Network::from(NetworkArg::Mainnet), Network::Mainnet);
        assert_eq!(Network::from(NetworkArg::Testnet), Network::Testnet);
        assert_eq!(Network::from(NetworkArg::Local), Network::Local);
        // Only mainnet is real funds — the predicate the SDK guards `fund()` with.
        assert!(Network::from(NetworkArg::Mainnet).is_mainnet());
        assert!(!Network::from(NetworkArg::Testnet).is_mainnet());
        assert!(!Network::from(NetworkArg::Local).is_mainnet());
    }

    /// `--network mainnet` is accepted here and refused by the SDK at request
    /// time, with a reason. Asserting the CLI does *not* pre-empt it keeps one
    /// gate instead of two that can disagree — the SDK owns the refusal.
    #[test]
    fn mainnet_is_selectable_and_left_to_the_sdk_to_refuse() {
        let cli = Cli::try_parse_from(["nexus", "--network", "mainnet", "markets"]).unwrap();
        assert_eq!(cli.network, Some(NetworkArg::Mainnet));
        assert_eq!(base_url(&cli), Network::Mainnet.base_url());
    }

    #[test]
    fn defaults_to_human_output() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Human);
    }

    #[test]
    fn parses_output_json() {
        let cli = Cli::try_parse_from(["nexus", "--output", "json", "markets"]).unwrap();
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn rejects_unknown_output() {
        assert!(Cli::try_parse_from(["nexus", "--output", "yaml", "markets"]).is_err());
    }

    #[test]
    fn credentials_require_both_halves() {
        let empty = FileConfig::default();
        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "markets"]).unwrap();
        assert!(cli.credentials(&empty).is_none());

        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "--api-secret", "s", "markets"])
            .unwrap();
        assert!(cli.credentials(&empty).is_some());
    }

    #[test]
    fn debug_redacts_api_secret() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--api-key",
            "nx_visible",
            "--api-secret",
            "topsecret",
            "markets",
        ])
        .unwrap();
        let dbg = format!("{cli:?}");
        assert!(!dbg.contains("topsecret"), "secret leaked via Debug: {dbg}");
        assert!(dbg.contains("nx_visible"));
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn credentials_fall_back_to_file() {
        let mut file = FileConfig::default();
        *file.section_mut(DEFAULT_NETWORK) = NetworkCredentials {
            api_key: Some("k".into()),
            api_secret: Some("s".into()),
            session_token: None,
        };
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        assert_eq!(cli.credentials(&file), Some(("k".into(), "s".into())));
    }

    #[test]
    fn flag_overrides_file_credentials() {
        let mut file = FileConfig::default();
        *file.section_mut(DEFAULT_NETWORK) = NetworkCredentials {
            api_key: Some("file-key".into()),
            api_secret: Some("file-secret".into()),
            session_token: None,
        };
        // Flag key layers over the file secret, per-field.
        let cli = Cli::try_parse_from(["nexus", "--api-key", "flag-key", "balance"]).unwrap();
        assert_eq!(
            cli.credentials(&file),
            Some(("flag-key".into(), "file-secret".into()))
        );
    }

    #[test]
    fn sets_descriptive_user_agent() {
        let expected = format!("nexus-cli/{}", env!("CARGO_PKG_VERSION"));

        // Network path.
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.config(&FileConfig::default()).user_agent(), expected);

        // Explicit base-url path also carries the UA.
        let cli = Cli::try_parse_from(["nexus", "--base-url", "http://x:1", "markets"]).unwrap();
        assert_eq!(cli.config(&FileConfig::default()).user_agent(), expected);
    }

    #[test]
    fn completions_parses_bash() {
        let cli = Cli::try_parse_from(["nexus", "completions", "bash"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Completions { shell: Shell::Bash }
        ));
    }

    #[test]
    fn order_place_parses() {
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "place",
            "--market",
            "BTC-USDX-PERP",
            "--side",
            "buy",
            "--type",
            "limit",
            "--price",
            "84000",
            "--quantity",
            "0.01",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action:
                    OrderCommand::Place {
                        market,
                        side,
                        order_type,
                        tif,
                        ..
                    },
            } => {
                assert_eq!(market, "BTC-USDX-PERP");
                assert_eq!(side, SideArg::Buy);
                assert_eq!(order_type, OrderTypeArg::Limit);
                assert_eq!(tif, TifArg::Gtc);
            }
            _ => panic!("expected order place"),
        }
    }

    #[test]
    fn market_summary_parses() {
        let cli = Cli::try_parse_from(["nexus", "market", "summary"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Market {
                action: MarketCommand::Summary
            }
        ));
    }

    #[test]
    fn account_rate_limit_parses() {
        let cli = Cli::try_parse_from(["nexus", "account", "rate-limit"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Account {
                action: AccountCommand::RateLimit
            }
        ));
    }

    #[test]
    fn market_status_takes_a_market_id() {
        let cli = Cli::try_parse_from(["nexus", "market", "status", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::Status { market_id },
            } => assert_eq!(market_id, "BTC-USDX-PERP"),
            _ => panic!("expected market status"),
        }
        // The market id is required.
        assert!(Cli::try_parse_from(["nexus", "market", "status"]).is_err());
    }

    #[test]
    fn market_mark_price_takes_a_market_id() {
        let cli = Cli::try_parse_from(["nexus", "market", "mark-price", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::MarkPrice { market_id },
            } => assert_eq!(market_id, "BTC-USDX-PERP"),
            _ => panic!("expected market mark-price"),
        }
    }

    #[test]
    fn order_cancel_all_conflicts_with_id() {
        // `--all` and a positional id are mutually exclusive.
        assert!(Cli::try_parse_from(["nexus", "order", "cancel", "abc", "--all"]).is_err());
    }

    #[test]
    fn ws_requires_at_least_one_channel() {
        assert!(Cli::try_parse_from(["nexus", "ws"]).is_err());
        let cli =
            Cli::try_parse_from(["nexus", "ws", "trades", "--market", "BTC-USDX-PERP"]).unwrap();
        assert!(matches!(cli.command, Command::Ws { .. }));
    }

    /// `--help` renders, names the binary, and lists the full command surface.
    /// Guards against a command silently dropping out of the top-level help.
    #[test]
    fn top_level_help_lists_every_command() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("nexus"), "help should name the binary");
        for cmd in [
            "markets",
            "market",
            "ticker",
            "tickers",
            "summaries",
            "mark-price",
            "market-status",
            "funding-rates",
            "orderbook",
            "trades",
            "candles",
            "health",
            "balance",
            "account",
            "positions",
            "fills",
            "withdrawals",
            "orders",
            "order",
            "funding-payments",
            "withdrawals",
            "account",
            "auth",
            "keys",
            "agents",
            "transfers",
            "sub-accounts",
            "ws",
            "setup",
            "completions",
        ] {
            assert!(help.contains(cmd), "top-level help should list `{cmd}`");
        }
    }

    /// Every subcommand (and nested subcommand) renders `--help` without
    /// panicking and produces a usage line — exercises the whole help path.
    #[test]
    fn every_subcommand_renders_help() {
        fn check(cmd: &mut clap::Command) {
            let help = cmd.render_long_help().to_string();
            assert!(
                help.contains("Usage:"),
                "`{}` help should have a usage line",
                cmd.get_name()
            );
            for sub in cmd.get_subcommands_mut() {
                check(sub);
            }
        }
        check(&mut Cli::command());
    }

    #[test]
    fn order_get_parses() {
        // `--market` is required (by-id lookups are routed per market).
        assert!(Cli::try_parse_from(["nexus", "order", "get", "o123"]).is_err());
        let cli =
            Cli::try_parse_from(["nexus", "order", "get", "o123", "--market", "BTC-USDX-PERP"])
                .unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::Get { order_id, market },
            } => {
                assert_eq!(order_id, "o123");
                assert_eq!(market, "BTC-USDX-PERP");
            }
            other => panic!("expected order get, got {other:?}"),
        }
    }

    #[test]
    fn order_cancel_by_id_requires_market() {
        // A single by-id cancel needs `--market`; `--all` does not (and the two
        // are mutually exclusive).
        assert!(Cli::try_parse_from(["nexus", "order", "cancel", "o123"]).is_err());
        assert!(Cli::try_parse_from([
            "nexus",
            "order",
            "cancel",
            "--all",
            "--market",
            "BTC-USDX-PERP"
        ])
        .is_err());
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "cancel",
            "o123",
            "--market",
            "BTC-USDX-PERP",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action:
                    OrderCommand::Cancel {
                        order_id, market, ..
                    },
            } => {
                assert_eq!(order_id.as_deref(), Some("o123"));
                assert_eq!(market.as_deref(), Some("BTC-USDX-PERP"));
            }
            other => panic!("expected order cancel, got {other:?}"),
        }
    }

    #[test]
    fn order_amend_requires_market() {
        // Amends are routed per market, so `--market` is required.
        assert!(
            Cli::try_parse_from(["nexus", "order", "amend", "o123", "--price", "100"]).is_err()
        );
        let cli = Cli::try_parse_from([
            "nexus",
            "order",
            "amend",
            "o123",
            "--market",
            "BTC-USDX-PERP",
            "--price",
            "100",
        ])
        .unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::Amend { market, .. },
            } => assert_eq!(market, "BTC-USDX-PERP"),
            other => panic!("expected order amend, got {other:?}"),
        }
    }

    #[test]
    fn account_leverage_parses() {
        let cli =
            Cli::try_parse_from(["nexus", "account", "leverage", "BTC-USDX-PERP", "10"]).unwrap();
        match cli.command {
            Command::Account {
                action:
                    AccountCommand::Leverage {
                        market_id,
                        leverage,
                    },
            } => {
                assert_eq!(market_id, "BTC-USDX-PERP");
                assert_eq!(leverage, 10);
            }
            _ => panic!("expected account leverage"),
        }
    }

    /// `account margin-mode` is withdrawn (ENG-7740): no endpoint backs it, so
    /// offering it was a correctness claim the CLI could not honour. Parsing must
    /// fail rather than accept the arguments and dispatch a request that cannot
    /// succeed. Re-adding the subcommand fails this test on purpose — it may only
    /// come back once ENG-7614 lands and the spec defines the request shape.
    #[test]
    fn account_margin_mode_is_withdrawn() {
        for args in [
            vec![
                "nexus",
                "account",
                "margin-mode",
                "BTC-USDX-PERP",
                "isolated",
            ],
            vec!["nexus", "account", "margin-mode", "BTC-USDX-PERP", "cross"],
            vec!["nexus", "account", "margin-mode"],
        ] {
            let err = Cli::try_parse_from(&args)
                .expect_err(&format!("{args:?} must be rejected, not parsed"));
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::InvalidSubcommand,
                "{args:?} should be an unrecognized-subcommand error, got: {err}"
            );
        }
    }

    /// The withdrawal must also be invisible in help, so `--help` can't keep
    /// advertising a command that cannot work.
    #[test]
    fn account_help_does_not_offer_margin_mode() {
        let mut cli = Cli::command();
        let account = cli
            .get_subcommands_mut()
            .find(|c| c.get_name() == "account")
            .expect("account subcommand");
        let help = account.render_long_help().to_string();
        assert!(
            help.contains("leverage"),
            "sanity: account help should still list leverage: {help}"
        );
        assert!(
            !help.contains("margin-mode"),
            "account help must not offer the withdrawn margin-mode command: {help}"
        );
    }

    /// The top-level `account` summary line must not advertise a margin surface
    /// either. It read "(deposit, credit, leverage, margin, rate-limit)" while the
    /// subcommand existed; leaving "margin" there would keep promising a
    /// capability the CLI does not have, which is the same claim in shorter form.
    #[test]
    fn account_summary_does_not_advertise_margin() {
        let help = Cli::command().render_long_help().to_string();
        let line = help
            .lines()
            .find(|l| l.contains("Manage account settings"))
            .expect("top-level help should summarize `account`");
        assert!(
            !line.contains("margin"),
            "the account summary must not advertise a margin surface: {line:?}"
        );
        assert!(
            line.contains("leverage") && line.contains("rate-limit"),
            "sanity: the summary should still name the settings that do work: {line:?}"
        );
    }

    /// The portfolio-parity reads (ENG-6460) parse as `account` subcommands.
    #[test]
    fn account_portfolio_subcommands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "summary"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::Summary
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "state"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::State
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "account", "fees"])
                .unwrap()
                .command,
            Command::Account {
                action: AccountCommand::Fees
            }
        ));
    }

    #[test]
    fn portfolio_history_defaults_window_and_limit_to_the_server() {
        // Both are optional: omitting them lets the server pick its `day`
        // default and full window rather than the CLI inventing one.
        let cli = Cli::try_parse_from(["nexus", "account", "portfolio-history"]).unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::PortfolioHistory { window, limit },
            } => {
                assert_eq!(window, None);
                assert_eq!(limit, None);
            }
            other => panic!("expected account portfolio-history, got {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "nexus",
            "account",
            "portfolio-history",
            "--window",
            "month",
            "--limit",
            "50",
        ])
        .unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::PortfolioHistory { window, limit },
            } => {
                assert_eq!(window, Some(PortfolioWindowArg::Month));
                assert_eq!(limit, Some(50));
            }
            other => panic!("expected account portfolio-history, got {other:?}"),
        }
    }

    /// `--limit` is bounded by the API's request schema (1..=366) at parse time,
    /// so an out-of-range value is refused before anything is signed or sent.
    #[test]
    fn portfolio_history_limit_is_bounded_by_the_spec() {
        for bad in ["0", "367", "-1", "4294967296"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--limit", bad])
                    .is_err(),
                "--limit {bad} should be rejected"
            );
        }
        for ok in ["1", "366"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--limit", ok])
                    .is_ok(),
                "--limit {ok} should be accepted"
            );
        }
        // The bound tracks the SDK constant rather than a copy of the number.
        assert_eq!(MAX_PORTFOLIO_HISTORY_LIMIT, 366);
    }

    /// `--window` only accepts the spec's closed enum, so no caller-shaped text
    /// can reach the signed query string.
    #[test]
    fn portfolio_history_window_is_a_closed_enum() {
        for bad in ["quarter", "DAY", "", "day; rm -rf /"] {
            assert!(
                Cli::try_parse_from(["nexus", "account", "portfolio-history", "--window", bad])
                    .is_err(),
                "--window {bad:?} should be rejected"
            );
        }
        assert_eq!(
            PortfolioWindow::from(PortfolioWindowArg::All).as_str(),
            "all"
        );
    }

    /// `fills --limit` is now the server-side page size, bounded by the API's
    /// 1..=1000 (nexus-exchange 0.7.0 forwards it instead of the CLI truncating).
    #[test]
    fn fills_limit_is_bounded_by_the_page_size() {
        for bad in ["0", "1001"] {
            assert!(
                Cli::try_parse_from(["nexus", "fills", "--limit", bad]).is_err(),
                "--limit {bad} should be rejected"
            );
        }
        assert!(Cli::try_parse_from(["nexus", "fills", "--limit", "1000"]).is_ok());
        assert_eq!(MAX_FILLS_LIMIT, 1000);
        // Default stays the server's own default page size.
        let cli = Cli::try_parse_from(["nexus", "fills"]).unwrap();
        assert!(matches!(cli.command, Command::Fills { limit: 100 }));
    }

    #[test]
    fn auth_login_parses_and_takes_private_key_flag() {
        let cli =
            Cli::try_parse_from(["nexus", "auth", "login", "--private-key", "0xabc"]).unwrap();
        match cli.command {
            Command::Auth {
                action: AuthCommand::Login { private_key },
            } => assert_eq!(private_key.as_deref(), Some("0xabc")),
            _ => panic!("expected auth login"),
        }
        // The private key is optional (env var / prompt fallback).
        let cli = Cli::try_parse_from(["nexus", "auth", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auth {
                action: AuthCommand::Login { private_key: None }
            }
        ));
    }

    #[test]
    fn agents_register_parses_with_defaults() {
        let cli = Cli::try_parse_from([
            "nexus",
            "agents",
            "register",
            "--agent",
            "0x1234567890abcdef1234567890abcdef12345678",
            "--private-key",
            "0xkey",
        ])
        .unwrap();
        match cli.command {
            Command::Agents {
                action:
                    AgentsCommand::Register {
                        agent,
                        chain_id,
                        nonce,
                        expires_at,
                        label,
                        ..
                    },
            } => {
                assert_eq!(agent, "0x1234567890abcdef1234567890abcdef12345678");
                assert_eq!(chain_id, 393, "chain id defaults to the exchange chain");
                assert_eq!(nonce, None, "nonce defaults at call time, not parse time");
                assert_eq!(expires_at, None);
                assert_eq!(label, None);
            }
            _ => panic!("expected agents register"),
        }
    }

    #[test]
    fn session_token_resolves_flag_over_file() {
        let mut file = FileConfig::default();
        file.section_mut(DEFAULT_NETWORK).session_token = Some("file-token".into());
        // No flag -> file token.
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        assert_eq!(cli.session_token(&file).as_deref(), Some("file-token"));
        // Flag wins.
        let cli =
            Cli::try_parse_from(["nexus", "--session-token", "flag-token", "balance"]).unwrap();
        assert_eq!(cli.session_token(&file).as_deref(), Some("flag-token"));
        // Neither set -> None.
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        assert_eq!(cli.session_token(&FileConfig::default()), None);
    }

    #[test]
    fn debug_redacts_session_token() {
        let cli =
            Cli::try_parse_from(["nexus", "--session-token", "topsecrettoken", "balance"]).unwrap();
        let dbg = format!("{cli:?}");
        assert!(
            !dbg.contains("topsecrettoken"),
            "session token leaked via Debug: {dbg}"
        );
        assert!(dbg.contains("<redacted>"));
    }

    #[test]
    fn keys_and_agents_subcommands_parse() {
        assert!(matches!(
            Cli::try_parse_from(["nexus", "keys", "list"])
                .unwrap()
                .command,
            Command::Keys {
                action: KeysCommand::List
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["nexus", "agents", "revoke", "0xabc"])
                .unwrap()
                .command,
            Command::Agents {
                action: AgentsCommand::Revoke { .. }
            }
        ));
    }

    #[test]
    fn transfers_create_requires_flags() {
        // Missing --to/--amount is an error.
        assert!(Cli::try_parse_from(["nexus", "transfers", "create", "--from", "a"]).is_err());
        let cli = Cli::try_parse_from([
            "nexus",
            "transfers",
            "create",
            "--from",
            "a",
            "--to",
            "b",
            "--amount",
            "5",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Transfers {
                action: TransfersCommand::Create { .. }
            }
        ));
    }

    /// `order place`/`cancel` help spells out their flags, so the trading surface
    /// stays documented.
    #[test]
    fn order_subcommand_help_documents_flags() {
        let mut cli = Cli::command();
        let order = cli
            .get_subcommands_mut()
            .find(|c| c.get_name() == "order")
            .expect("order subcommand");
        let help = order.render_long_help().to_string();
        assert!(help.contains("place"));
        assert!(help.contains("cancel"));
    }

    // ───────────────── per-network credentials (ENG-6462) ─────────────────

    /// [`DEFAULT_NETWORK`] transcribes the SDK's default so credential sections
    /// can be named before a `Config` exists. If the SDK ever moves its default,
    /// the two must move together — otherwise the CLI would read one network's
    /// stored key while sending the request to another.
    #[test]
    fn the_default_network_matches_the_sdk() {
        let sdk_default = Config::default()
            .network()
            .expect("the SDK default targets a named network, not a bare base URL");
        assert_eq!(
            Network::from(DEFAULT_NETWORK),
            sdk_default,
            "DEFAULT_NETWORK is {:?} but the SDK now defaults to {sdk_default:?}; credentials \
             would be read from one network's section and sent to another",
            DEFAULT_NETWORK
        );
    }

    /// `is_real_funds` is what every guardrail keys off, so it must agree with
    /// the SDK's own notion rather than with a hardcoded list that can rot.
    #[test]
    fn real_funds_matches_the_sdk() {
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            assert_eq!(
                arg.is_real_funds(),
                Network::from(arg).is_mainnet(),
                "{} disagrees with the SDK about whether it moves real funds",
                arg.as_str()
            );
        }
    }

    /// The section key is the name users type and hand-edit into the config
    /// file, so it has to survive a round trip through `parse`.
    #[test]
    fn section_keys_round_trip_through_parse() {
        for arg in [NetworkArg::Mainnet, NetworkArg::Testnet, NetworkArg::Local] {
            assert_eq!(NetworkArg::parse(arg.as_str()), Some(arg));
        }
    }

    #[test]
    fn the_credential_network_follows_flag_then_file_then_default() {
        let file = FileConfig {
            network: Some("local".into()),
            ..Default::default()
        };

        let cli = Cli::try_parse_from(["nexus", "--network", "mainnet", "markets"]).unwrap();
        assert_eq!(cli.credential_network(&file), NetworkArg::Mainnet);

        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.credential_network(&file), NetworkArg::Local);
        assert_eq!(
            cli.credential_network(&FileConfig::default()),
            DEFAULT_NETWORK
        );
    }

    /// A base-URL override redirects the request but must not change *whose*
    /// key is presented — there is no network to derive from an arbitrary URL,
    /// and users pointing at a proxy in front of a network still want that
    /// network's credentials.
    #[test]
    fn a_base_url_override_does_not_change_the_credential_namespace() {
        let file = FileConfig {
            network: Some("mainnet".into()),
            ..Default::default()
        };
        let cli = Cli::try_parse_from(["nexus", "--base-url", "http://127.0.0.1:9090", "markets"])
            .unwrap();
        assert_eq!(cli.credential_network(&file), NetworkArg::Mainnet);
    }

    /// The core guarantee of ENG-6462: a key stored for one network is never
    /// offered to another. Server-side enforcement (ENG-6443) would reject it
    /// anyway, but only after the request has left with a real-funds host as its
    /// destination — the refusal belongs on this side too.
    #[test]
    fn a_stored_key_is_never_offered_to_another_network() {
        let mut file = FileConfig::default();
        *file.section_mut(NetworkArg::Testnet) = NetworkCredentials {
            api_key: Some("nx_testnet".into()),
            api_secret: Some("testnet-secret".into()),
            session_token: Some("testnet-token".into()),
        };

        let on_testnet = Cli::try_parse_from(["nexus", "--network", "testnet", "balance"]).unwrap();
        assert_eq!(
            on_testnet.credentials(&file),
            Some(("nx_testnet".into(), "testnet-secret".into()))
        );
        assert_eq!(
            on_testnet.session_token(&file).as_deref(),
            Some("testnet-token")
        );

        let on_mainnet = Cli::try_parse_from(["nexus", "--network", "mainnet", "balance"]).unwrap();
        assert_eq!(
            on_mainnet.credentials(&file),
            None,
            "a testnet key must not authenticate a mainnet invocation"
        );
        assert_eq!(
            on_mainnet.session_token(&file),
            None,
            "a testnet session token must not authenticate a mainnet invocation"
        );
    }

    /// Flags and env stay global: they are a per-invocation override for the
    /// network just selected, so they apply whichever section is active — and
    /// they still win over a stored value for that network.
    #[test]
    fn flags_override_the_selected_networks_section() {
        let mut file = FileConfig::default();
        *file.section_mut(NetworkArg::Mainnet) = NetworkCredentials {
            api_key: Some("stored".into()),
            api_secret: Some("stored-secret".into()),
            session_token: None,
        };
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "mainnet",
            "--api-key",
            "flag",
            "--api-secret",
            "flag-secret",
            "balance",
        ])
        .unwrap();
        assert_eq!(
            cli.credentials(&file),
            Some(("flag".into(), "flag-secret".into()))
        );
    }

    /// Two networks configured at once is the case a flat config could not
    /// express, and the reason for the map.
    #[test]
    fn two_networks_coexist() {
        let mut file = FileConfig::default();
        *file.section_mut(NetworkArg::Testnet) = NetworkCredentials {
            api_key: Some("nx_testnet".into()),
            api_secret: Some("s1".into()),
            session_token: None,
        };
        *file.section_mut(NetworkArg::Mainnet) = NetworkCredentials {
            api_key: Some("nx_mainnet".into()),
            api_secret: Some("s2".into()),
            session_token: None,
        };
        for (flag, expected) in [("testnet", "nx_testnet"), ("mainnet", "nx_mainnet")] {
            let cli = Cli::try_parse_from(["nexus", "--network", flag, "balance"]).unwrap();
            assert_eq!(cli.credentials(&file).unwrap().0, expected);
        }
    }
}
