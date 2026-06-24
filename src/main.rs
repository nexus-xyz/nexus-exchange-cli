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
use nexus_exchange::auth::AgentRegistration;
use nexus_exchange::types::{AmendOrder, Decimal, OrderRequest, TransferRequest};
use nexus_exchange::{Client, EthSigner, ExposeSecret};

use cli::{
    AccountCommand, AgentsCommand, AuthCommand, Cli, Command, KeysCommand, OrderCommand,
    OutputFormat, SubAccountsCommand, TransfersCommand,
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
    let session_token = cli.session_token(&file);
    // Either credential path authenticates account-scoped commands. The HMAC
    // pair is the request signer; the session token is a fallback used only
    // when no pair is configured (both set `Config::credentials`, last wins).
    let authenticated = credentials.is_some() || session_token.is_some();
    let mut config = cli.config(&file);
    match (credentials, session_token) {
        (Some((key, secret)), _) => config = config.api_key(key, secret),
        (None, Some(token)) => config = config.session_token(token),
        (None, None) => {}
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
        Command::Auth { action } => handle_auth(&client, action, format).await?,
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

/// Resolve the raw EVM private key for a wallet-signed command: use the value
/// supplied via flag/env (clap already merged those, with the env value hidden
/// from `--help`/`Debug`), otherwise fall back to a hidden interactive prompt
/// when stdin is a terminal. The key is never echoed and never persisted.
fn resolve_private_key(flag_or_env: Option<String>) -> Result<String> {
    if let Some(key) = flag_or_env {
        if key.trim().is_empty() {
            anyhow::bail!("private key is empty");
        }
        return Ok(key);
    }
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "no private key provided — pass --private-key, set NEXUS_PRIVATE_KEY,              or run interactively to be prompted"
        );
    }
    let key = rpassword::prompt_password("EVM private key (input hidden): ")
        .context("failed to read private key")?;
    if key.trim().is_empty() {
        anyhow::bail!("private key is empty");
    }
    Ok(key)
}

/// Handle the `nexus auth` subcommands (wallet-signed sign-in).
async fn handle_auth(client: &Client, action: AuthCommand, format: OutputFormat) -> Result<()> {
    match action {
        AuthCommand::Login { private_key } => {
            // Build the signer from the raw key (validated by the SDK), then
            // EIP-191-sign the fixed sign-in challenge and exchange it for a
            // session token. The SDK keeps the key in a zeroizing secret; the
            // CLI drops the `EthSigner` as soon as the request is sent.
            let signer = EthSigner::from_hex(resolve_private_key(private_key)?)
                .context("invalid EVM private key")?;
            let login = client.sign_in(&signer).await.context("failed to sign in")?;

            // Persist the token via the session-token credential path (0600).
            let token = login.token.expose_secret();
            let path =
                credentials::save_session_token(token).context("failed to save session token")?;
            let path = path.display().to_string();
            emit(format, output::login(&login.address, token, &path), || {
                output::login_json(&login.address, token, &path)
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
        AgentsCommand::Register {
            agent,
            private_key,
            expires_at,
            nonce,
            chain_id,
            label,
            yes,
        } => {
            // Wallet-signed (EIP-712) and unauthenticated: the owning wallet's
            // signature is the authorization, so no API key / session token is
            // required. Defaults: expiry = now + 30d, nonce = now (Unix ms),
            // both inside the spec's accepted ranges.
            let now_ms = unix_millis()?;
            let expires_at = expires_at.unwrap_or(now_ms + THIRTY_DAYS_MS);
            let nonce = nonce.unwrap_or(now_ms);

            let signer = EthSigner::from_hex(resolve_private_key(private_key)?)
                .context("invalid EVM private key")?;
            let registration: AgentRegistration = signer
                .register_agent(&agent, expires_at, nonce, chain_id, label)
                .context("failed to sign agent registration")?;

            if !confirm(
                &format!(
                    "Register agent {agent} (expires {expires_at} ms, nonce {nonce}, chain {chain_id})"
                ),
                yes,
            )? {
                eprintln!("aborted.");
                return Ok(());
            }

            let result = client
                .register_agent(&registration)
                .await
                .context("failed to register agent")?;
            emit(
                format,
                output::agent_registered(&result.agent_address, result.expires_at),
                || output::agent_registered_json(&result.agent_address, result.expires_at),
            );
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

/// Thirty days in milliseconds — the default agent-registration expiry window
/// (the spec accepts `[now+1d, now+90d]`).
const THIRTY_DAYS_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Current Unix time in milliseconds, used to default the agent-registration
/// nonce and expiry.
fn unix_millis() -> Result<u64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    Ok(now.as_millis() as u64)
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

    // Canonical Hardhat/ethers account #0 — a published, externally verifiable
    // keypair, so the address and signatures below are deterministic and pinned
    // against the SDK's own known-answer vectors (`src/auth/eth.rs`).
    const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
    const TEST_ADDR: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    /// The sign-in path the CLI drives produces the exact, deterministic EIP-191
    /// signature for a known key. This is what `auth login` sends as the body.
    #[test]
    fn sign_in_is_deterministic_for_a_known_key() {
        let signer = EthSigner::from_hex(TEST_KEY).unwrap();
        assert_eq!(signer.address(), TEST_ADDR);

        let a = signer.sign_in().unwrap();
        let b = signer.sign_in().unwrap();
        assert_eq!(a.signature, b.signature, "signing must be deterministic");
        assert_eq!(
            a.signature,
            "0xff4ddf3b1af438fe00d02368ad8fa5fc5e57667e6826dbda3ddddc395a5287bb6eab0bc97652f6e7e1f08f665b868ca143da79e18dae8021799cdafc4af670ea1b"
        );
    }

    /// The register-agent path produces the exact, deterministic EIP-712
    /// signature for a known key, chain id, expiry, and nonce.
    #[test]
    fn register_agent_is_deterministic_for_a_known_key() {
        let signer = EthSigner::from_hex(TEST_KEY).unwrap();
        let agent = "0x1234567890abcdef1234567890abcdef12345678";
        let reg: AgentRegistration = signer
            .register_agent(agent, 1_782_000_000_000, 1, 393, None)
            .unwrap();
        assert_eq!(reg.wallet, TEST_ADDR);
        assert_eq!(reg.agent, agent);
        assert_eq!(
            reg.signature,
            "0x5df263ed6d1b619a72d436a01104f9036af6258cacf56dea973321cbe722a99550644eea6bf75656d48e982d2ce5db9ef13c4aced4539cf3c2ff87802b0197cc1b"
        );
    }

    /// A malformed private key is rejected before any network call.
    #[test]
    fn from_hex_rejects_a_bad_key() {
        assert!(EthSigner::from_hex("not-hex").is_err());
    }

    /// `resolve_private_key` returns a flag/env value verbatim and never logs it.
    #[test]
    fn resolve_private_key_passes_through_flag_value() {
        let key = resolve_private_key(Some(TEST_KEY.to_string())).unwrap();
        assert_eq!(key, TEST_KEY);
        // An empty value is rejected rather than silently used.
        assert!(resolve_private_key(Some("   ".to_string())).is_err());
    }

    /// Neither the rendered login output nor its JSON form ever contains the
    /// private key — only the recovered address, the session token, and the
    /// save path. Guards against the key leaking through the output layer.
    #[test]
    fn login_output_never_contains_the_private_key() {
        let signer = EthSigner::from_hex(TEST_KEY).unwrap();
        // The flow renders the *response* (address + token), not the key.
        let human = output::login(&signer.address(), "sess_tok_123", "/tmp/config.json");
        let json = output::login_json(&signer.address(), "sess_tok_123", "/tmp/config.json");
        for out in [&human, &json] {
            assert!(!out.contains(TEST_KEY), "private key leaked: {out}");
            assert!(out.contains(TEST_ADDR));
            assert!(out.contains("sess_tok_123"));
        }
    }
}
