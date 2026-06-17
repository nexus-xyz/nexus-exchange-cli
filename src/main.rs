//! `nexus` — command-line interface for the Nexus Exchange API.

mod cli;
mod output;

use anyhow::{Context, Result};
use clap::Parser;
use nexus_exchange::Client;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Credentials are accepted but not yet consumed: every command below hits a
    // public, unauthenticated endpoint. Flag it so a user who supplied half a
    // pair (or expected auth) isn't surprised.
    if cli.credentials.api_key.is_some() && !cli.credentials.is_complete() {
        eprintln!("warning: --api-key/$NEXUS_API_KEY set without a matching --api-secret/$NEXUS_API_SECRET");
    }

    let client = Client::new(cli.config());

    match cli.command {
        Command::Markets => {
            let markets = client
                .fetch_markets()
                .await
                .context("failed to fetch markets")?;
            println!("{}", output::markets(&markets));
        }
        Command::Ticker { market_id } => {
            let ticker = client
                .fetch_ticker(&market_id)
                .await
                .with_context(|| format!("failed to fetch ticker for {market_id}"))?;
            println!("{}", output::ticker(&ticker));
        }
        Command::Health => {
            let health = client
                .health_check()
                .await
                .context("failed to fetch health status")?;
            println!("{}", output::health(&health));
        }
    }

    Ok(())
}
