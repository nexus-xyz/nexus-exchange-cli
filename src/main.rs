//! `nexus` — command-line interface for the Nexus Exchange API.

mod api;
mod auth;
mod cli;
mod credentials;
mod output;
mod wire;
mod wsclient;

use std::io::{self, IsTerminal, Write};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use nexus_exchange::Client;

use api::ApiClient;
use cli::{Cli, Command, OrderCommand, OutputFormat};
use wire::NewOrder;
use wsclient::{Subscription, ACCOUNT_CHANNELS, PUBLIC_CHANNELS};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Shell completions need neither network nor credentials — generate and exit
    // before touching config or the network.
    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(shell, &mut Cli::command(), "nexus", &mut io::stdout());
        return Ok(());
    }

    // `setup` is purely local and manages its own file I/O.
    if let Command::Setup = cli.command {
        return credentials::setup();
    }

    // Layer flags/env over the config file (a malformed file is a hard error so
    // the user notices rather than silently losing settings).
    let file = credentials::load()?.unwrap_or_default();

    // The SDK serves the public methods it already ships; `ApiClient` carries
    // everything else, both built from the same resolved base URL.
    let client = Client::new(cli.config(&file));
    let base_url = client.base_url().to_string();
    let api = ApiClient::new(base_url, cli.signer(&file))?;
    let format = cli.output;

    match cli.command {
        // ── public market data (via the SDK) ──
        Command::Markets => {
            let markets = client
                .fetch_markets()
                .await
                .context("failed to fetch markets")?;
            emit(format, output::markets(&markets), || {
                output::markets_json(&markets)
            });
        }
        Command::Ticker { market_id } => {
            let ticker = client
                .fetch_ticker(&market_id)
                .await
                .with_context(|| format!("failed to fetch ticker for {market_id}"))?;
            emit(format, output::ticker(&ticker), || {
                output::ticker_json(&ticker)
            });
        }
        Command::Health => {
            let health = client
                .health_check()
                .await
                .context("failed to fetch health status")?;
            emit(format, output::health(&health), || {
                output::health_json(&health)
            });
        }

        // ── public market data (via the signed client) ──
        Command::Orderbook { market_id } => {
            let book = api
                .fetch_orderbook(&market_id)
                .await
                .with_context(|| format!("failed to fetch order book for {market_id}"))?;
            emit(format, output::orderbook(&book), || {
                output::orderbook_json(&book)
            });
        }
        Command::Trades { market_id, limit } => {
            let trades = api
                .fetch_trades(&market_id, limit)
                .await
                .with_context(|| format!("failed to fetch trades for {market_id}"))?;
            emit(format, output::trades(&trades), || {
                output::trades_json(&trades)
            });
        }
        Command::Candles {
            market_id,
            timeframe,
            limit,
        } => {
            let candles = api
                .fetch_candles(&market_id, &timeframe, limit)
                .await
                .with_context(|| format!("failed to fetch candles for {market_id}"))?;
            emit(format, output::candles(&candles), || {
                output::candles_json(&candles)
            });
        }

        // ── authenticated account ──
        Command::Balance => {
            require_authenticated(&api, "balance")?;
            let balance = api
                .fetch_balance()
                .await
                .context("failed to fetch account balance")?;
            emit(format, output::balance(&balance), || {
                output::balance_json(&balance)
            });
        }
        Command::Positions => {
            require_authenticated(&api, "positions")?;
            let positions = api
                .fetch_positions()
                .await
                .context("failed to fetch positions")?;
            emit(format, output::positions(&positions), || {
                output::positions_json(&positions)
            });
        }
        Command::Fills { limit } => {
            require_authenticated(&api, "fills")?;
            let fills = api
                .fetch_fills(limit)
                .await
                .context("failed to fetch fills")?;
            emit(format, output::fills(&fills), || output::fills_json(&fills));
        }
        Command::Orders => {
            require_authenticated(&api, "orders")?;
            let orders = api
                .fetch_open_orders()
                .await
                .context("failed to fetch open orders")?;
            emit(format, output::orders(&orders), || {
                output::orders_json(&orders)
            });
        }

        // ── trading ──
        Command::Order { action } => handle_order(&api, action, format).await?,

        // ── websocket ──
        Command::Ws {
            channels,
            market,
            since,
        } => {
            let subs = build_subscriptions(&channels, market, since)?;
            if subs
                .iter()
                .any(|s| ACCOUNT_CHANNELS.contains(&s.channel.as_str()))
                && !api.is_authenticated()
            {
                eprintln!(
                    "warning: account channels (orders/fills/positions/balances) require credentials; \
                     they will be empty without `nexus setup` or --api-key/--api-secret"
                );
            }
            wsclient::stream(&api, &subs, format).await?;
        }

        Command::Completions { .. } | Command::Setup => unreachable!("handled above"),
    }

    Ok(())
}

/// Handle `order place` / `order cancel`, including the safety confirmation
/// prompt for these mutating actions.
async fn handle_order(api: &ApiClient, action: OrderCommand, format: OutputFormat) -> Result<()> {
    match action {
        OrderCommand::Place {
            market,
            side,
            order_type,
            price,
            quantity,
            tif,
            reduce_only,
            yes,
        } => {
            require_authenticated(api, "order place")?;
            validate_amount("quantity", &quantity)?;

            use cli::OrderTypeArg;
            let price = match order_type {
                OrderTypeArg::Limit => {
                    let p = price.context("--price is required for a limit order")?;
                    validate_amount("price", &p)?;
                    Some(p)
                }
                OrderTypeArg::Market => {
                    if price.is_some() {
                        eprintln!("note: --price is ignored for a market order");
                    }
                    None
                }
            };

            let new_order = NewOrder {
                market_id: market.clone(),
                side: side.wire().to_string(),
                order_type: order_type.wire().to_string(),
                price: price.clone(),
                quantity: quantity.clone(),
                time_in_force: tif.wire().to_string(),
                reduce_only: if reduce_only { Some(true) } else { None },
            };

            let summary = format!(
                "Place {} {} order: {} {} @ {} (tif {}{})",
                side.wire(),
                order_type.wire(),
                quantity,
                market,
                price.as_deref().unwrap_or("market"),
                tif.wire(),
                if reduce_only { ", reduce-only" } else { "" },
            );
            if !confirm(&summary, yes)? {
                eprintln!("aborted.");
                return Ok(());
            }

            let result = api
                .place_order(&new_order)
                .await
                .context("failed to place order")?;
            emit(format, output::order_result(&result), || {
                output::order_result_json(&result)
            });
        }

        OrderCommand::Cancel {
            order_id,
            all,
            market,
            yes,
        } => {
            require_authenticated(api, "order cancel")?;
            if all {
                let scope = market
                    .as_deref()
                    .map(|m| format!(" in {m}"))
                    .unwrap_or_else(|| " across all markets".to_string());
                if !confirm(&format!("Cancel ALL open orders{scope}"), yes)? {
                    eprintln!("aborted.");
                    return Ok(());
                }
                let value = api
                    .cancel_all(market.as_deref())
                    .await
                    .context("failed to cancel orders")?;
                emit(
                    format,
                    output::cancel(&value, "cancelled all matching orders."),
                    || serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
            } else {
                let id = order_id
                    .context("provide an order id, or use --all to cancel every open order")?;
                let market =
                    market.context("--market is required when cancelling a single order")?;
                if !confirm(&format!("Cancel order {id} in {market}"), yes)? {
                    eprintln!("aborted.");
                    return Ok(());
                }
                let value = api
                    .cancel_order(&id, &market)
                    .await
                    .context("failed to cancel order")?;
                emit(
                    format,
                    output::cancel(&value, &format!("cancelled order {id}.")),
                    || serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
            }
        }
    }
    Ok(())
}

/// Map CLI channel arguments to validated [`Subscription`]s.
fn build_subscriptions(
    channels: &[String],
    market: Option<String>,
    since: Option<i64>,
) -> Result<Vec<Subscription>> {
    let mut subs = Vec::with_capacity(channels.len());
    for channel in channels {
        let is_public = PUBLIC_CHANNELS.contains(&channel.as_str());
        let is_account = ACCOUNT_CHANNELS.contains(&channel.as_str());
        if !is_public && !is_account {
            anyhow::bail!(
                "unknown channel '{channel}'. Public: {}. Account: {}.",
                PUBLIC_CHANNELS.join(", "),
                ACCOUNT_CHANNELS.join(", "),
            );
        }
        if is_public && market.is_none() {
            anyhow::bail!("channel '{channel}' requires --market");
        }
        subs.push(Subscription {
            channel: channel.clone(),
            // Account channels are scoped by the token, so drop any market.
            market: if is_public { market.clone() } else { None },
            since,
        });
    }
    Ok(subs)
}

/// Print human or JSON output. The JSON renderer is a closure so it is only run
/// for the format actually selected.
fn emit(format: OutputFormat, human: String, json: impl FnOnce() -> String) {
    match format {
        OutputFormat::Human => println!("{human}"),
        OutputFormat::Json => println!("{}", json()),
    }
}

/// Fail fast when an account-scoped command is invoked without credentials.
/// These requests can only ever succeed signed, so we error (non-zero exit)
/// instead of sending an unsigned request that surfaces as an opaque 401 —
/// this stops scripts from silently mis-authenticating.
fn require_authenticated(api: &ApiClient, what: &str) -> Result<()> {
    if !api.is_authenticated() {
        anyhow::bail!(
            "'{what}' is an authenticated command but no credentials are configured \
             (run `nexus setup` or set NEXUS_API_KEY/NEXUS_API_SECRET)"
        );
    }
    Ok(())
}

/// Validate that a price/quantity string is a positive decimal, so we don't ship
/// an obviously-bad order to the exchange.
fn validate_amount(field: &str, value: &str) -> Result<()> {
    match value.parse::<f64>() {
        Ok(n) if n > 0.0 && n.is_finite() => Ok(()),
        _ => anyhow::bail!("{field} must be a positive number, got {value:?}"),
    }
}

/// Confirm a mutating action. Returns `Ok(true)` to proceed. With `--yes`, skips
/// the prompt; without a terminal and without `--yes`, refuses outright so an
/// automated context can never place/cancel by accident.
fn confirm(prompt: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "{prompt}: refusing to proceed without confirmation — pass --yes to skip the prompt"
        );
    }
    eprint!("{prompt}? [y/N]: ");
    io::stderr().flush().ok();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read confirmation")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
