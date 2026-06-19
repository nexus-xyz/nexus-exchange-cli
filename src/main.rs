//! `nexus` — command-line interface for the Nexus Exchange API.

mod cli;
mod output;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use nexus_exchange::Client;

use cli::{Cli, Command, OutputFormat};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Shell completions need neither network nor credentials — generate and exit
    // before constructing the client or emitting credential warnings.
    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "nexus", &mut std::io::stdout());
        return Ok(());
    }

    // Credentials are accepted but not yet consumed: every command below hits a
    // public, unauthenticated endpoint. Flag it so a user who supplied half a
    // pair (or expected auth) isn't surprised.
    if cli.credentials.api_key.is_some() && !cli.credentials.is_complete() {
        eprintln!("warning: --api-key/$NEXUS_API_KEY set without a matching --api-secret/$NEXUS_API_SECRET");
    }

    let client = Client::new(cli.config());
    let format = cli.output;

    match cli.command {
        Command::Markets => {
            let markets = client
                .fetch_markets()
                .await
                .context("failed to fetch markets")?;
            match format {
                OutputFormat::Human => println!("{}", output::markets(&markets)),
                OutputFormat::Json => println!("{}", output::markets_json(&markets)),
            }
        }
        Command::Ticker { market_id } => {
            let ticker = client
                .fetch_ticker(&market_id)
                .await
                .with_context(|| format!("failed to fetch ticker for {market_id}"))?;
            match format {
                OutputFormat::Human => println!("{}", output::ticker(&ticker)),
                OutputFormat::Json => println!("{}", output::ticker_json(&ticker)),
            }
        }
        Command::Health => {
            let health = client
                .health_check()
                .await
                .context("failed to fetch health status")?;
            match format {
                OutputFormat::Human => println!("{}", output::health(&health)),
                OutputFormat::Json => println!("{}", output::health_json(&health)),
            }
        }
        Command::Completions { .. } => unreachable!("handled above"),
    }

    Ok(())
}
