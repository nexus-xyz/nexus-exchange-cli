//! Command-line argument parsing and config/credential resolution.

use clap::{Parser, Subcommand, ValueEnum};
use nexus_exchange::types::{OrderType, Side, TimeInForce};
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

/// Command-line interface for the Nexus Exchange API.
#[derive(Debug, Parser)]
#[command(name = "nexus", version, about, long_about = None)]
pub struct Cli {
    /// Which network to target (default: stable, or the `nexus setup` value).
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

/// Which Nexus Exchange environment to target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum NetworkArg {
    /// Production / stable channel.
    Stable,
    /// Beta channel (tracks `main`; may break).
    Beta,
    /// Local development server.
    Local,
}

impl NetworkArg {
    /// Parse a network name from the config file. Returns `None` for unknown
    /// values so a stale config can't crash the CLI.
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            "local" => Some(Self::Local),
            _ => None,
        }
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
}

impl From<TifArg> for TimeInForce {
    fn from(t: TifArg) -> Self {
        match t {
            TifArg::Gtc => TimeInForce::Gtc,
            TifArg::Ioc => TimeInForce::Ioc,
            TifArg::Fok => TimeInForce::Fok,
        }
    }
}

impl From<NetworkArg> for Network {
    fn from(n: NetworkArg) -> Self {
        match n {
            NetworkArg::Stable => Network::Stable,
            NetworkArg::Beta => Network::Beta,
            NetworkArg::Local => Network::Local,
        }
    }
}

impl Cli {
    /// Resolve the SDK [`Config`], layering: `--base-url` > `--network`/env >
    /// config-file `base_url` > config-file `network` > the SDK default
    /// (stable). Every resolved config carries the CLI's [`USER_AGENT`].
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
            Config::default()
        };
        config.with_user_agent(USER_AGENT)
    }

    /// Resolve an API key/secret pair, layering flags/env over the config file.
    /// Returns `None` when no usable pair is configured. Warns (and still
    /// returns `None`) when only one half is present, since that is almost
    /// always a mistake.
    ///
    /// The pair is handed to [`Config::api_key`] so the SDK signs authenticated
    /// requests; the CLI never touches the secret beyond passing it through.
    pub fn credentials(&self, file: &FileConfig) -> Option<(String, String)> {
        let key = self
            .credentials
            .api_key
            .clone()
            .or_else(|| file.api_key.clone());
        let secret = self
            .credentials
            .api_secret
            .clone()
            .or_else(|| file.api_secret.clone());

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

    /// Resolve a wallet session token, layering flag/env over the config file
    /// (the same precedence as the HMAC pair). Returns `None` when none is
    /// configured. Handed to [`Config::session_token`] only when no HMAC key
    /// pair is present, so the HMAC pair takes precedence as the request signer.
    pub fn session_token(&self, file: &FileConfig) -> Option<String> {
        self.credentials
            .session_token
            .clone()
            .or_else(|| file.session_token.clone())
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

    /// Show the indexer health/status snapshot.
    Health,

    /// Show your account summary (balance, collateral, equity, margin).
    Balance,

    /// List your open positions.
    Positions,

    /// List your recent fills (executions).
    Fills {
        /// Maximum number of fills to return.
        #[arg(long, default_value_t = 100)]
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

    /// Manage account settings (deposit, credit, leverage, margin, rate-limit).
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

    /// ADL settlement events for a market, most recent first. Unlike the other
    /// `market` reads, this endpoint is HMAC-gated server-side, so it requires
    /// credentials.
    AdlEvents {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Maximum number of events to return (server default 100, max 1000).
        #[arg(long)]
        limit: Option<u32>,
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

    /// Cancel a single order by id (requires `--market`), every open order in
    /// one market with `--market` alone, or all open orders with `--all`.
    Cancel {
        /// Order id to cancel. Requires `--market` (by-id cancels are routed
        /// per market).
        #[arg(requires = "market")]
        order_id: Option<String>,
        /// Market to target, e.g. `BTC-USDX-PERP`. With an order id: the
        /// market the order is on (required — the engine routes by-id cancels
        /// per market). Alone: cancel every open order in this market (a
        /// per-market flatten). Not used with `--all`.
        #[arg(long, conflicts_with = "all")]
        market: Option<String>,
        /// Cancel all open orders.
        #[arg(long, conflicts_with = "order_id")]
        all: bool,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
    },

    /// Cancel a batch of orders by id in a single request.
    CancelBatch {
        /// Order ids to cancel (at least one).
        #[arg(required = true, num_args = 1..)]
        order_ids: Vec<String>,
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

    /// Fetch a single order by its caller-assigned client order id.
    GetByClientId {
        /// Client order id assigned at placement (`client_order_id`).
        client_order_id: String,
    },

    /// Cancel a single order by its caller-assigned client order id.
    CancelByClientId {
        /// Client order id assigned at placement (`client_order_id`).
        client_order_id: String,
        /// Skip the confirmation prompt (required when not run interactively).
        #[arg(long)]
        yes: bool,
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

    /// Set the margin mode (cross/isolated) for a market.
    MarginMode {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
        /// Margin mode.
        #[arg(value_enum)]
        mode: MarginModeArg,
    },

    /// ADL settlement events touching an account, where the address was the
    /// bankrupt target or a closed counterparty. Most recent first.
    AdlHistory {
        /// Account address (`0x`-prefixed).
        address: String,
        /// Maximum number of events to return (server default 100, max 1000).
        #[arg(long)]
        limit: Option<u32>,
    },
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

/// Margin mode. Maps onto the SDK's [`MarginMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum MarginModeArg {
    Cross,
    Isolated,
}

impl From<MarginModeArg> for nexus_exchange::types::MarginMode {
    fn from(m: MarginModeArg) -> Self {
        match m {
            MarginModeArg::Cross => nexus_exchange::types::MarginMode::Cross,
            MarginModeArg::Isolated => nexus_exchange::types::MarginMode::Isolated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn defaults_to_stable_network() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.network, None);
        assert_eq!(base_url(&cli), Network::Stable.base_url());
    }

    #[test]
    fn base_url_overrides_network() {
        let cli = Cli::try_parse_from([
            "nexus",
            "--network",
            "beta",
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
            network: Some("beta".into()),
            base_url: None,
            api_key: None,
            api_secret: None,
            session_token: None,
        };
        // No flag → file network wins.
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(
            Client::new(cli.config(&file)).base_url(),
            Network::Beta.base_url()
        );
        // Flag beats the file.
        let cli = Cli::try_parse_from(["nexus", "--network", "local", "markets"]).unwrap();
        assert_eq!(
            Client::new(cli.config(&file)).base_url(),
            Network::Local.base_url()
        );
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
        let file = FileConfig {
            api_key: Some("k".into()),
            api_secret: Some("s".into()),
            ..Default::default()
        };
        let cli = Cli::try_parse_from(["nexus", "balance"]).unwrap();
        assert_eq!(cli.credentials(&file), Some(("k".into(), "s".into())));
    }

    #[test]
    fn flag_overrides_file_credentials() {
        let file = FileConfig {
            api_key: Some("file-key".into()),
            api_secret: Some("file-secret".into()),
            ..Default::default()
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
    fn order_cancel_market_flatten_parses() {
        // `--market` alone is a per-market flatten. It cannot combine with
        // `--all` (with a positional id it is a by-id cancel — see
        // `order_cancel_by_id_requires_market`).
        assert!(Cli::try_parse_from([
            "nexus",
            "order",
            "cancel",
            "--all",
            "--market",
            "BTC-USDX-PERP"
        ])
        .is_err());
        let cli =
            Cli::try_parse_from(["nexus", "order", "cancel", "--market", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Order {
                action:
                    OrderCommand::Cancel {
                        order_id,
                        market,
                        all,
                        ..
                    },
            } => {
                assert_eq!(order_id, None);
                assert_eq!(market.as_deref(), Some("BTC-USDX-PERP"));
                assert!(!all);
            }
            _ => panic!("expected order cancel --market"),
        }
    }

    #[test]
    fn order_cancel_batch_requires_at_least_one_id() {
        assert!(Cli::try_parse_from(["nexus", "order", "cancel-batch"]).is_err());
        let cli = Cli::try_parse_from(["nexus", "order", "cancel-batch", "o1", "o2"]).unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::CancelBatch { order_ids, yes },
            } => {
                assert_eq!(order_ids, vec!["o1".to_string(), "o2".to_string()]);
                assert!(!yes);
            }
            _ => panic!("expected order cancel-batch"),
        }
    }

    #[test]
    fn order_by_client_id_commands_parse() {
        let cli = Cli::try_parse_from(["nexus", "order", "get-by-client-id", "ladder-1"]).unwrap();
        match cli.command {
            Command::Order {
                action: OrderCommand::GetByClientId { client_order_id },
            } => assert_eq!(client_order_id, "ladder-1"),
            _ => panic!("expected order get-by-client-id"),
        }
        let cli =
            Cli::try_parse_from(["nexus", "order", "cancel-by-client-id", "ladder-1"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Order {
                action: OrderCommand::CancelByClientId { .. }
            }
        ));
        // The client order id is required for both.
        assert!(Cli::try_parse_from(["nexus", "order", "get-by-client-id"]).is_err());
        assert!(Cli::try_parse_from(["nexus", "order", "cancel-by-client-id"]).is_err());
    }

    #[test]
    fn market_adl_events_parses_with_optional_limit() {
        let cli = Cli::try_parse_from(["nexus", "market", "adl-events", "BTC-USDX-PERP"]).unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::AdlEvents { market_id, limit },
            } => {
                assert_eq!(market_id, "BTC-USDX-PERP");
                assert_eq!(limit, None, "limit defaults to the server default");
            }
            _ => panic!("expected market adl-events"),
        }
        let cli = Cli::try_parse_from([
            "nexus",
            "market",
            "adl-events",
            "BTC-USDX-PERP",
            "--limit",
            "50",
        ])
        .unwrap();
        match cli.command {
            Command::Market {
                action: MarketCommand::AdlEvents { limit, .. },
            } => assert_eq!(limit, Some(50)),
            _ => panic!("expected market adl-events"),
        }
        // The market id is required.
        assert!(Cli::try_parse_from(["nexus", "market", "adl-events"]).is_err());
    }

    #[test]
    fn account_adl_history_parses() {
        let cli = Cli::try_parse_from(["nexus", "account", "adl-history", "0xabc"]).unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::AdlHistory { address, limit },
            } => {
                assert_eq!(address, "0xabc");
                assert_eq!(limit, None);
            }
            _ => panic!("expected account adl-history"),
        }
        // The address is required.
        assert!(Cli::try_parse_from(["nexus", "account", "adl-history"]).is_err());
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

    #[test]
    fn account_margin_mode_parses_enum() {
        let cli = Cli::try_parse_from([
            "nexus",
            "account",
            "margin-mode",
            "BTC-USDX-PERP",
            "isolated",
        ])
        .unwrap();
        match cli.command {
            Command::Account {
                action: AccountCommand::MarginMode { mode, .. },
            } => assert_eq!(mode, MarginModeArg::Isolated),
            _ => panic!("expected account margin-mode"),
        }
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
        let file = FileConfig {
            session_token: Some("file-token".into()),
            ..Default::default()
        };
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
        assert!(help.contains("cancel-batch"));
        assert!(help.contains("get-by-client-id"));
        assert!(help.contains("cancel-by-client-id"));
    }
}
