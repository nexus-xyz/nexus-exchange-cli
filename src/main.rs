//! `nexus` — command-line interface for the Nexus Exchange API.
//!
//! A thin command/output layer over the [`nexus_exchange`] SDK: every request
//! goes through the SDK's [`Client`], which owns request signing, the HTTP/WS
//! transport, retries, rate-limit pacing, and the wire types. This binary only
//! parses arguments, resolves config/credentials, and renders results.

mod cli;
mod credentials;
mod output;
mod wsclient;

use std::io::{self, IsTerminal, Write};
use std::str::FromStr;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use nexus_exchange::types::{AmendOrder, Decimal, OrderRequest, TransferRequest};
use nexus_exchange::{Client, ExposeSecret};

use cli::{
    AccountCommand, AgentsCommand, Cli, Command, KeysCommand, OrderCommand, OutputFormat,
    SubAccountsCommand, TransfersCommand,
};
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

    // Build the SDK config (network / base URL), then attach credentials so the
    // SDK signs authenticated requests. `credentials` is resolved once here so
    // its single "half a pair" warning isn't emitted twice.
    let credentials = cli.credentials(&file);
    let authenticated = credentials.is_some();
    let mut config = cli.config(&file);
    if let Some((key, secret)) = credentials {
        config = config.api_key(key, secret);
    }
    let client = Client::new(config.clone());
    let format = cli.output;

    match cli.command {
        // ── public market data ──
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
        Command::Tickers => {
            let tickers = client
                .fetch_tickers()
                .await
                .context("failed to fetch tickers")?;
            emit(format, output::tickers(&tickers), || {
                output::tickers_json(&tickers)
            });
        }
        Command::Summaries => {
            let summaries = client
                .fetch_market_summaries()
                .await
                .context("failed to fetch market summaries")?;
            emit(format, output::summaries(&summaries), || {
                output::summaries_json(&summaries)
            });
        }
        Command::MarkPrice { market_id } => {
            let mp = client
                .fetch_mark_price(&market_id)
                .await
                .with_context(|| format!("failed to fetch mark price for {market_id}"))?;
            emit(format, output::mark_price(&mp), || {
                output::mark_price_json(&mp)
            });
        }
        Command::MarketStatus { market_id } => {
            let status = client
                .fetch_market_status(&market_id)
                .await
                .with_context(|| format!("failed to fetch market status for {market_id}"))?;
            emit(format, output::market_status(&status), || {
                output::market_status_json(&status)
            });
        }
        Command::FundingRates { market_id, limit } => {
            let samples = client
                .fetch_funding_rate_history(&market_id, Some(limit))
                .await
                .with_context(|| format!("failed to fetch funding rates for {market_id}"))?;
            emit(format, output::funding_rates(&samples), || {
                output::funding_rates_json(&samples)
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
        Command::Orderbook { market_id } => {
            let book = client
                .fetch_order_book(&market_id)
                .await
                .with_context(|| format!("failed to fetch order book for {market_id}"))?;
            emit(format, output::orderbook(&book), || {
                output::orderbook_json(&book)
            });
        }
        Command::Trades { market_id, limit } => {
            let trades = client
                .fetch_trades(&market_id, Some(limit))
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
            let candles = client
                .fetch_ohlcv(&market_id, Some(&timeframe), Some(limit))
                .await
                .with_context(|| format!("failed to fetch candles for {market_id}"))?;
            emit(format, output::candles(&candles), || {
                output::candles_json(&candles)
            });
        }

        // ── authenticated account ──
        Command::Balance => {
            require_authenticated(authenticated, "balance")?;
            let balance = client
                .fetch_balance()
                .await
                .context("failed to fetch account balance")?;
            emit(format, output::balance(&balance), || {
                output::balance_json(&balance)
            });
        }
        Command::Positions => {
            require_authenticated(authenticated, "positions")?;
            let positions = client
                .fetch_positions()
                .await
                .context("failed to fetch positions")?;
            emit(format, output::positions(&positions), || {
                output::positions_json(&positions)
            });
        }
        Command::Fills { limit } => {
            require_authenticated(authenticated, "fills")?;
            let mut fills = client
                .fetch_my_trades()
                .await
                .context("failed to fetch fills")?;
            // The SDK returns the full set; honor the CLI's `--limit` client-side.
            fills.truncate(limit as usize);
            emit(format, output::fills(&fills), || output::fills_json(&fills));
        }
        Command::Orders => {
            require_authenticated(authenticated, "orders")?;
            let orders = client
                .fetch_open_orders()
                .await
                .context("failed to fetch open orders")?;
            emit(format, output::orders(&orders), || {
                output::orders_json(&orders)
            });
        }

        Command::FundingPayments { limit } => {
            require_authenticated(authenticated, "funding-payments")?;
            let mut payments = client
                .fetch_funding_payments(None)
                .await
                .context("failed to fetch funding payments")?;
            // The SDK returns the full set; honor the CLI's `--limit` client-side.
            payments.truncate(limit as usize);
            emit(format, output::funding_payments(&payments), || {
                output::funding_payments_json(&payments)
            });
        }
        Command::Withdrawals => {
            require_authenticated(authenticated, "withdrawals")?;
            let withdrawals = client
                .fetch_withdrawals()
                .await
                .context("failed to fetch withdrawals")?;
            emit(format, output::withdrawals(&withdrawals), || {
                output::withdrawals_json(&withdrawals)
            });
        }

        // ── trading ──
        Command::Order { action } => handle_order(&client, authenticated, action, format).await?,

        // ── account / keys / agents / transfers / sub-accounts ──
        Command::Account { action } => {
            handle_account(&client, authenticated, action, format).await?
        }
        Command::Keys { action } => handle_keys(&client, authenticated, action, format).await?,
        Command::Agents { action } => handle_agents(&client, authenticated, action, format).await?,
        Command::Transfers { action } => {
            handle_transfers(&client, authenticated, action, format).await?
        }
        Command::SubAccounts { action } => {
            handle_sub_accounts(&client, authenticated, action, format).await?
        }

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
                && !authenticated
            {
                eprintln!(
                    "warning: account channels (orders/fills/positions/balances) require credentials; \
                     they will be empty without `nexus setup` or --api-key/--api-secret"
                );
            }
            wsclient::stream(&client, &config, authenticated, &subs, format).await?;
        }

        Command::Completions { .. } | Command::Setup => unreachable!("handled above"),
    }

    Ok(())
}

/// Handle `order place` / `order cancel`, including the safety confirmation
/// prompt for these mutating actions.
async fn handle_order(
    client: &Client,
    authenticated: bool,
    action: OrderCommand,
    format: OutputFormat,
) -> Result<()> {
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
            require_authenticated(authenticated, "order place")?;
            let quantity = parse_amount("quantity", &quantity)?;

            use cli::OrderTypeArg;
            let mut request = match order_type {
                OrderTypeArg::Limit => {
                    let p = price
                        .as_deref()
                        .context("--price is required for a limit order")?;
                    let price = parse_amount("price", p)?;
                    OrderRequest::limit(market.clone(), side.into(), price, quantity, tif.into())
                }
                OrderTypeArg::Market => {
                    if price.is_some() {
                        eprintln!("note: --price is ignored for a market order");
                    }
                    OrderRequest::market(market.clone(), side.into(), quantity)
                }
            };
            if reduce_only {
                request.reduce_only = Some(true);
            }

            let summary = format!(
                "Place {:?} {:?} order: {} {} @ {} (tif {:?}{})",
                request.side,
                request.order_type,
                request.quantity,
                market,
                request
                    .price
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "market".into()),
                request.time_in_force,
                if reduce_only { ", reduce-only" } else { "" },
            );
            if !confirm(&summary, yes)? {
                eprintln!("aborted.");
                return Ok(());
            }

            let result = client
                .create_order(&request)
                .await
                .context("failed to place order")?;
            emit(format, output::order_result(&result), || {
                output::order_result_json(&result)
            });
        }

        OrderCommand::Cancel { order_id, all, yes } => {
            require_authenticated(authenticated, "order cancel")?;
            if all {
                if !confirm("Cancel ALL open orders", yes)? {
                    eprintln!("aborted.");
                    return Ok(());
                }
                let value = client
                    .cancel_all_orders()
                    .await
                    .context("failed to cancel orders")?;
                emit(
                    format,
                    output::cancel(&value, "cancelled all open orders."),
                    || serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
            } else {
                let id = order_id
                    .context("provide an order id, or use --all to cancel every open order")?;
                if !confirm(&format!("Cancel order {id}"), yes)? {
                    eprintln!("aborted.");
                    return Ok(());
                }
                let value = client
                    .cancel_order(&id)
                    .await
                    .context("failed to cancel order")?;
                emit(
                    format,
                    output::cancel(&value, &format!("cancelled order {id}.")),
                    || serde_json::to_string_pretty(&value).unwrap_or_default(),
                );
            }
        }

        OrderCommand::Get { order_id } => {
            require_authenticated(authenticated, "order get")?;
            let order = client
                .fetch_order(&order_id)
                .await
                .with_context(|| format!("failed to fetch order {order_id}"))?;
            emit(format, output::order_detail(&order), || {
                output::order_detail_json(&order)
            });
        }

        OrderCommand::Amend {
            order_id,
            price,
            quantity,
            tif,
            yes,
        } => {
            require_authenticated(authenticated, "order amend")?;
            let mut amend = AmendOrder::new();
            if let Some(p) = price.as_deref() {
                amend = amend.price(parse_amount("price", p)?);
            }
            if let Some(q) = quantity.as_deref() {
                amend = amend.quantity(parse_amount("quantity", q)?);
            }
            if let Some(t) = tif {
                amend = amend.time_in_force(t.into());
            }
            if !confirm(&format!("Amend order {order_id}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let result = client
                .amend_order(&order_id, &amend)
                .await
                .with_context(|| format!("failed to amend order {order_id}"))?;
            emit(format, output::order_result(&result), || {
                output::order_result_json(&result)
            });
        }

        OrderCommand::Batch { file, yes } => {
            require_authenticated(authenticated, "order batch")?;
            let requests = read_order_batch(&file)?;
            if requests.is_empty() {
                anyhow::bail!("the batch file contains no orders");
            }
            if !confirm(
                &format!("Submit a batch of {} order(s)", requests.len()),
                yes,
            )? {
                eprintln!("aborted.");
                return Ok(());
            }
            let value = client
                .create_orders(&requests)
                .await
                .context("failed to submit order batch")?;
            emit(
                format,
                output::cancel(&value, &format!("submitted {} order(s).", requests.len())),
                || serde_json::to_string_pretty(&value).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

/// Handle the `nexus account` subcommands.
async fn handle_account(
    client: &Client,
    authenticated: bool,
    action: AccountCommand,
    format: OutputFormat,
) -> Result<()> {
    match action {
        AccountCommand::Deposit { amount, yes } => {
            require_authenticated(authenticated, "account deposit")?;
            let amount = parse_amount("amount", &amount)?;
            if !confirm(&format!("Deposit {amount}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let result = client.deposit(amount).await.context("failed to deposit")?;
            emit(format, output::deposit(&result), || {
                output::deposit_json(&result)
            });
        }
        AccountCommand::Credit { amount } => {
            require_authenticated(authenticated, "account credit")?;
            let amount = match amount.as_deref() {
                Some(a) => Some(parse_amount("amount", a)?),
                None => None,
            };
            let result = client
                .claim_credit(amount)
                .await
                .context("failed to claim credit")?;
            emit(format, output::credit(&result), || {
                output::credit_json(&result)
            });
        }
        AccountCommand::RateLimit => {
            require_authenticated(authenticated, "account rate-limit")?;
            let status = client
                .fetch_rate_limit_status()
                .await
                .context("failed to fetch rate-limit status")?;
            emit(format, output::rate_limit(&status), || {
                output::rate_limit_json(&status)
            });
        }
        AccountCommand::Leverage {
            market_id,
            leverage,
        } => {
            require_authenticated(authenticated, "account leverage")?;
            let result = client
                .set_leverage(&market_id, leverage)
                .await
                .with_context(|| format!("failed to set leverage for {market_id}"))?;
            emit(format, output::leverage(&result), || {
                output::leverage_json(&result)
            });
        }
        AccountCommand::MarginMode { market_id, mode } => {
            require_authenticated(authenticated, "account margin-mode")?;
            let result = client
                .set_margin_mode(&market_id, mode.into())
                .await
                .with_context(|| format!("failed to set margin mode for {market_id}"))?;
            emit(format, output::margin_mode(&result), || {
                output::margin_mode_json(&result)
            });
        }
    }
    Ok(())
}

/// Handle the `nexus keys` subcommands.
async fn handle_keys(
    client: &Client,
    authenticated: bool,
    action: KeysCommand,
    format: OutputFormat,
) -> Result<()> {
    match action {
        KeysCommand::List => {
            require_authenticated(authenticated, "keys list")?;
            let keys = client
                .fetch_api_keys()
                .await
                .context("failed to fetch API keys")?;
            emit(format, output::api_keys(&keys), || {
                output::api_keys_json(&keys)
            });
        }
        KeysCommand::Create { yes } => {
            require_authenticated(authenticated, "keys create")?;
            if !confirm("Create a new API key", yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let created = client
                .create_api_key()
                .await
                .context("failed to create API key")?;
            // Expose the one-time secret once for display; the SDK keeps it in
            // a zeroizing SecretString otherwise.
            let secret = created.secret.expose_secret();
            let tier = created.tier.as_deref();
            emit(
                format,
                output::created_api_key(&created.key_id, secret, tier),
                || output::created_api_key_json(&created.key_id, secret, tier),
            );
        }
        KeysCommand::Delete { key_id, yes } => {
            require_authenticated(authenticated, "keys delete")?;
            if !confirm(&format!("Delete API key {key_id}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let value = client
                .delete_api_key(&key_id)
                .await
                .with_context(|| format!("failed to delete API key {key_id}"))?;
            emit(
                format,
                output::cancel(&value, &format!("deleted API key {key_id}.")),
                || serde_json::to_string_pretty(&value).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

/// Handle the `nexus agents` subcommands.
async fn handle_agents(
    client: &Client,
    authenticated: bool,
    action: AgentsCommand,
    format: OutputFormat,
) -> Result<()> {
    match action {
        AgentsCommand::List => {
            require_authenticated(authenticated, "agents list")?;
            let agents = client
                .fetch_agents()
                .await
                .context("failed to fetch agents")?;
            emit(format, output::agents(&agents), || {
                output::agents_json(&agents)
            });
        }
        AgentsCommand::Revoke { address, yes } => {
            require_authenticated(authenticated, "agents revoke")?;
            if !confirm(&format!("Revoke agent {address}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let value = client
                .revoke_agent(&address)
                .await
                .with_context(|| format!("failed to revoke agent {address}"))?;
            emit(
                format,
                output::cancel(&value, &format!("revoked agent {address}.")),
                || serde_json::to_string_pretty(&value).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

/// Handle the `nexus transfers` subcommands.
async fn handle_transfers(
    client: &Client,
    authenticated: bool,
    action: TransfersCommand,
    format: OutputFormat,
) -> Result<()> {
    match action {
        TransfersCommand::List => {
            require_authenticated(authenticated, "transfers list")?;
            let transfers = client
                .fetch_transfers()
                .await
                .context("failed to fetch transfers")?;
            emit(format, output::transfers(&transfers), || {
                output::transfers_json(&transfers)
            });
        }
        TransfersCommand::Create {
            from,
            to,
            amount,
            yes,
        } => {
            require_authenticated(authenticated, "transfers create")?;
            let amount = parse_amount("amount", &amount)?;
            if !confirm(&format!("Transfer {amount} from {from} to {to}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let request = TransferRequest::new(from, to, amount);
            let transfer = client
                .create_transfer(&request)
                .await
                .context("failed to create transfer")?;
            emit(format, output::transfer(&transfer), || {
                output::transfer_json(&transfer)
            });
        }
    }
    Ok(())
}

/// Handle the `nexus sub-accounts` subcommands.
async fn handle_sub_accounts(
    client: &Client,
    authenticated: bool,
    action: SubAccountsCommand,
    format: OutputFormat,
) -> Result<()> {
    match action {
        SubAccountsCommand::List => {
            require_authenticated(authenticated, "sub-accounts list")?;
            let accounts = client
                .fetch_sub_accounts()
                .await
                .context("failed to fetch sub-accounts")?;
            emit(format, output::sub_accounts(&accounts), || {
                output::sub_accounts_json(&accounts)
            });
        }
        SubAccountsCommand::Create { label, yes } => {
            require_authenticated(authenticated, "sub-accounts create")?;
            if !confirm(&format!("Create sub-account {label:?}"), yes)? {
                eprintln!("aborted.");
                return Ok(());
            }
            let account = client
                .create_sub_account(&label)
                .await
                .context("failed to create sub-account")?;
            emit(format, output::sub_account(&account), || {
                output::sub_account_json(&account)
            });
        }
    }
    Ok(())
}

/// One entry in a batch file. Mirrors the `order place` flags so a batch reads
/// the same way the single-order command does. Amounts are JSON strings to
/// preserve decimal precision. The SDK's `OrderRequest` is serialize-only, so
/// we deserialize this CLI-side shape and build the request from it.
#[derive(serde::Deserialize)]
struct BatchOrder {
    market: String,
    side: cli::SideArg,
    #[serde(rename = "type")]
    order_type: cli::OrderTypeArg,
    #[serde(default)]
    price: Option<String>,
    quantity: String,
    #[serde(default)]
    tif: Option<cli::TifArg>,
    #[serde(default)]
    reduce_only: Option<bool>,
    #[serde(default)]
    client_order_id: Option<String>,
}

impl BatchOrder {
    fn into_request(self) -> Result<OrderRequest> {
        use cli::OrderTypeArg;
        let quantity = parse_amount("quantity", &self.quantity)?;
        let tif = self.tif.unwrap_or(cli::TifArg::Gtc);
        let mut request = match self.order_type {
            OrderTypeArg::Limit => {
                let p = self
                    .price
                    .as_deref()
                    .context("price is required for a limit order in the batch")?;
                let price = parse_amount("price", p)?;
                OrderRequest::limit(self.market, self.side.into(), price, quantity, tif.into())
            }
            OrderTypeArg::Market => OrderRequest::market(self.market, self.side.into(), quantity),
        };
        request.reduce_only = self.reduce_only;
        request.client_order_id = self.client_order_id;
        Ok(request)
    }
}

/// Read a JSON array of batch order objects from a file path, or from stdin
/// when `file` is `-`, and convert them into SDK [`OrderRequest`]s.
fn read_order_batch(file: &str) -> Result<Vec<OrderRequest>> {
    let bytes = if file == "-" {
        let mut buf = Vec::new();
        io::Read::read_to_end(&mut io::stdin(), &mut buf)
            .context("failed to read order batch from stdin")?;
        buf
    } else {
        std::fs::read(file).with_context(|| format!("failed to read order batch file {file}"))?
    };
    let entries: Vec<BatchOrder> = serde_json::from_slice(&bytes)
        .context("order batch must be a JSON array of order objects")?;
    entries.into_iter().map(BatchOrder::into_request).collect()
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
fn require_authenticated(authenticated: bool, what: &str) -> Result<()> {
    if !authenticated {
        anyhow::bail!(
            "'{what}' is an authenticated command but no credentials are configured \
             (run `nexus setup` or set NEXUS_API_KEY/NEXUS_API_SECRET)"
        );
    }
    Ok(())
}

/// Parse a price/quantity into the SDK's [`Decimal`], rejecting non-positive or
/// malformed values so we don't ship an obviously-bad order to the exchange.
fn parse_amount(field: &str, value: &str) -> Result<Decimal> {
    match Decimal::from_str(value) {
        Ok(n) if n > Decimal::ZERO => Ok(n),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_amount_accepts_positive_decimals() {
        assert_eq!(
            parse_amount("price", "84000").unwrap(),
            Decimal::from_str("84000").unwrap()
        );
        assert_eq!(
            parse_amount("quantity", "0.001").unwrap(),
            Decimal::from_str("0.001").unwrap()
        );
    }

    #[test]
    fn parse_amount_rejects_zero_negative_and_garbage() {
        for bad in ["0", "-1", "-0.5", "abc", "", "1.2.3", "NaN"] {
            let err =
                parse_amount("amount", bad).expect_err(&format!("{bad:?} should be rejected"));
            // The field name and the offending value are echoed back.
            let msg = err.to_string();
            assert!(
                msg.contains("amount"),
                "message should name the field: {msg}"
            );
            assert!(
                msg.contains("positive"),
                "message should explain the constraint: {msg}"
            );
        }
    }

    #[test]
    fn require_authenticated_gates_on_the_flag() {
        assert!(require_authenticated(true, "balance").is_ok());
        let err = require_authenticated(false, "balance").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("balance"), "names the command: {msg}");
        assert!(msg.contains("credentials"), "explains why: {msg}");
    }

    #[test]
    fn confirm_short_circuits_on_yes() {
        // With --yes we proceed without reading stdin (so this is safe in CI,
        // where stdin is not a terminal).
        assert!(confirm("do a thing", true).unwrap());
    }

    #[test]
    fn confirm_refuses_non_interactive_without_yes() {
        // Tests run with a non-terminal stdin, so this exercises the refusal
        // path rather than blocking on a read.
        let err = confirm("place order", false).unwrap_err();
        assert!(err.to_string().contains("refusing to proceed"));
    }

    #[test]
    fn build_subscriptions_validates_channels() {
        // Unknown channel is rejected and the error lists the valid sets.
        let err = build_subscriptions(&["bogus".into()], None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unknown channel"), "{msg}");
        assert!(msg.contains("trades"), "lists public channels: {msg}");
        assert!(msg.contains("orders"), "lists account channels: {msg}");

        // A public channel requires --market.
        let err = build_subscriptions(&["trades".into()], None, None).unwrap_err();
        assert!(err.to_string().contains("requires --market"));
    }

    #[test]
    fn build_subscriptions_scopes_market_correctly() {
        let subs = build_subscriptions(
            &["trades".into(), "orders".into()],
            Some("BTC-USDX-PERP".into()),
            Some(42),
        )
        .unwrap();
        assert_eq!(subs.len(), 2);
        // Public channel keeps the market; account channel drops it.
        assert_eq!(subs[0].channel, "trades");
        assert_eq!(subs[0].market.as_deref(), Some("BTC-USDX-PERP"));
        assert_eq!(subs[0].since, Some(42));
        assert_eq!(subs[1].channel, "orders");
        assert_eq!(subs[1].market, None);
    }

    #[test]
    fn emit_runs_the_json_closure_only_for_json() {
        use std::cell::Cell;
        // Human format must not invoke the JSON renderer.
        let called = Cell::new(false);
        emit(OutputFormat::Human, "human".into(), || {
            called.set(true);
            "json".into()
        });
        assert!(!called.get(), "JSON closure should be skipped for Human");

        // JSON format must invoke it.
        let called = Cell::new(false);
        emit(OutputFormat::Json, "human".into(), || {
            called.set(true);
            "json".into()
        });
        assert!(called.get(), "JSON closure should run for Json");
    }
}
