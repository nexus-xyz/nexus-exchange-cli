//! Command-line argument parsing.

use clap::{Parser, Subcommand, ValueEnum};
use nexus_exchange::{Config, Network};

/// Command-line interface for the Nexus Exchange API.
#[derive(Debug, Parser)]
#[command(name = "nexus", version, about, long_about = None)]
pub struct Cli {
    /// Which network to target.
    #[arg(long, value_enum, global = true, default_value_t = NetworkArg::Stable, env = "NEXUS_NETWORK")]
    pub network: NetworkArg,

    /// Override the API base URL (takes precedence over `--network`).
    #[arg(long, global = true, env = "NEXUS_BASE_URL")]
    pub base_url: Option<String>,

    #[command(flatten)]
    pub credentials: Credentials,

    #[command(subcommand)]
    pub command: Command,
}

/// API credentials. Read from flags or the corresponding environment
/// variables. The public market-data commands below are unauthenticated, so
/// these are optional today; they are wired up for the authenticated endpoints
/// the SDK adds in follow-ups.
#[derive(Debug, clap::Args)]
pub struct Credentials {
    /// API key.
    #[arg(long, global = true, env = "NEXUS_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// API secret.
    #[arg(long, global = true, env = "NEXUS_API_SECRET", hide_env_values = true)]
    pub api_secret: Option<String>,
}

impl Credentials {
    /// Whether both halves of a credential pair were supplied.
    pub fn is_complete(&self) -> bool {
        self.api_key.is_some() && self.api_secret.is_some()
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
    /// Build the SDK [`Config`] from the parsed network / base-url flags.
    pub fn config(&self) -> Config {
        match &self.base_url {
            Some(url) => Config::with_base_url(url.clone()),
            None => Config::new(self.network.into()),
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List all tradable markets and their trading rules.
    Markets,

    /// Fetch the ticker for a single market, e.g. `BTC-USDX-PERP`.
    Ticker {
        /// Market identifier, e.g. `BTC-USDX-PERP`.
        market_id: String,
    },

    /// Show the indexer health/status snapshot.
    Health,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use nexus_exchange::Client;

    fn base_url(cli: &Cli) -> String {
        Client::new(cli.config()).base_url().to_string()
    }

    /// Catches conflicting flags, bad arg specs, etc. at test time.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_to_stable_network() {
        let cli = Cli::try_parse_from(["nexus", "markets"]).unwrap();
        assert_eq!(cli.network, NetworkArg::Stable);
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
    fn credentials_completeness() {
        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "markets"]).unwrap();
        assert!(!cli.credentials.is_complete());

        let cli = Cli::try_parse_from(["nexus", "--api-key", "k", "--api-secret", "s", "markets"])
            .unwrap();
        assert!(cli.credentials.is_complete());
    }
}
