//! Human-readable and JSON rendering of the SDK's response types.
//!
//! The SDK's wire types are mostly deserialize-only, so we format them by hand
//! rather than re-serializing. Money is the SDK's [`Decimal`], rendered as a
//! decimal string in JSON so no precision is lost and the output round-trips the
//! exact value the exchange sent.

use nexus_exchange::types::{
    AccountSummary, AgentInfo, ApiKeyInfo, CreditResult, DepositResult, Fill, FundingPayment,
    FundingSample, HealthStatus, LeverageUpdate, MarginModeUpdate, MarkPrice, Market, MarketStatus,
    MarketSummary, Ohlcv, Order, OrderBook, OrderResponse, Position, PriceLevel, RateLimitStatus,
    Side, SubAccount, Ticker, Trade, Transfer, Withdrawal,
};
use serde_json::{json, Value};

/// Render an order side enum (`Buy`/`Sell`) for display.
fn side_str(side: Side) -> &'static str {
    match side {
        Side::Buy => "Buy",
        Side::Sell => "Sell",
    }
}

/// Format an optional value, showing `-` when absent.
fn opt<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// Render an optional value as a JSON string, or `null` when absent.
fn opt_json<T: std::fmt::Display>(v: &Option<T>) -> Value {
    match v {
        Some(d) => Value::String(d.to_string()),
        None => Value::Null,
    }
}

/// Pretty-print a JSON value. `serde_json` only fails to serialize on types it
/// cannot represent; the values built here are always representable.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value is always serializable")
}

/// How many book levels / rows to show in human tables before truncating.
const MAX_ROWS: usize = 20;

// ───────────────────────── markets ─────────────────────────

/// Render the markets list as an aligned table.
pub fn markets(markets: &[Market]) -> String {
    if markets.is_empty() {
        return "No markets returned.".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<16}  {:>12}  {:>12}  {:>14}  {:>14}  {:>9}\n",
        "MARKET", "TICK SIZE", "LOT SIZE", "MIN ORDER", "MAX ORDER", "MAX LEV",
    ));
    for m in markets {
        out.push_str(&format!(
            "{:<16}  {:>12}  {:>12}  {:>14}  {:>14}  {:>8}x\n",
            m.market_id,
            m.tick_size,
            m.lot_size,
            m.min_order_size,
            m.max_order_size,
            m.max_leverage,
        ));
    }
    out.push_str(&format!("\n{} market(s).", markets.len()));
    out
}

/// Render the markets list as pretty JSON.
pub fn markets_json(markets: &[Market]) -> String {
    let value: Value = markets
        .iter()
        .map(|m| {
            json!({
                "market_id": m.market_id,
                "tick_size": m.tick_size.to_string(),
                "lot_size": m.lot_size.to_string(),
                "min_order_size": m.min_order_size.to_string(),
                "max_order_size": m.max_order_size.to_string(),
                "max_leverage": m.max_leverage,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── ticker ─────────────────────────

/// Render a single ticker as aligned key/value lines.
pub fn ticker(t: &Ticker) -> String {
    let rows = [
        ("symbol", t.symbol.clone()),
        ("datetime", t.datetime.clone()),
        ("last", opt(&t.last)),
        ("mark price", opt(&t.mark_price)),
        ("index price", opt(&t.index_price)),
        ("bid", opt(&t.bid)),
        ("ask", opt(&t.ask)),
        ("high", opt(&t.high)),
        ("low", opt(&t.low)),
        ("open", opt(&t.open)),
        ("close", opt(&t.close)),
        ("change", opt(&t.change)),
        ("percentage", opt(&t.percentage)),
        ("base volume", opt(&t.base_volume)),
        ("quote volume", opt(&t.quote_volume)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a single ticker as pretty JSON.
pub fn ticker_json(t: &Ticker) -> String {
    let value = json!({
        "symbol": t.symbol,
        "datetime": t.datetime,
        "last": opt_json(&t.last),
        "mark_price": opt_json(&t.mark_price),
        "index_price": opt_json(&t.index_price),
        "bid": opt_json(&t.bid),
        "ask": opt_json(&t.ask),
        "high": opt_json(&t.high),
        "low": opt_json(&t.low),
        "open": opt_json(&t.open),
        "close": opt_json(&t.close),
        "change": opt_json(&t.change),
        "percentage": opt_json(&t.percentage),
        "base_volume": opt_json(&t.base_volume),
        "quote_volume": opt_json(&t.quote_volume),
    });
    pretty(&value)
}

// ───────────────────────── health ─────────────────────────

/// Render the health snapshot as aligned key/value lines.
pub fn health(h: &HealthStatus) -> String {
    let rows = [
        (
            "health",
            h.health.clone().unwrap_or_else(|| "unknown".into()),
        ),
        ("connected", h.connected.to_string()),
        ("events received", h.events_received.to_string()),
        ("fills total", h.fills_total.to_string()),
        ("uptime (s)", h.uptime_seconds.to_string()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<18}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the health snapshot as pretty JSON.
pub fn health_json(h: &HealthStatus) -> String {
    let value = json!({
        "health": h.health.clone().unwrap_or_else(|| "unknown".into()),
        "connected": h.connected,
        "events_received": h.events_received,
        "fills_total": h.fills_total,
        "uptime_seconds": h.uptime_seconds,
    });
    pretty(&value)
}

// ───────────────────────── order book ─────────────────────────

/// Render the order book as two aligned columns (bids | asks).
pub fn orderbook(b: &OrderBook) -> String {
    let mut out = format!("{} order book\n\n", b.symbol);
    out.push_str(&format!(
        "{:>14} {:>14}   |  {:>14} {:>14}\n",
        "BID PRICE", "SIZE", "ASK PRICE", "SIZE"
    ));
    let rows = b.bids.len().max(b.asks.len()).min(MAX_ROWS);
    for i in 0..rows {
        let bid = b
            .bids
            .get(i)
            .map(|l| format!("{:>14} {:>14}", l.price(), l.amount()))
            .unwrap_or_else(|| format!("{:>14} {:>14}", "-", "-"));
        let ask = b
            .asks
            .get(i)
            .map(|l| format!("{:>14} {:>14}", l.price(), l.amount()))
            .unwrap_or_else(|| format!("{:>14} {:>14}", "-", "-"));
        out.push_str(&format!("{bid}   |  {ask}\n"));
    }
    out.push_str(&format!(
        "\n{} bid level(s), {} ask level(s).",
        b.bids.len(),
        b.asks.len()
    ));
    out
}

pub fn orderbook_json(b: &OrderBook) -> String {
    let levels = |ls: &[PriceLevel]| -> Value {
        Value::Array(
            ls.iter()
                .map(|l| json!([l.price().to_string(), l.amount().to_string()]))
                .collect::<Vec<_>>(),
        )
    };
    let value = json!({
        "symbol": b.symbol,
        "timestamp": b.timestamp,
        "datetime": b.datetime,
        "nonce": b.nonce,
        "bids": levels(&b.bids),
        "asks": levels(&b.asks),
    });
    pretty(&value)
}

// ───────────────────────── trades ─────────────────────────

pub fn trades(ts: &[Trade]) -> String {
    if ts.is_empty() {
        return "No trades returned.".to_string();
    }
    let mut out = format!(
        "{:<6}  {:>14}  {:>14}  {:<24}\n",
        "SIDE", "PRICE", "AMOUNT", "TIME"
    );
    for t in ts {
        out.push_str(&format!(
            "{:<6}  {:>14}  {:>14}  {:<24}\n",
            side_str(t.side),
            t.price,
            t.amount,
            t.datetime,
        ));
    }
    out.push_str(&format!("\n{} trade(s).", ts.len()));
    out
}

pub fn trades_json(ts: &[Trade]) -> String {
    let value: Value = ts
        .iter()
        .map(|t| {
            json!({
                "id": t.id,
                "symbol": t.symbol,
                "side": side_str(t.side),
                "price": t.price.to_string(),
                "amount": t.amount.to_string(),
                "cost": t.cost.to_string(),
                "timestamp": t.timestamp,
                "datetime": t.datetime,
                "is_liquidation": t.is_liquidation,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── candles ─────────────────────────

pub fn candles(cs: &[Ohlcv]) -> String {
    if cs.is_empty() {
        return "No candles returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}\n",
        "TIME(ms)", "OPEN", "HIGH", "LOW", "CLOSE", "VOLUME"
    );
    for c in cs {
        out.push_str(&format!(
            "{:<16}  {:>12}  {:>12}  {:>12}  {:>12}  {:>12}\n",
            c.timestamp(),
            c.open(),
            c.high(),
            c.low(),
            c.close(),
            c.volume()
        ));
    }
    out.push_str(&format!("\n{} candle(s).", cs.len()));
    out
}

pub fn candles_json(cs: &[Ohlcv]) -> String {
    // Emit the natural CCXT shape: an array of [ts, o, h, l, c, v]. Money stays a
    // decimal string to preserve precision.
    let value: Value = cs
        .iter()
        .map(|c| {
            json!([
                c.timestamp(),
                c.open().to_string(),
                c.high().to_string(),
                c.low().to_string(),
                c.close().to_string(),
                c.volume().to_string(),
            ])
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── balance / positions ─────────────────────────

pub fn balance(b: &AccountSummary) -> String {
    let rows = [
        ("balance", b.balance.to_string()),
        ("collateral", b.collateral.to_string()),
        ("equity", b.equity.to_string()),
        ("available margin", b.available_margin.to_string()),
    ];
    let mut out = rows
        .iter()
        .map(|(k, v)| format!("{k:<18}{v}"))
        .collect::<Vec<_>>()
        .join("\n");
    if !b.positions.is_empty() {
        out.push_str("\n\n");
        out.push_str(&positions(&b.positions));
    }
    out
}

pub fn balance_json(b: &AccountSummary) -> String {
    let value = json!({
        "balance": b.balance.to_string(),
        "collateral": b.collateral.to_string(),
        "equity": b.equity.to_string(),
        "available_margin": b.available_margin.to_string(),
        "positions": positions_value(&b.positions),
    });
    pretty(&value)
}

pub fn positions(ps: &[Position]) -> String {
    if ps.is_empty() {
        return "No open positions.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:<5}  {:>12}  {:>14}  {:>16}  {:>16}\n",
        "MARKET", "SIDE", "SIZE", "ENTRY", "UNREAL PNL", "LIQ PRICE"
    );
    for p in ps {
        out.push_str(&format!(
            "{:<16}  {:<5}  {:>12}  {:>14}  {:>16}  {:>16}\n",
            p.market_id,
            p.side,
            p.size,
            p.entry_price,
            p.unrealized_pnl,
            opt(&p.liquidation_price),
        ));
    }
    out.push_str(&format!("\n{} position(s).", ps.len()));
    out
}

fn positions_value(ps: &[Position]) -> Value {
    ps.iter()
        .map(|p| {
            json!({
                "market_id": p.market_id,
                "side": p.side,
                "size": p.size.to_string(),
                "entry_price": p.entry_price.to_string(),
                "unrealized_pnl": p.unrealized_pnl.to_string(),
                "realized_pnl": p.realized_pnl.to_string(),
                "liquidation_price": opt_json(&p.liquidation_price),
            })
        })
        .collect()
}

pub fn positions_json(ps: &[Position]) -> String {
    pretty(&positions_value(ps))
}

// ───────────────────────── fills ─────────────────────────

pub fn fills(fs: &[Fill]) -> String {
    if fs.is_empty() {
        return "No fills returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:<5}  {:>14}  {:>12}  {:>10}  {:<7}\n",
        "MARKET", "SIDE", "PRICE", "SIZE", "FEE", "ROLE"
    );
    for f in fs {
        out.push_str(&format!(
            "{:<16}  {:<5}  {:>14}  {:>12}  {:>10}  {:<7}\n",
            f.market_id,
            side_str(f.side),
            f.price,
            f.size,
            f.fee,
            f.taker_or_maker.as_deref().unwrap_or("-"),
        ));
    }
    out.push_str(&format!("\n{} fill(s).", fs.len()));
    out
}

pub fn fills_json(fs: &[Fill]) -> String {
    let value: Value = fs
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "order_id": f.order_id,
                "market_id": f.market_id,
                "side": side_str(f.side),
                "price": f.price.to_string(),
                "size": f.size.to_string(),
                "fee": f.fee.to_string(),
                "taker_or_maker": f.taker_or_maker,
                "timestamp": f.timestamp,
                "is_liquidation": f.is_liquidation,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── orders ─────────────────────────

pub fn orders(os: &[Order]) -> String {
    if os.is_empty() {
        return "No open orders.".to_string();
    }
    let mut out = format!(
        "{:<38}  {:<16}  {:<5}  {:<7}  {:>12}  {:>10}  {:>10}  {:<14}\n",
        "ID", "MARKET", "SIDE", "TYPE", "PRICE", "QTY", "FILLED", "STATUS"
    );
    for o in os {
        out.push_str(&order_row(o));
        out.push('\n');
    }
    out.push_str(&format!("\n{} order(s).", os.len()));
    out
}

fn order_row(o: &Order) -> String {
    format!(
        "{:<38}  {:<16}  {:<5}  {:<7}  {:>12}  {:>10}  {:>10}  {:<14}",
        o.id,
        o.market_id,
        side_str(o.side),
        format!("{:?}", o.order_type),
        opt(&o.price),
        o.quantity,
        o.filled_qty,
        o.status,
    )
}

/// Detailed single-order view (key/value lines).
pub fn order(o: &Order) -> String {
    let rows = [
        ("id", o.id.clone()),
        ("market", o.market_id.clone()),
        ("side", side_str(o.side).to_string()),
        ("type", format!("{:?}", o.order_type)),
        ("price", opt(&o.price)),
        ("quantity", o.quantity.to_string()),
        ("filled", o.filled_qty.to_string()),
        ("status", o.status.clone()),
        ("time in force", format!("{:?}", o.time_in_force)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<16}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn order_value(o: &Order) -> Value {
    json!({
        "id": o.id,
        "market_id": o.market_id,
        "account_id": o.account_id,
        "side": side_str(o.side),
        "order_type": format!("{:?}", o.order_type),
        "price": opt_json(&o.price),
        "quantity": o.quantity.to_string(),
        "filled_qty": o.filled_qty.to_string(),
        "status": o.status,
        "time_in_force": format!("{:?}", o.time_in_force),
        "client_order_id": o.client_order_id,
        "created_at": o.created_at,
        "updated_at": o.updated_at,
    })
}

pub fn orders_json(os: &[Order]) -> String {
    let value: Value = os.iter().map(order_value).collect();
    pretty(&value)
}

/// Render a `POST /orders` result: the order plus a count of immediate fills.
pub fn order_result(r: &OrderResponse) -> String {
    let mut out = order(&r.order);
    out.push_str(&format!("\n{:<16}{}", "immediate fills", r.fills.len()));
    out
}

pub fn order_result_json(r: &OrderResponse) -> String {
    let value = json!({
        "order": order_value(&r.order),
        "fills": r.fills,
    });
    pretty(&value)
}

/// Render a cancel response. The exact body shape isn't fixed by the spec, so
/// we pretty-print whatever the server returned (and a short human note).
pub fn cancel(value: &Value, human_note: &str) -> String {
    format!("{human_note}\n{}", pretty(value))
}

// ───────────────────────── tickers / summaries ─────────────────────────

/// Render every market's ticker as one aligned row each.
pub fn tickers(ts: &std::collections::HashMap<String, Ticker>) -> String {
    if ts.is_empty() {
        return "No tickers returned.".to_string();
    }
    // Sort by symbol so the output is stable across runs (HashMap order isn't).
    let mut rows: Vec<&Ticker> = ts.values().collect();
    rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    let mut out = format!(
        "{:<16}  {:>14}  {:>14}  {:>14}  {:>12}\n",
        "MARKET", "LAST", "BID", "ASK", "CHANGE %"
    );
    for t in &rows {
        out.push_str(&format!(
            "{:<16}  {:>14}  {:>14}  {:>14}  {:>12}\n",
            t.symbol,
            opt(&t.last),
            opt(&t.bid),
            opt(&t.ask),
            opt(&t.percentage),
        ));
    }
    out.push_str(&format!("\n{} ticker(s).", rows.len()));
    out
}

pub fn tickers_json(ts: &std::collections::HashMap<String, Ticker>) -> String {
    let mut rows: Vec<&Ticker> = ts.values().collect();
    rows.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    let value: Value = rows
        .iter()
        .map(|t| {
            json!({
                "symbol": t.symbol,
                "datetime": t.datetime,
                "last": opt_json(&t.last),
                "bid": opt_json(&t.bid),
                "ask": opt_json(&t.ask),
                "percentage": opt_json(&t.percentage),
                "base_volume": opt_json(&t.base_volume),
                "quote_volume": opt_json(&t.quote_volume),
            })
        })
        .collect();
    pretty(&value)
}

/// Render per-market 24h summaries as an aligned table.
pub fn summaries(ss: &[MarketSummary]) -> String {
    if ss.is_empty() {
        return "No market summaries returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:>14}  {:>16}  {:>10}  {:<8}\n",
        "MARKET", "MARK", "VOLUME 24H", "TRADES", "STATUS"
    );
    for s in ss {
        out.push_str(&format!(
            "{:<16}  {:>14}  {:>16}  {:>10}  {:<8}\n",
            s.market_id,
            opt(&s.mark_price),
            s.volume_24h,
            s.trade_count,
            s.status,
        ));
    }
    out.push_str(&format!("\n{} market summary(ies).", ss.len()));
    out
}

pub fn summaries_json(ss: &[MarketSummary]) -> String {
    let value: Value = ss
        .iter()
        .map(|s| {
            json!({
                "market_id": s.market_id,
                "mark_price": opt_json(&s.mark_price),
                "volume_24h": s.volume_24h.to_string(),
                "trade_count": s.trade_count,
                "status": s.status,
                "halt_reason": s.halt_reason,
                "halted_at": s.halted_at,
                "adl_event_count": s.adl_event_count,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── mark price / market status ─────────────────────────

pub fn mark_price(m: &MarkPrice) -> String {
    format!(
        "{:<14}{}\n{:<14}{}",
        "market", m.market_id, "mark price", m.mark_price
    )
}

pub fn mark_price_json(m: &MarkPrice) -> String {
    pretty(&json!({
        "market_id": m.market_id,
        "mark_price": m.mark_price.to_string(),
    }))
}

pub fn market_status(s: &MarketStatus) -> String {
    let rows = [
        ("market", s.market_id.clone()),
        ("status", s.status.clone()),
        (
            "halt reason",
            s.halt_reason.clone().unwrap_or_else(|| "-".into()),
        ),
        ("halted at", opt(&s.halted_at)),
        ("adl events", s.adl_event_count.to_string()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn market_status_json(s: &MarketStatus) -> String {
    pretty(&json!({
        "market_id": s.market_id,
        "status": s.status,
        "halt_reason": s.halt_reason,
        "halted_at": s.halted_at,
        "adl_event_count": s.adl_event_count,
    }))
}

// ───────────────────────── funding ─────────────────────────

pub fn funding_rates(fs: &[FundingSample]) -> String {
    if fs.is_empty() {
        return "No funding samples returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:>16}  {:>14}  {:>14}  {:>14}\n",
        "TIME(ms)", "FUNDING RATE", "PREMIUM", "MARK", "ORACLE"
    );
    for f in fs {
        out.push_str(&format!(
            "{:<16}  {:>16}  {:>14}  {:>14}  {:>14}\n",
            f.timestamp, f.funding_rate, f.premium_index, f.mark_price, f.oracle_price,
        ));
    }
    out.push_str(&format!("\n{} sample(s).", fs.len()));
    out
}

pub fn funding_rates_json(fs: &[FundingSample]) -> String {
    let value: Value = fs
        .iter()
        .map(|f| {
            json!({
                "timestamp": f.timestamp,
                "funding_rate": f.funding_rate.to_string(),
                "premium_index": f.premium_index.to_string(),
                "mark_price": f.mark_price.to_string(),
                "oracle_price": f.oracle_price.to_string(),
            })
        })
        .collect();
    pretty(&value)
}

pub fn funding_payments(fs: &[FundingPayment]) -> String {
    if fs.is_empty() {
        return "No funding payments returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:>16}  {:>14}  {:<16}\n",
        "MARKET", "AMOUNT", "RATE", "TIME(ms)"
    );
    for f in fs {
        out.push_str(&format!(
            "{:<16}  {:>16}  {:>14}  {:<16}\n",
            f.market_id,
            f.amount,
            opt(&f.funding_rate),
            f.timestamp,
        ));
    }
    out.push_str(&format!("\n{} payment(s).", fs.len()));
    out
}

pub fn funding_payments_json(fs: &[FundingPayment]) -> String {
    let value: Value = fs
        .iter()
        .map(|f| {
            json!({
                "market_id": f.market_id,
                "amount": f.amount.to_string(),
                "funding_rate": opt_json(&f.funding_rate),
                "timestamp": f.timestamp,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── single order (get) ─────────────────────────

/// Single-order detail view, reusing the key/value `order` renderer.
pub fn order_detail(o: &Order) -> String {
    order(o)
}

pub fn order_detail_json(o: &Order) -> String {
    pretty(&order_value(o))
}

// ───────────────────────── api keys ─────────────────────────

pub fn api_keys(ks: &[ApiKeyInfo]) -> String {
    if ks.is_empty() {
        return "No API keys.".to_string();
    }
    let mut out = format!("{:<24}  {:<12}\n", "KEY ID", "TIER");
    for k in ks {
        out.push_str(&format!("{:<24}  {:<12}\n", k.key_id, k.tier));
    }
    out.push_str(&format!("\n{} key(s).", ks.len()));
    out
}

pub fn api_keys_json(ks: &[ApiKeyInfo]) -> String {
    let value: Value = ks
        .iter()
        .map(|k| json!({ "key_id": k.key_id, "tier": k.tier }))
        .collect();
    pretty(&value)
}

/// Render a newly created API key. The secret is shown once — surface it
/// prominently and warn it is unrecoverable. `secret` is passed in by the
/// caller (which exposes it from the `SecretString`); this module never holds
/// the secret.
pub fn created_api_key(key_id: &str, secret: &str, tier: Option<&str>) -> String {
    format!(
        "Created API key. Store the secret now — it is shown only once.\n\n\
         {:<14}{}\n{:<14}{}\n{:<14}{}",
        "key id",
        key_id,
        "secret",
        secret,
        "tier",
        tier.unwrap_or("-"),
    )
}

pub fn created_api_key_json(key_id: &str, secret: &str, tier: Option<&str>) -> String {
    pretty(&json!({
        "key_id": key_id,
        "secret": secret,
        "tier": tier,
    }))
}

// ───────────────────────── agents ─────────────────────────

pub fn agents(ags: &[AgentInfo]) -> String {
    if ags.is_empty() {
        return "No registered agents.".to_string();
    }
    let mut out = format!(
        "{:<44}  {:<16}  {:<16}  {:<16}\n",
        "ADDRESS", "EXPIRES(ms)", "REGISTERED(ms)", "LABEL"
    );
    for a in ags {
        out.push_str(&format!(
            "{:<44}  {:<16}  {:<16}  {:<16}\n",
            a.address,
            a.expires_at,
            a.registered_at,
            a.label.as_deref().unwrap_or("-"),
        ));
    }
    out.push_str(&format!("\n{} agent(s).", ags.len()));
    out
}

pub fn agents_json(ags: &[AgentInfo]) -> String {
    let value: Value = ags
        .iter()
        .map(|a| {
            json!({
                "address": a.address,
                "expires_at": a.expires_at,
                "registered_at": a.registered_at,
                "label": a.label,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── account: deposit / credit / rate-limit ─────────────────────────

pub fn deposit(d: &DepositResult) -> String {
    format!("{:<14}{}", "balance", d.balance)
}

pub fn deposit_json(d: &DepositResult) -> String {
    pretty(&json!({ "balance": d.balance.to_string() }))
}

pub fn credit(c: &CreditResult) -> String {
    let rows = [
        ("credited", c.amount.to_string()),
        ("credited today", c.credited_today.to_string()),
        ("daily limit", c.daily_limit.to_string()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<18}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn credit_json(c: &CreditResult) -> String {
    pretty(&json!({
        "amount": c.amount.to_string(),
        "credited_today": c.credited_today.to_string(),
        "daily_limit": c.daily_limit.to_string(),
    }))
}

pub fn rate_limit(r: &RateLimitStatus) -> String {
    let rows = [
        ("tier", r.tier.clone()),
        ("limit", opt(&r.limit)),
        ("remaining", opt(&r.remaining)),
        ("reset at (ms)", opt(&r.reset_at_ms)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<16}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn rate_limit_json(r: &RateLimitStatus) -> String {
    pretty(&json!({
        "tier": r.tier,
        "limit": r.limit,
        "remaining": r.remaining,
        "reset_at_ms": r.reset_at_ms,
    }))
}

// ───────────────────────── leverage / margin mode ─────────────────────────

pub fn leverage(l: &LeverageUpdate) -> String {
    format!(
        "{:<14}{}\n{:<14}{}x",
        "market", l.market_id, "leverage", l.leverage
    )
}

pub fn leverage_json(l: &LeverageUpdate) -> String {
    pretty(&json!({ "market_id": l.market_id, "leverage": l.leverage }))
}

pub fn margin_mode(m: &MarginModeUpdate) -> String {
    format!(
        "{:<14}{}\n{:<14}{:?}",
        "market", m.market_id, "margin mode", m.margin_mode
    )
}

pub fn margin_mode_json(m: &MarginModeUpdate) -> String {
    pretty(&json!({
        "market_id": m.market_id,
        "margin_mode": format!("{:?}", m.margin_mode),
    }))
}

// ───────────────────────── withdrawals / transfers / sub-accounts ─────────────────────────

pub fn withdrawals(ws: &[Withdrawal]) -> String {
    if ws.is_empty() {
        return "No withdrawals.".to_string();
    }
    let mut out = format!(
        "{:<24}  {:>16}  {:<16}  {:<12}\n",
        "ID", "AMOUNT", "TIME(ms)", "STATUS"
    );
    for w in ws {
        out.push_str(&format!(
            "{:<24}  {:>16}  {:<16}  {:<12}\n",
            w.id, w.amount, w.timestamp, w.status,
        ));
    }
    out.push_str(&format!("\n{} withdrawal(s).", ws.len()));
    out
}

pub fn withdrawals_json(ws: &[Withdrawal]) -> String {
    let value: Value = ws
        .iter()
        .map(|w| {
            json!({
                "id": w.id,
                "amount": w.amount.to_string(),
                "timestamp": w.timestamp,
                "status": w.status,
            })
        })
        .collect();
    pretty(&value)
}

pub fn transfers(ts: &[Transfer]) -> String {
    if ts.is_empty() {
        return "No transfers.".to_string();
    }
    let mut out = format!(
        "{:<24}  {:<20}  {:<20}  {:>16}  {:<12}\n",
        "ID", "FROM", "TO", "AMOUNT", "STATUS"
    );
    for t in ts {
        out.push_str(&format!(
            "{:<24}  {:<20}  {:<20}  {:>16}  {:<12}\n",
            t.id, t.from_account, t.to_account, t.amount, t.status,
        ));
    }
    out.push_str(&format!("\n{} transfer(s).", ts.len()));
    out
}

pub fn transfers_json(ts: &[Transfer]) -> String {
    let value: Value = ts.iter().map(transfer_value).collect();
    pretty(&value)
}

fn transfer_value(t: &Transfer) -> Value {
    json!({
        "id": t.id,
        "from_account": t.from_account,
        "to_account": t.to_account,
        "amount": t.amount.to_string(),
        "timestamp": t.timestamp,
        "status": t.status,
    })
}

/// Render a single transfer result (the `POST /transfers` response).
pub fn transfer(t: &Transfer) -> String {
    let rows = [
        ("id", t.id.clone()),
        ("from", t.from_account.clone()),
        ("to", t.to_account.clone()),
        ("amount", t.amount.to_string()),
        ("status", t.status.clone()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<10}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn transfer_json(t: &Transfer) -> String {
    pretty(&transfer_value(t))
}

pub fn sub_accounts(ss: &[SubAccount]) -> String {
    if ss.is_empty() {
        return "No sub-accounts.".to_string();
    }
    let mut out = format!("{:<24}  {:<20}  {:>16}\n", "ACCOUNT ID", "LABEL", "EQUITY");
    for s in ss {
        out.push_str(&format!(
            "{:<24}  {:<20}  {:>16}\n",
            s.account_id,
            if s.label.is_empty() { "-" } else { &s.label },
            opt(&s.equity),
        ));
    }
    out.push_str(&format!("\n{} sub-account(s).", ss.len()));
    out
}

pub fn sub_accounts_json(ss: &[SubAccount]) -> String {
    let value: Value = ss.iter().map(sub_account_value).collect();
    pretty(&value)
}

fn sub_account_value(s: &SubAccount) -> Value {
    json!({
        "account_id": s.account_id,
        "label": s.label,
        "equity": opt_json(&s.equity),
    })
}

/// Render a single created sub-account.
pub fn sub_account(s: &SubAccount) -> String {
    let rows = [
        ("account id", s.account_id.clone()),
        (
            "label",
            if s.label.is_empty() {
                "-".into()
            } else {
                s.label.clone()
            },
        ),
        ("equity", opt(&s.equity)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn sub_account_json(s: &SubAccount) -> String {
    pretty(&sub_account_value(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Sorted so the assertion is independent of serde_json's key ordering
    // (which depends on the `preserve_order` feature).
    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
        k.sort();
        k
    }

    #[test]
    fn markets_json_shape() {
        // The SDK types are deserialize-only, so build fixtures from JSON.
        let markets: Vec<Market> = serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP",
            "base_asset": "BTC",
            "quote_asset": "USDX",
            "tick_size": "0.5",
            "lot_size": "0.001",
            "min_order_size": "0.001",
            "max_order_size": "100",
            "initial_margin_rate": "0.05",
            "maintenance_margin_rate": "0.03",
            "max_leverage": 20
        }]))
        .unwrap();

        let v: Value = serde_json::from_str(&markets_json(&markets)).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(
            keys(row),
            [
                "lot_size",
                "market_id",
                "max_leverage",
                "max_order_size",
                "min_order_size",
                "tick_size",
            ]
        );
        // Money is a decimal string; leverage stays a JSON number.
        assert_eq!(row["tick_size"], json!("0.5"));
        assert_eq!(row["max_leverage"], json!(20));
    }

    #[test]
    fn ticker_json_shape_and_null_contract() {
        let ticker: Ticker = serde_json::from_value(json!({
            "symbol": "BTC-USDX-PERP",
            "timestamp": 1_700_000_000_000i64,
            "datetime": "2023-11-14T22:13:20Z",
            "high": 100.5, "low": 90.0, "bid": null, "bidVolume": null,
            "ask": null, "askVolume": null, "open": 95.0, "close": 99.0,
            "last": 99.0, "change": 4.0, "percentage": 4.2,
            "baseVolume": 12.0, "quoteVolume": 1200.0,
            "markPrice": 99.1, "indexPrice": 99.2
        }))
        .unwrap();

        let v: Value = serde_json::from_str(&ticker_json(&ticker)).unwrap();
        assert_eq!(
            keys(&v),
            [
                "ask",
                "base_volume",
                "bid",
                "change",
                "close",
                "datetime",
                "high",
                "index_price",
                "last",
                "low",
                "mark_price",
                "open",
                "percentage",
                "quote_volume",
                "symbol",
            ]
        );
        // Present money -> decimal string; absent -> JSON null.
        assert_eq!(v["last"], json!("99"));
        assert_eq!(v["bid"], Value::Null);
    }

    #[test]
    fn health_json_shape_and_unknown_default() {
        let health: HealthStatus = serde_json::from_value(json!({
            "events_received": 7,
            "fills_total": 3,
            "uptime_seconds": 42,
            "connected": true
        }))
        .unwrap();

        let v: Value = serde_json::from_str(&health_json(&health)).unwrap();
        assert_eq!(
            keys(&v),
            [
                "connected",
                "events_received",
                "fills_total",
                "health",
                "uptime_seconds",
            ]
        );
        assert_eq!(v["health"], json!("unknown"));
        assert_eq!(v["connected"], json!(true));
        assert_eq!(v["events_received"], json!(7));
    }

    #[test]
    fn orders_json_uses_decimal_strings_and_plain_account_id() {
        let orders: Vec<Order> = serde_json::from_value(json!([{
            "id": "o1",
            "market_id": "BTC-USDX-PERP",
            "account_id": "0xabc",
            "side": "Buy",
            "order_type": "Limit",
            "price": "84000",
            "quantity": "0.01",
            "filled_qty": "0",
            "status": "Open",
            "time_in_force": "GTC"
        }]))
        .unwrap();
        let v: Value = serde_json::from_str(&orders_json(&orders)).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(row["account_id"], json!("0xabc"));
        assert_eq!(row["price"], json!("84000"));
        assert_eq!(row["quantity"], json!("0.01"));
        assert_eq!(row["side"], json!("Buy"));
    }
}
