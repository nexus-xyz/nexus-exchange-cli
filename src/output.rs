//! Human-readable and JSON rendering of the SDK's response types.
//!
//! The SDK's wire types are mostly deserialize-only, so we format them by hand
//! rather than re-serializing. Money is the SDK's [`Decimal`], rendered as a
//! decimal string in JSON so no precision is lost and the output round-trips the
//! exact value the exchange sent.

use nexus_exchange::types::{
    AccountFees, AccountFunding, AccountPortfolioSummary, AccountState, AccountSummary, AdlEvent,
    AgentInfo, ApiKeyInfo, BridgeAsset, BridgeAssetsResponse, BridgeDeposit, BridgeDepositAddress,
    CancelOnDisconnectStatus, ClosedPosition, CreditResult, Decimal, DepositResponse,
    DepositResult, EquityPoint, FaucetResponse, Fill, FundingDirection, FundingPremiumSample,
    FundingSample, FundsEntry, FundsKind, FundsStatus, HealthStatus, MarginAdjustment, MarkPrice,
    Market, MarketRiskParams, MarketStatus, MarketSummary, Ohlcv, Order, OrderBook,
    OrderHistoryEntry, OrderPreview, OrderResponse, OrderResult, PortfolioHistory, Position,
    PriceLevel, RateLimitStatus, Side, StatsSnapshot, ThroughputSample, Ticker, Trade, Withdrawal,
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

/// Neutralize control characters in a free-form, server-supplied string before
/// it reaches a terminal.
///
/// Open strings the server chooses — a position's `side`, a fee `tier` /
/// `schedule`, the served portfolio `window`, a `*_error` reason — are echoed
/// straight into the user's terminal, which *interprets* ESC sequences: colour,
/// cursor movement, clear-screen. That is enough to hide or forge a line of
/// output (a fabricated "withdrawable" figure, say), so the escape byte never
/// leaves this function intact. Only C0/C1 controls and DEL are replaced; the
/// text is otherwise passed through verbatim, so legitimate values are
/// unchanged. JSON output needs no equivalent — `serde_json` escapes control
/// characters itself, and machine consumers don't interpret them.
fn safe(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Render an optional value as a JSON string, or `null` when absent.
fn opt_json<T: std::fmt::Display>(v: &Option<T>) -> Value {
    match v {
        Some(d) => Value::String(d.to_string()),
        None => Value::Null,
    }
}

/// Format a unix-millisecond timestamp as an ISO-8601 UTC string
/// (`YYYY-MM-DDTHH:MM:SSZ`), matching the `datetime` fields the SDK pre-formats
/// elsewhere. Done with date arithmetic (Howard Hinnant's `civil_from_days`) to
/// avoid pulling in a `chrono`/`time` dependency for this single field.
fn ms_to_iso8601(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // Shift the epoch to 0000-03-01 so leap days fall at the end of the cycle.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day-of-era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day-of-year [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], Mar=0
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Format an optional unix-millisecond timestamp as ISO-8601, or `-` when absent.
fn opt_ms_iso(ms: &Option<i64>) -> String {
    ms.map(ms_to_iso8601).unwrap_or_else(|| "-".to_string())
}

/// Render an optional unix-millisecond timestamp as an ISO-8601 JSON string, or
/// `null` when absent — keeping JSON timestamps consistent with `datetime`.
fn opt_ms_iso_json(ms: &Option<i64>) -> Value {
    match ms {
        Some(ms) => Value::String(ms_to_iso8601(*ms)),
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
///
/// The v0.7.1 spec replaced the old `/health` liveness probe with the aggregate
/// `GET /status` snapshot: a worst-of `status` (`ok`/`degraded`/`down`/
/// `starting`), the `timestamp_ms` it was taken, and a free-form, evolving
/// `services` object of per-component detail. We surface the status and
/// timestamp, and append the raw `services` payload (compact JSON) when the
/// server includes it, without assuming its shape.
pub fn health(h: &HealthStatus) -> String {
    // Friendly substitution for the human view only: an empty/slim payload reads
    // as "unknown" here, while `health_json` deliberately passes the raw value
    // through verbatim so scripts see exactly what the server sent.
    let status = if h.status.is_empty() {
        "unknown"
    } else {
        h.status.as_str()
    };
    let mut rows = vec![
        ("status", status.to_string()),
        ("timestamp (ms)", h.timestamp_ms.to_string()),
    ];
    if !h.services.is_null() {
        rows.push(("services", h.services.to_string()));
    }
    rows.iter()
        .map(|(k, v)| format!("{k:<18}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the health snapshot as pretty JSON, passing the `GET /status` shape
/// through verbatim (`status`, `timestamp_ms`, and the opaque `services`) — no
/// `""`→`"unknown"` substitution, unlike the human [`health`] renderer, so the
/// machine-readable view stays faithful to the wire.
pub fn health_json(h: &HealthStatus) -> String {
    let value = json!({
        "status": h.status,
        "timestamp_ms": h.timestamp_ms,
        "services": h.services,
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

/// Render open positions: the core table, then the enriched per-position risk
/// table, then any reasons the server could not derive a risk field.
///
/// Two narrow tables rather than one very wide one — the combined column set
/// runs past 140 characters and would wrap on a normal terminal, which is worse
/// than reading two aligned blocks.
///
/// A risk field the server could not derive comes back `null` **with a reason in
/// its companion `*_error`**, never as a fabricated number, so a blank cell here
/// means "not computable" and never `0`. The reasons are listed under the table
/// so a `-` is explained rather than mistaken for a zero.
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
            safe(&p.market_id),
            safe(&p.side),
            p.size,
            p.entry_price,
            p.unrealized_pnl,
            opt(&p.liquidation_price),
        ));
    }

    out.push_str(&format!(
        "\n{:<16}  {:>16}  {:>14}  {:>10}  {:>9}  {:>14}\n",
        "MARKET", "NOTIONAL", "MARGIN USED", "ROE", "MAX LEV", "FUNDING PAID"
    ));
    for p in ps {
        out.push_str(&format!(
            "{:<16}  {:>16}  {:>14}  {:>10}  {:>9}  {:>14}\n",
            safe(&p.market_id),
            opt(&p.notional_value),
            opt(&p.margin_used),
            opt(&p.roe),
            p.max_leverage
                .map(|l| format!("{l}x"))
                .unwrap_or_else(|| "-".to_string()),
            opt(&p.funding_paid),
        ));
    }

    out.push_str(&format!("\n{} position(s).", ps.len()));

    let unavailable = position_errors(ps);
    if !unavailable.is_empty() {
        out.push_str(
            "\n\nNot computable by the server (shown as `-` above — this means unknown, not zero):",
        );
        for (field, reason, markets) in unavailable {
            out.push_str(&format!("\n  {field} ({reason}): {}", markets.join(", ")));
        }
    }
    out
}

/// Collect `(field, reason, markets)` for every risk field the server reported as
/// `null` *with* a reason. A field that is simply absent (an older server) has no
/// reason and is left out rather than invented.
///
/// Grouped by `(field, reason)` because the common case repeats: `leverage` is
/// currently never derivable, so an ungrouped list would print the same sentence
/// once per position and bury the reasons that differ.
fn position_errors(ps: &[Position]) -> Vec<(&'static str, String, Vec<String>)> {
    let mut rows: Vec<(&'static str, String, Vec<String>)> = Vec::new();
    for p in ps {
        let fields: [(&'static str, bool, &Option<String>); 5] = [
            ("leverage", p.leverage.is_none(), &p.leverage_error),
            (
                "notional value",
                p.notional_value.is_none(),
                &p.notional_value_error,
            ),
            ("roe", p.roe.is_none(), &p.roe_error),
            ("margin used", p.margin_used.is_none(), &p.margin_used_error),
            (
                "max leverage",
                p.max_leverage.is_none(),
                &p.max_leverage_error,
            ),
        ];
        for (name, missing, reason) in fields {
            if let (true, Some(reason)) = (missing, reason.as_deref()) {
                let reason = safe(reason);
                let market = safe(&p.market_id);
                match rows.iter_mut().find(|(f, r, _)| *f == name && *r == reason) {
                    Some((_, _, markets)) => markets.push(market),
                    None => rows.push((name, reason, vec![market])),
                }
            }
        }
    }
    rows
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
                // Enriched risk detail. Each value is `null` — never `0` — when
                // the server could not derive it, and the companion `*_error`
                // carries the machine-readable reason. Pair them: `null` with a
                // reason is "not computable, because X", not a zero.
                "leverage": opt_json(&p.leverage),
                "leverage_error": p.leverage_error,
                "notional_value": opt_json(&p.notional_value),
                "notional_value_error": p.notional_value_error,
                "roe": opt_json(&p.roe),
                "roe_error": p.roe_error,
                "margin_used": opt_json(&p.margin_used),
                "margin_used_error": p.margin_used_error,
                // A count, not money: stays a JSON number (or null).
                "max_leverage": p.max_leverage,
                "max_leverage_error": p.max_leverage_error,
                "funding_paid": opt_json(&p.funding_paid),
            })
        })
        .collect()
}

pub fn positions_json(ps: &[Position]) -> String {
    pretty(&positions_value(ps))
}

// ───────────────── portfolio: summary / state / fees / history ─────────────────

/// Render the portfolio summary (`GET /api/v1/account/summary`).
///
/// Every field is optional on the wire, so an unreported one shows `-`. That is
/// deliberately **not** `0`: substituting zero for an unreported aggregate would
/// make an underwater account read as flat. A footnote spells this out whenever
/// something is missing.
///
/// Note this can only be reached on a successful read: the endpoint fails closed
/// (`502 authoritative_margin_unavailable`) rather than serving an estimated
/// `withdrawable`, and the CLI surfaces that as an error, never as an empty
/// account.
pub fn account_summary(s: &AccountPortfolioSummary) -> String {
    let rows = [
        ("collateral", opt(&s.collateral)),
        ("total equity", opt(&s.total_equity)),
        ("unrealized pnl", opt(&s.total_unrealized_pnl)),
        ("realized pnl 24h", opt(&s.total_realized_pnl_24h)),
        ("volume 24h", opt(&s.total_volume_24h)),
        ("open positions", opt(&s.open_positions_count)),
        ("open orders", opt(&s.open_orders_count)),
        ("margin used", opt(&s.margin_used)),
        ("available margin", opt(&s.available_margin)),
        ("withdrawable", opt(&s.withdrawable)),
        ("early access", opt(&s.early_access_allowed)),
    ];
    let mut out = rows
        .iter()
        .map(|(k, v)| format!("{k:<20}{v}"))
        .collect::<Vec<_>>()
        .join("\n");
    if rows.iter().any(|(_, v)| v == "-") {
        out.push_str("\n\n`-` means the server did not report that field — it does not mean zero.");
    }
    out
}

fn account_summary_value(s: &AccountPortfolioSummary) -> Value {
    // An absent field stays `null`: a caller must be able to tell "not reported"
    // from a real `0`, so nothing here is defaulted.
    json!({
        "collateral": opt_json(&s.collateral),
        "total_equity": opt_json(&s.total_equity),
        "total_unrealized_pnl": opt_json(&s.total_unrealized_pnl),
        "total_realized_pnl_24h": opt_json(&s.total_realized_pnl_24h),
        "total_volume_24h": opt_json(&s.total_volume_24h),
        "open_positions_count": s.open_positions_count,
        "open_orders_count": s.open_orders_count,
        "margin_used": opt_json(&s.margin_used),
        "available_margin": opt_json(&s.available_margin),
        "withdrawable": opt_json(&s.withdrawable),
        "early_access_allowed": s.early_access_allowed,
    })
}

pub fn account_summary_json(s: &AccountPortfolioSummary) -> String {
    pretty(&account_summary_value(s))
}

/// Render the consolidated account state (`GET /api/v1/account/state`): the
/// summary and the position list, which come from one server-side read and so
/// cannot disagree with each other.
pub fn account_state(s: &AccountState) -> String {
    format!(
        "{}\n\n{}",
        account_summary(&s.summary),
        positions(&s.positions)
    )
}

pub fn account_state_json(s: &AccountState) -> String {
    pretty(&json!({
        "summary": account_summary_value(&s.summary),
        "positions": positions_value(&s.positions),
    }))
}

/// Render the account's effective fee schedule (`GET /api/v1/account/fees`).
///
/// The maker fee is signed — a negative value is a rebate paid to the maker — so
/// it is labelled rather than left to be misread as a charge. The rate is scoped
/// to the reported `schedule`, not a venue-wide guarantee, which the footnote
/// says out loud.
pub fn account_fees(f: &AccountFees) -> String {
    let maker = if f.maker_fee_bps < 0 {
        format!("{} bps (rebate paid to you)", f.maker_fee_bps)
    } else {
        format!("{} bps", f.maker_fee_bps)
    };
    let volume = if f.volume_30d_estimated {
        format!("{} (estimated — may undercount)", f.volume_30d)
    } else {
        f.volume_30d.to_string()
    };
    let discounts = if f.discounts.is_empty() {
        "none".to_string()
    } else {
        // The discount shape is not fixed by the spec yet, so pass the server's
        // objects through as compact JSON rather than inventing a layout.
        f.discounts
            .iter()
            .map(|d| Value::Object(d.fields.clone()).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let rows = [
        ("maker fee", maker),
        ("taker fee", format!("{} bps", f.taker_fee_bps)),
        ("tier", safe(&f.tier)),
        ("schedule", safe(&f.schedule)),
        ("volume 30d", volume),
        ("discounts", discounts),
    ];
    let mut out = rows
        .iter()
        .map(|(k, v)| format!("{k:<16}{v}"))
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&format!(
        "\n\nForward-looking schedule rate for the `{}` schedule, not a realized per-fill average.",
        safe(&f.schedule)
    ));
    out
}

pub fn account_fees_json(f: &AccountFees) -> String {
    pretty(&json!({
        // Basis points stay JSON numbers, and maker stays signed: a negative
        // value is a rebate, so it must not be rendered unsigned.
        "maker_fee_bps": f.maker_fee_bps,
        "taker_fee_bps": f.taker_fee_bps,
        "tier": f.tier,
        "schedule": f.schedule,
        "volume_30d": f.volume_30d.to_string(),
        "volume_30d_estimated": f.volume_30d_estimated,
        "discounts": f.discounts.iter().map(|d| Value::Object(d.fields.clone())).collect::<Vec<_>>(),
    }))
}

/// Render the portfolio time series (`GET /api/v1/account/portfolio-history`).
///
/// The header reports the window the server actually **served** (which may
/// differ from the one requested) and the sample cadence. An unrecognized window
/// is displayed rather than dropped, so a value added to a later spec is still
/// visible here.
pub fn portfolio_history(h: &PortfolioHistory) -> String {
    let window = match h.window_parsed() {
        Some(w) => w.to_string(),
        None => format!("{} (unknown to this CLI build)", safe(&h.window)),
    };
    let mut out = format!(
        "{:<16}{}\n{:<16}{} ms\n\n",
        "window", window, "cadence", h.cadence_ms
    );
    if h.points.is_empty() {
        out.push_str("No points in this window.");
        return out;
    }
    out.push_str(&format!(
        "{:<22}  {:>16}  {:>16}  {:>18}\n",
        "TIME (UTC)", "EQUITY", "PNL", "VOLUME"
    ));
    for p in &h.points {
        out.push_str(&format!(
            "{:<22}  {:>16}  {:>16}  {:>18}\n",
            ms_to_iso8601(p.timestamp_ms),
            p.equity,
            p.pnl,
            p.volume,
        ));
    }
    out.push_str(&format!("\n{} point(s), oldest first.", h.points.len()));
    out
}

pub fn portfolio_history_json(h: &PortfolioHistory) -> String {
    let points: Value = h
        .points
        .iter()
        .map(|p| {
            json!({
                "timestamp_ms": p.timestamp_ms,
                "datetime": ms_to_iso8601(p.timestamp_ms),
                "equity": p.equity.to_string(),
                "pnl": p.pnl.to_string(),
                "volume": p.volume.to_string(),
            })
        })
        .collect();
    pretty(&json!({
        // The served window, verbatim — including a value this build cannot name,
        // so a script sees exactly what the server sent.
        "window": h.window,
        "cadence_ms": h.cadence_ms,
        "points": points,
    }))
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

/// Render a `POST /orders/batch` result. The batch is non-atomic: each entry
/// independently reports a placed order or a per-order rejection, so we render
/// every entry in request order and lead with a placed/rejected tally so a
/// partial failure is obvious without scanning the whole list.
pub fn order_batch(results: &[OrderResult], note: &str) -> String {
    let placed = results.iter().filter(|r| r.succeeded()).count();
    let rejected = results.len() - placed;
    let mut out = format!("{note}\n{placed} placed, {rejected} rejected.\n");
    for (i, r) in results.iter().enumerate() {
        match r {
            OrderResult::Placed { order: o, fills } => {
                out.push_str(&format!("\n[{i}] OK\n{}", order(o)));
                out.push_str(&format!("\n{:<16}{}\n", "immediate fills", fills.len()));
            }
            OrderResult::Rejected { error, message } => {
                out.push_str(&format!("\n[{i}] REJECTED  {error}: {message}\n"));
            }
        }
    }
    out
}

/// Render a batch result as JSON, preserving the wire's `outcome` tag so a
/// rejected entry stays distinguishable from a placed one.
pub fn order_batch_json(results: &[OrderResult]) -> String {
    let value: Value = results
        .iter()
        .map(|r| match r {
            OrderResult::Placed { order: o, fills } => json!({
                "outcome": "ok",
                "order": order_value(o),
                "fills": fills,
            }),
            OrderResult::Rejected { error, message } => json!({
                "outcome": "err",
                "error": error,
                "message": message,
            }),
        })
        .collect();
    pretty(&value)
}

/// Render a cancel response. The exact body shape isn't fixed by the spec, so
/// we pretty-print whatever the server returned (and a short human note).
pub fn cancel(value: &Value, human_note: &str) -> String {
    format!("{human_note}\n{}", pretty(value))
}

// ───────────────────────── market summaries ─────────────────────────

/// Render per-market summaries (24h volume, halt state) as an aligned table.
pub fn market_summaries(ss: &[MarketSummary]) -> String {
    if ss.is_empty() {
        return "No market summaries returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:>14}  {:>16}  {:>10}  {:<10}  {:>9}\n",
        "MARKET", "LAST PRICE", "VOLUME 24H", "TRADES", "STATUS", "ADL EVTS"
    );
    for s in ss {
        out.push_str(&format!(
            "{:<16}  {:>14}  {:>16}  {:>10}  {:<10}  {:>9}\n",
            s.market_id,
            opt(&s.last_trade_price),
            s.volume_24h,
            s.trade_count,
            s.status,
            s.adl_event_count,
        ));
    }
    out.push_str(&format!("\n{} market(s).", ss.len()));
    out
}

/// Render per-market summaries as pretty JSON.
pub fn market_summaries_json(ss: &[MarketSummary]) -> String {
    let value: Value = ss
        .iter()
        .map(|s| {
            json!({
                "market_id": s.market_id,
                "last_trade_price": opt_json(&s.last_trade_price),
                "volume_24h": s.volume_24h.to_string(),
                "trade_count": s.trade_count,
                "status": s.status,
                "halt_reason": s.halt_reason,
                "halted_at": opt_ms_iso_json(&s.halted_at),
                "adl_event_count": s.adl_event_count,
            })
        })
        .collect();
    pretty(&value)
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
        "MARKET", "LAST", "VOLUME 24H", "TRADES", "STATUS"
    );
    for s in ss {
        out.push_str(&format!(
            "{:<16}  {:>14}  {:>16}  {:>10}  {:<8}\n",
            s.market_id,
            opt(&s.last_trade_price),
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
                "last_trade_price": opt_json(&s.last_trade_price),
                "volume_24h": s.volume_24h.to_string(),
                "trade_count": s.trade_count,
                "status": s.status,
                "halt_reason": s.halt_reason,
                "halted_at": opt_ms_iso_json(&s.halted_at),
                "adl_event_count": s.adl_event_count,
            })
        })
        .collect();
    pretty(&value)
}

// ───────────────────────── mark price / market status ─────────────────────────

/// Render a market's mark price as key/value lines.
pub fn mark_price(m: &MarkPrice) -> String {
    format!(
        "{:<14}{}\n{:<14}{}",
        "market", m.market_id, "mark price", m.mark_price
    )
}

/// Render a market's mark price as pretty JSON.
pub fn mark_price_json(m: &MarkPrice) -> String {
    pretty(&json!({
        "market_id": m.market_id,
        "mark_price": m.mark_price.to_string(),
    }))
}

/// Render a single market's lifecycle/halt status as key/value lines.
pub fn market_status(s: &MarketStatus) -> String {
    let rows = [
        ("market", s.market_id.clone()),
        ("status", s.status.clone()),
        (
            "halt reason",
            s.halt_reason.clone().unwrap_or_else(|| "-".into()),
        ),
        ("halted at", opt_ms_iso(&s.halted_at)),
        ("adl events", s.adl_event_count.to_string()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a single market's status as pretty JSON.
pub fn market_status_json(s: &MarketStatus) -> String {
    pretty(&json!({
        "market_id": s.market_id,
        "status": s.status,
        "halt_reason": s.halt_reason,
        "halted_at": opt_ms_iso_json(&s.halted_at),
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

// The `funding_payments` / `funding_payments_json` renderers were removed with
// the `funding-payments` command in ENG-12369: GET /funding-payments is in no
// spec version (ENG-3817), so nothing renders `FundingPayment` any more. The
// funding-RATE renderers above are contracted and stay.

// ───────────────────────── ADL events ─────────────────────────

/// Render ADL settlement events (market or account scope) as an aligned table.
/// The per-counterparty closures are summarized as a count here; the JSON
/// renderer carries them in full.
pub fn adl_events(es: &[AdlEvent]) -> String {
    if es.is_empty() {
        return "No ADL events returned.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:<42}  {:>14}  {:>14}  {:>8}  {:>10}  {:<16}\n",
        "MARKET", "TARGET", "BANKRUPTCY PX", "BAD DEBT", "CLOSURES", "SEQ", "TIME(ms)"
    );
    for e in es {
        out.push_str(&format!(
            "{:<16}  {:<42}  {:>14}  {:>14}  {:>8}  {:>10}  {:<16}\n",
            e.market_id,
            e.target_account,
            e.bankruptcy_price,
            e.bad_debt_absorbed_by_fund,
            e.counterparty_closures.len(),
            e.sequence,
            e.timestamp,
        ));
    }
    out.push_str(&format!("\n{} event(s).", es.len()));
    out
}

pub fn adl_events_json(es: &[AdlEvent]) -> String {
    let value: Value = es
        .iter()
        .map(|e| {
            json!({
                "market_id": e.market_id,
                "target_account": e.target_account,
                "bankruptcy_price": e.bankruptcy_price.to_string(),
                "bad_debt_absorbed_by_fund": e.bad_debt_absorbed_by_fund.to_string(),
                "counterparty_closures": e
                    .counterparty_closures
                    .iter()
                    .map(|c| {
                        json!({
                            "account_id": c.account_id,
                            "position_closed": c.position_closed.to_string(),
                            "settlement_amount": c.settlement_amount.to_string(),
                        })
                    })
                    .collect::<Value>(),
                "sequence": e.sequence,
                "timestamp": e.timestamp,
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

// ───────────────────────── wallet auth: login / register ─────────────────────────

/// Render a successful EIP-191 sign-in. The session token is shown so the user
/// can copy it, but it has also been persisted to the config file; `token` is
/// passed in by the caller (which exposes it from the `SecretString`) — this
/// module never holds it. `path` is where the token was saved.
pub fn login(address: &str, token: &str, path: &str) -> String {
    format!(
        "Signed in. Session token saved to {} (permissions 0600).

         {:<14}{}
{:<14}{}",
        path, "address", address, "token", token,
    )
}

pub fn login_json(address: &str, token: &str, path: &str) -> String {
    pretty(&json!({
        "address": address,
        "token": token,
        "saved_to": path,
    }))
}

/// Render a successful EIP-712 agent registration.
pub fn agent_registered(agent_address: &str, expires_at: u64) -> String {
    format!(
        "Registered agent.

{:<16}{}
{:<16}{}",
        "agent", agent_address, "expires(ms)", expires_at,
    )
}

pub fn agent_registered_json(agent_address: &str, expires_at: u64) -> String {
    pretty(&json!({
        "agent_address": agent_address,
        "expires_at": expires_at,
    }))
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

// The `leverage` / `leverage_json` renderers were removed with the
// `account leverage` subcommand in ENG-12369 — nothing rendered `LeverageUpdate`
// once the command went away. POST /account/leverage is in no spec version and
// routes nowhere; ENG-7318 documents the served POST /leverage.

// The `margin_mode` / `margin_mode_json` renderers were removed with the
// `account margin-mode` subcommand in ENG-7740 — nothing rendered
// `MarginModeUpdate` once the command went away. ENG-7614 gates its return.

// ───────────────────────── withdrawals ─────────────────────────

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

// ───────────────────────── bridge ─────────────────────────
//
// Every string in these tables is server-chosen and free-form — a chain name, an
// asset symbol, a deposit status, an opaque deposit id — so all of them go
// through `safe()` before reaching a terminal. Timestamps are rendered ISO-8601
// UTC under their wire field names, matching `market_status`'s `halted_at`; the
// raw milliseconds are never the interesting value here.

/// Render the bridgeable assets per chain (`GET /api/v1/bridge/assets`).
///
/// Deposit and withdraw assets are listed in one table with a `DIRECTION`
/// column. Withdrawals are a later phase and no endpoint serves them yet, so a
/// `withdraw` row states an eventual capability, not something this CLI can do —
/// which the footer says in words rather than leaving the reader to infer.
pub fn bridge_assets(r: &BridgeAssetsResponse) -> String {
    if r.chains.is_empty() {
        return "No bridge chains.".to_string();
    }
    let mut out = format!(
        "{:<20}  {:<9}  {:<8}  {:>8}  {:>16}  {:>6}  {:>12}  {:<44}\n",
        "CHAIN", "DIRECTION", "ASSET", "DECIMALS", "MIN AMOUNT", "CONF", "FEE", "CONTRACT"
    );
    let mut assets = 0usize;
    let mut has_withdraw = false;
    for c in &r.chains {
        let chain = match c.chain_id {
            Some(id) => format!("{} ({id})", safe(&c.chain)),
            None => safe(&c.chain),
        };
        let rows = c
            .deposit_assets
            .iter()
            .map(|a| ("deposit", a))
            .chain(c.withdraw_assets.iter().map(|a| ("withdraw", a)));
        for (direction, a) in rows {
            has_withdraw |= direction == "withdraw";
            assets += 1;
            out.push_str(&format!(
                "{:<20}  {:<9}  {:<8}  {:>8}  {:>16}  {:>6}  {:>12}  {:<44}\n",
                chain,
                direction,
                safe(&a.symbol),
                a.decimals,
                a.min_amount,
                a.confirmations,
                opt(&a.fee),
                a.contract_address
                    .as_deref()
                    .map(safe)
                    .unwrap_or_else(|| "-".to_string()),
            ));
        }
    }
    out.push_str(&format!(
        "\n{} chain(s), {} asset(s).",
        r.chains.len(),
        assets
    ));
    if has_withdraw {
        out.push_str(
            "\n`withdraw` rows list the eventual capability; the bridge serves deposits only \
             today, and this CLI exposes no withdrawal command.",
        );
    }
    out
}

pub fn bridge_assets_json(r: &BridgeAssetsResponse) -> String {
    fn assets(list: &[BridgeAsset]) -> Value {
        list.iter()
            .map(|a| {
                json!({
                    "symbol": a.symbol,
                    "decimals": a.decimals,
                    "min_amount": a.min_amount.to_string(),
                    "confirmations": a.confirmations,
                    "fee": opt_json(&a.fee),
                    "contract_address": a.contract_address,
                })
            })
            .collect()
    }
    let chains: Value = r
        .chains
        .iter()
        .map(|c| {
            json!({
                "chain": c.chain,
                "chain_id": c.chain_id,
                "deposit_assets": assets(&c.deposit_assets),
                "withdraw_assets": assets(&c.withdraw_assets),
            })
        })
        .collect();
    pretty(&json!({ "chains": chains }))
}

/// Comma-joined accepted assets, or `-` when the server sent none.
fn accepts(list: &[String]) -> String {
    if list.is_empty() {
        return "-".to_string();
    }
    list.iter().map(|s| safe(s)).collect::<Vec<_>>().join(",")
}

fn deposit_address_value(a: &BridgeDepositAddress) -> Value {
    json!({
        "address": a.address,
        "chain": a.chain,
        "accepts": a.accepts,
        "account_id": a.account_id,
        "created_at": ms_to_iso8601(a.created_at),
    })
}

/// Render one deposit address (`POST /api/v1/bridge/deposit-addresses`), the
/// get-or-create result for a single chain.
pub fn bridge_deposit_address(a: &BridgeDepositAddress) -> String {
    let rows = [
        ("address", safe(&a.address)),
        ("chain", safe(&a.chain)),
        ("accepts", accepts(&a.accepts)),
        ("account id", safe(&a.account_id)),
        ("created", ms_to_iso8601(a.created_at)),
    ];
    let mut out = rows
        .iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(
        "\n\nSend only the accepted assets, on this chain, to this address. \
         Anything else is not credited.",
    );
    out
}

pub fn bridge_deposit_address_json(a: &BridgeDepositAddress) -> String {
    pretty(&deposit_address_value(a))
}

/// Render the account's deposit addresses
/// (`GET /api/v1/bridge/deposit-addresses`).
pub fn bridge_deposit_addresses(addrs: &[BridgeDepositAddress]) -> String {
    if addrs.is_empty() {
        return "No bridge deposit addresses. Create one with \
                `nexus bridge deposit-address --chain <CHAIN>`."
            .to_string();
    }
    let mut out = format!(
        "{:<44}  {:<16}  {:<16}  {:<22}\n",
        "ADDRESS", "CHAIN", "ACCEPTS", "CREATED (UTC)"
    );
    for a in addrs {
        out.push_str(&format!(
            "{:<44}  {:<16}  {:<16}  {:<22}\n",
            safe(&a.address),
            safe(&a.chain),
            accepts(&a.accepts),
            ms_to_iso8601(a.created_at),
        ));
    }
    out.push_str(&format!("\n{} address(es).", addrs.len()));
    out
}

pub fn bridge_deposit_addresses_json(addrs: &[BridgeDepositAddress]) -> String {
    let value: Value = addrs.iter().map(deposit_address_value).collect();
    pretty(&value)
}

/// Confirmations as `seen/required`, with `-` for either half the server has not
/// reported. A deposit that is not yet on chain has neither, and printing `0/0`
/// would read as "confirmed, nothing required".
fn confirmations(seen: &Option<u32>, required: &Option<u32>) -> String {
    format!("{}/{}", opt(seen), opt(required))
}

fn deposit_value(d: &BridgeDeposit) -> Value {
    json!({
        "id": d.id,
        "account_id": d.account_id,
        "chain": d.chain,
        "asset": d.asset,
        // Decimal as a string, like every other money field here, so no
        // precision is lost on the way to a script.
        "amount": d.amount.to_string(),
        "address": d.address,
        "status": d.status,
        "confirmations": d.confirmations,
        "required_confirmations": d.required_confirmations,
        "tx_hash": d.tx_hash,
        "created_at": ms_to_iso8601(d.created_at),
        "updated_at": ms_to_iso8601(d.updated_at),
        "credited_at": opt_ms_iso_json(&d.credited_at),
    })
}

/// Render the tracked cross-chain deposits (`GET /api/v1/bridge/deposits`).
///
/// `tx_hash` is deliberately not a column — it is 66 characters and would push
/// everything else off a normal terminal. `nexus bridge deposits --id <ID>` shows
/// it, and `--output json` always carries it.
pub fn bridge_deposits(ds: &[BridgeDeposit]) -> String {
    if ds.is_empty() {
        return "No bridge deposits.".to_string();
    }
    let mut out = format!(
        "{:<24}  {:<16}  {:<8}  {:>16}  {:<12}  {:>9}  {:<22}\n",
        "ID", "CHAIN", "ASSET", "AMOUNT", "STATUS", "CONF", "CREATED (UTC)"
    );
    for d in ds {
        out.push_str(&format!(
            "{:<24}  {:<16}  {:<8}  {:>16}  {:<12}  {:>9}  {:<22}\n",
            safe(&d.id),
            safe(&d.chain),
            safe(&d.asset),
            d.amount,
            safe(&d.status),
            confirmations(&d.confirmations, &d.required_confirmations),
            ms_to_iso8601(d.created_at),
        ));
    }
    out.push_str(&format!("\n{} deposit(s).", ds.len()));
    out
}

pub fn bridge_deposits_json(ds: &[BridgeDeposit]) -> String {
    let value: Value = ds.iter().map(deposit_value).collect();
    pretty(&value)
}

/// Render a single tracked deposit (`GET /api/v1/bridge/deposits/{id}`).
pub fn bridge_deposit(d: &BridgeDeposit) -> String {
    let rows = [
        ("id", safe(&d.id)),
        ("status", safe(&d.status)),
        ("chain", safe(&d.chain)),
        ("asset", safe(&d.asset)),
        ("amount", d.amount.to_string()),
        (
            "confirmations",
            confirmations(&d.confirmations, &d.required_confirmations),
        ),
        ("address", safe(&d.address)),
        ("account id", safe(&d.account_id)),
        (
            "tx hash",
            d.tx_hash
                .as_deref()
                .map(safe)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", ms_to_iso8601(d.created_at)),
        ("updated", ms_to_iso8601(d.updated_at)),
        ("credited", opt_ms_iso(&d.credited_at)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<16}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn bridge_deposit_json(d: &BridgeDeposit) -> String {
    pretty(&deposit_value(d))
}

// The transfer and sub-account renderers were removed with the `transfers` and
// `sub-accounts` commands in ENG-12369 (closing ENG-8123): both route groups 404
// on the live venue where documented routes 401, and neither has ever been in the
// spec (ENG-7800). `Transfer` and `SubAccount` no longer reach this module.

// ── venue stats ──────────────────────────────────────────────────────────────

pub fn stats(s: &StatsSnapshot) -> String {
    let mut out = String::from("Venue activity\n");
    out.push_str(&format!("  health:             {}\n", safe(&s.health)));
    out.push_str(&format!("  ingest connected:   {}\n", s.connected));
    out.push_str(&format!("  events received:    {}\n", s.events_received));
    out.push_str(&format!("  events/sec:         {:.2}\n", s.events_per_sec));
    out.push_str(&format!("  fills total:        {}\n", s.fills_total));
    out.push_str(&format!("  liquidations total: {}\n", s.liquidations_total));
    out.push_str(&format!(
        "  highest sequence:   {}\n",
        s.highest_sequence_seen
    ));
    // A non-zero gap count means the indexer missed events, so every counter
    // above is a lower bound. Say so rather than printing a bare number.
    out.push_str(&format!(
        "  sequence gaps:      {}{}\n",
        s.gap_count,
        if s.gap_count > 0 {
            "  (counters above are lower bounds)"
        } else {
            ""
        }
    ));
    out.push_str(&format!("  uptime (s):         {}\n", s.uptime_seconds));
    out.push_str(&format!(
        "  last event:         {}\n",
        opt_ms_iso(&s.last_event_ms)
    ));
    out.push_str(&format!(
        "  unique traders 24h: {}\n",
        opt(&s.unique_traders_24h)
    ));
    out.push_str(&format!(
        "  unique traders 7d:  {}\n",
        opt(&s.unique_traders_7d)
    ));
    out.push_str(&format!(
        "  unique traders 30d: {}",
        opt(&s.unique_traders_30d)
    ));
    out
}

pub fn stats_json(s: &StatsSnapshot) -> String {
    pretty(&json!({
        "health": s.health,
        "connected": s.connected,
        "events_received": s.events_received,
        "events_per_sec": s.events_per_sec,
        "fills_total": s.fills_total,
        "liquidations_total": s.liquidations_total,
        "highest_sequence_seen": s.highest_sequence_seen,
        "gap_count": s.gap_count,
        "uptime_seconds": s.uptime_seconds,
        "last_event_ms": opt_json(&s.last_event_ms),
        "last_event": opt_ms_iso_json(&s.last_event_ms),
        "unique_traders_24h": opt_json(&s.unique_traders_24h),
        "unique_traders_7d": opt_json(&s.unique_traders_7d),
        "unique_traders_30d": opt_json(&s.unique_traders_30d),
    }))
}

pub fn stats_history(samples: &[ThroughputSample]) -> String {
    if samples.is_empty() {
        return "No throughput samples.".to_string();
    }
    let mut out = format!("{:<26}  {:>12}\n", "TIME", "FILLS");
    for s in samples {
        out.push_str(&format!(
            "{:<26}  {:>12}\n",
            ms_to_iso8601(s.timestamp),
            s.fills
        ));
    }
    out.push_str(&format!("\n{} sample(s).", samples.len()));
    out
}

pub fn stats_history_json(samples: &[ThroughputSample]) -> String {
    let value: Value = samples
        .iter()
        .map(|s| {
            json!({
                "timestamp": s.timestamp,
                "time": ms_to_iso8601(s.timestamp),
                "fills": s.fills,
            })
        })
        .collect();
    pretty(&value)
}

// ── market risk parameters ───────────────────────────────────────────────────

pub fn market_risk_params(p: &MarketRiskParams) -> String {
    // Rates arrive as fractions; show the percentage too, since margin is read
    // in percent far more often than in basis fractions.
    let pct = |d: Decimal| d * Decimal::from(100);
    format!(
        "Risk parameters for {}\n  max leverage:      {}x\n  initial margin:    {} ({}%)\n  maintenance margin: {} ({}%)",
        safe(&p.market_id),
        p.max_leverage,
        p.initial_margin_rate,
        pct(p.initial_margin_rate).normalize(),
        p.maintenance_margin_rate,
        pct(p.maintenance_margin_rate).normalize(),
    )
}

pub fn market_risk_params_json(p: &MarketRiskParams) -> String {
    pretty(&json!({
        "market_id": p.market_id,
        "max_leverage": p.max_leverage,
        "initial_margin_rate": p.initial_margin_rate.to_string(),
        "maintenance_margin_rate": p.maintenance_margin_rate.to_string(),
    }))
}

// ── funding premium samples ──────────────────────────────────────────────────

pub fn funding_samples(samples: &[FundingPremiumSample]) -> String {
    if samples.is_empty() {
        return "No funding premium samples.".to_string();
    }
    let mut out = format!("{:<26}  {:>20}\n", "TIME", "PREMIUM INDEX");
    for s in samples {
        out.push_str(&format!(
            "{:<26}  {:>20}\n",
            ms_to_iso8601(s.timestamp),
            s.premium_index
        ));
    }
    out.push_str(&format!("\n{} sample(s).", samples.len()));
    out
}

pub fn funding_samples_json(samples: &[FundingPremiumSample]) -> String {
    let value: Value = samples
        .iter()
        .map(|s| {
            json!({
                "timestamp": s.timestamp,
                "time": ms_to_iso8601(s.timestamp),
                "premium_index": s.premium_index.to_string(),
            })
        })
        .collect();
    pretty(&value)
}

// ── account equity history ───────────────────────────────────────────────────

pub fn equity_history(points: &[EquityPoint]) -> String {
    if points.is_empty() {
        return "No equity history.".to_string();
    }
    let mut out = format!("{:<26}  {:>20}\n", "TIME", "EQUITY");
    for p in points {
        out.push_str(&format!(
            "{:<26}  {:>20}\n",
            opt_ms_iso(&p.timestamp_ms),
            opt(&p.equity)
        ));
    }
    out.push_str(&format!("\n{} point(s).", points.len()));
    out
}

pub fn equity_history_json(points: &[EquityPoint]) -> String {
    let value: Value = points
        .iter()
        .map(|p| {
            json!({
                "timestamp_ms": opt_json(&p.timestamp_ms),
                "time": opt_ms_iso_json(&p.timestamp_ms),
                "equity": opt_json(&p.equity),
            })
        })
        .collect();
    pretty(&value)
}

// ── order history ────────────────────────────────────────────────────────────

pub fn order_history(entries: &[OrderHistoryEntry]) -> String {
    if entries.is_empty() {
        return "No order history.".to_string();
    }
    let mut out = format!(
        "{:<24}  {:<16}  {:<5}  {:<10}  {:>14}  {:>14}  {:<12}  {:<20}\n",
        "ID", "MARKET", "SIDE", "TYPE", "PRICE", "FILLED/SIZE", "STATUS", "CANCEL-REASON"
    );
    for e in entries {
        let filled = format!("{}/{}", opt(&e.filled_qty), opt(&e.size));
        out.push_str(&format!(
            "{:<24}  {:<16}  {:<5}  {:<10}  {:>14}  {:>14}  {:<12}  {:<20}\n",
            opt(&e.id),
            opt(&e.market_id),
            e.side.map(side_str).unwrap_or("-"),
            opt(&e.order_type),
            opt(&e.price),
            filled,
            opt(&e.status),
            opt(&e.cancellation_reason),
        ));
    }
    out.push_str(&format!("\n{} order(s).", entries.len()));
    out
}

pub fn order_history_json(entries: &[OrderHistoryEntry]) -> String {
    let value: Value = entries
        .iter()
        .map(|e| {
            json!({
                "id": opt_json(&e.id),
                "market_id": opt_json(&e.market_id),
                "side": e.side.map(side_str),
                "order_type": opt_json(&e.order_type),
                "price": opt_json(&e.price),
                "size": opt_json(&e.size),
                "filled_qty": opt_json(&e.filled_qty),
                "status": opt_json(&e.status),
                "cancellation_reason": opt_json(&e.cancellation_reason),
                "created_at_ms": opt_json(&e.created_at_ms),
                "completed_at_ms": opt_json(&e.completed_at_ms),
            })
        })
        .collect();
    pretty(&value)
}

// ── closed positions ─────────────────────────────────────────────────────────

pub fn closed_positions(ps: &[ClosedPosition]) -> String {
    if ps.is_empty() {
        return "No closed positions.".to_string();
    }
    let mut out = format!(
        "{:<16}  {:<5}  {:>14}  {:>14}  {:>14}  {:>16}  {:<26}\n",
        "MARKET", "SIDE", "SIZE", "ENTRY", "EXIT", "REALIZED PNL", "CLOSED"
    );
    for p in ps {
        out.push_str(&format!(
            "{:<16}  {:<5}  {:>14}  {:>14}  {:>14}  {:>16}  {:<26}\n",
            opt(&p.market_id),
            opt(&p.side),
            opt(&p.size),
            opt(&p.entry_price),
            opt(&p.exit_price),
            opt(&p.realized_pnl),
            opt_ms_iso(&p.closed_at_ms),
        ));
    }
    out.push_str(&format!("\n{} closed position(s).", ps.len()));
    out
}

pub fn closed_positions_json(ps: &[ClosedPosition]) -> String {
    let value: Value = ps
        .iter()
        .map(|p| {
            json!({
                "market_id": opt_json(&p.market_id),
                "side": opt_json(&p.side),
                "size": opt_json(&p.size),
                "entry_price": opt_json(&p.entry_price),
                "exit_price": opt_json(&p.exit_price),
                "realized_pnl": opt_json(&p.realized_pnl),
                "closed_at_ms": opt_json(&p.closed_at_ms),
                "closed_at": opt_ms_iso_json(&p.closed_at_ms),
            })
        })
        .collect();
    pretty(&value)
}

// ── account funding payments ─────────────────────────────────────────────────

/// Render a funding direction for display.
fn funding_direction_str(d: FundingDirection) -> &'static str {
    match d {
        FundingDirection::Paid => "Paid",
        FundingDirection::Received => "Received",
    }
}

pub fn account_funding(entries: &[AccountFunding]) -> String {
    if entries.is_empty() {
        return "No funding payments.".to_string();
    }
    let mut out = format!(
        "{:<26}  {:<16}  {:<9}  {:>16}  {:>14}  {:>16}\n",
        "TIME", "MARKET", "DIRECTION", "AMOUNT", "RATE", "POSITION SIZE"
    );
    for e in entries {
        out.push_str(&format!(
            "{:<26}  {:<16}  {:<9}  {:>16}  {:>14}  {:>16}\n",
            ms_to_iso8601(e.timestamp),
            safe(&e.market_id),
            funding_direction_str(e.direction),
            e.amount,
            e.funding_rate,
            e.position_size,
        ));
    }
    out.push_str(&format!("\n{} payment(s).", entries.len()));
    out
}

pub fn account_funding_json(entries: &[AccountFunding]) -> String {
    let value: Value = entries
        .iter()
        .map(|e| {
            json!({
                "timestamp": e.timestamp,
                "time": ms_to_iso8601(e.timestamp),
                "market_id": e.market_id,
                "direction": funding_direction_str(e.direction),
                "amount": e.amount.to_string(),
                "funding_rate": e.funding_rate.to_string(),
                "position_size": e.position_size.to_string(),
            })
        })
        .collect();
    pretty(&value)
}

// ── deposits ledger ──────────────────────────────────────────────────────────

/// Render a funds-movement kind for display.
fn funds_kind_str(k: FundsKind) -> &'static str {
    match k {
        FundsKind::Deposit => "Deposit",
        FundsKind::Withdrawal => "Withdrawal",
        FundsKind::Faucet => "Faucet",
    }
}

/// Render a funds-movement status for display.
fn funds_status_str(s: FundsStatus) -> &'static str {
    match s {
        FundsStatus::Pending => "Pending",
        FundsStatus::Confirmed => "Confirmed",
        FundsStatus::Failed => "Failed",
    }
}

pub fn funds_entries(entries: &[FundsEntry]) -> String {
    if entries.is_empty() {
        return "No deposits.".to_string();
    }
    // KIND is shown, not assumed. `GET /deposits` shares one row type across
    // deposits, withdrawals and faucet grants, so labelling every row a deposit
    // would misreport the other two (the SDK's own type doc warns about this).
    let mut out = format!(
        "{:<26}  {:<10}  {:<10}  {:>18}  {:<8}  {:<12}  {:<20}\n",
        "TIME", "KIND", "STATUS", "AMOUNT", "ASSET", "ID", "TX"
    );
    for e in entries {
        out.push_str(&format!(
            "{:<26}  {:<10}  {:<10}  {:>18}  {:<8}  {:<12}  {:<20}\n",
            ms_to_iso8601(e.timestamp),
            funds_kind_str(e.kind),
            funds_status_str(e.status),
            e.amount,
            safe(&e.asset),
            e.id,
            e.tx_hash
                .as_deref()
                .map(safe)
                .unwrap_or_else(|| "-".to_string()),
        ));
    }
    let deposits = entries
        .iter()
        .filter(|e| matches!(e.kind, FundsKind::Deposit))
        .count();
    out.push_str(&format!(
        "\n{} row(s), {} of them deposits.",
        entries.len(),
        deposits
    ));
    out
}

pub fn funds_entries_json(entries: &[FundsEntry]) -> String {
    let value: Value = entries
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "kind": funds_kind_str(e.kind),
                "status": funds_status_str(e.status),
                "account": e.account,
                "amount": e.amount.to_string(),
                "asset": e.asset,
                "timestamp": e.timestamp,
                "time": ms_to_iso8601(e.timestamp),
                "tx_hash": opt_json(&e.tx_hash),
            })
        })
        .collect();
    pretty(&value)
}

// ── cancel-on-disconnect ─────────────────────────────────────────────────────

pub fn cancel_on_disconnect(s: &CancelOnDisconnectStatus) -> String {
    let mut out = String::from("Cancel-on-disconnect\n");
    out.push_str(&format!("  enabled:   {}\n", s.enabled));
    out.push_str(&format!("  active:    {}\n", s.active));
    out.push_str(&format!("  grace (s): {}", opt(&s.grace_secs)));
    // The two flags disagreeing is the interesting case and the reason both are
    // reported: your setting is on, the venue is not honouring it.
    if s.enabled && !s.active {
        out.push_str(
            "\n\nEnabled but not active: the exchange has cancel-on-disconnect\n\
             switched off deployment-wide, so orders will NOT be pulled if you\n\
             disconnect.",
        );
    }
    out
}

pub fn cancel_on_disconnect_json(s: &CancelOnDisconnectStatus) -> String {
    pretty(&json!({
        "enabled": s.enabled,
        "active": s.active,
        "grace_secs": opt_json(&s.grace_secs),
    }))
}

/// Render the result of a spec'd deposit (`POST /deposits`).
///
/// The server answers with the authoritative post-deposit balance, so that is
/// what is shown — it is a balance, not the amount that was credited.
pub fn deposit_created(d: &DepositResponse) -> String {
    format!("Deposit accepted. Balance is now {}.", d.balance)
}

pub fn deposit_created_json(d: &DepositResponse) -> String {
    pretty(&json!({ "balance": d.balance.to_string() }))
}

/// Render a faucet claim (`POST /faucet`).
pub fn faucet(f: &FaucetResponse) -> String {
    format!(
        "Claimed {} from the faucet.\nNext claim available at {} (server-reported; a claim inside the cooldown answers 429).",
        f.amount,
        ms_to_iso8601(f.available_at_ms),
    )
}

pub fn faucet_json(f: &FaucetResponse) -> String {
    pretty(&json!({
        "amount": f.amount.to_string(),
        "available_at_ms": f.available_at_ms,
        "available_at": ms_to_iso8601(f.available_at_ms),
    }))
}

/// Render an isolated-margin adjustment (`POST /account/margin`).
pub fn margin_adjustment(m: &MarginAdjustment) -> String {
    let rows = [
        ("market", safe(&m.market_id)),
        ("allocated margin", m.allocated_margin.to_string()),
        ("collateral", m.collateral.to_string()),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<20}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn margin_adjustment_json(m: &MarginAdjustment) -> String {
    pretty(&json!({
        "market_id": m.market_id,
        "allocated_margin": m.allocated_margin.to_string(),
        "collateral": m.collateral.to_string(),
    }))
}

/// Render an order preview (`POST /api/v1/orders/preview`).
///
/// A preview that the engine would reject is not an error — the request
/// succeeded and the answer is "no". It is rendered as a refusal with the
/// server's reason so the two outcomes cannot be confused. An unreported
/// verdict is not "accepted" either: the SDK's own `is_accepted` treats `None`
/// as not vouched for, and so does this.
pub fn order_preview(p: &OrderPreview) -> String {
    let verdict = match p.accepted {
        Some(true) => "ACCEPTED (preview only — nothing was placed)".to_string(),
        Some(false) => format!("REJECTED — {}", safe(&opt(&p.reject_reason))),
        None => "verdict not reported by the server (treat as NOT accepted)".to_string(),
    };
    let rows = [
        ("required margin", opt(&p.required_initial_margin)),
        ("post-trade equity", opt(&p.projected_post_trade_equity)),
        (
            "post-trade liq. price",
            opt(&p.projected_post_trade_liquidation_price),
        ),
        ("post-trade leverage", opt(&p.projected_post_trade_leverage)),
        ("expected fill VWAP", opt(&p.expected_fill_vwap)),
        ("projected fees", opt(&p.projected_fees)),
    ];
    let body = rows
        .iter()
        .map(|(k, v)| format!("{k:<24}{v}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{verdict}\n\n{body}")
}

pub fn order_preview_json(p: &OrderPreview) -> String {
    pretty(&json!({
        "accepted": p.accepted,
        "reject_reason": p.reject_reason,
        "required_initial_margin": opt_json(&p.required_initial_margin),
        "projected_post_trade_equity": opt_json(&p.projected_post_trade_equity),
        "projected_post_trade_liquidation_price": opt_json(&p.projected_post_trade_liquidation_price),
        "projected_post_trade_leverage": opt_json(&p.projected_post_trade_leverage),
        "expected_fill_vwap": opt_json(&p.expected_fill_vwap),
        "projected_fees": opt_json(&p.projected_fees),
    }))
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
    fn adl_events_render_table_and_json() {
        let events: Vec<AdlEvent> = serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP",
            "target_account": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
            "bankruptcy_price": "42000.5",
            "bad_debt_absorbed_by_fund": "1250",
            "counterparty_closures": [{
                "account_id": "0x1234567890abcdef1234567890abcdef12345678",
                "position_closed": "0.25",
                "settlement_amount": "10500.125"
            }],
            "sequence": 991,
            "timestamp": 1_776_033_900_000i64
        }]))
        .unwrap();

        // Human table: header, the row, and the count footer.
        let human = adl_events(&events);
        assert!(human.contains("MARKET"), "{human}");
        assert!(human.contains("BANKRUPTCY PX"), "{human}");
        assert!(human.contains("BTC-USDX-PERP"), "{human}");
        assert!(human.contains("42000.5"), "{human}");
        assert!(human.contains("1 event(s)."), "{human}");
        // Closures are summarized as a count in the table…
        assert!(
            !human.contains("0x1234567890abcdef1234567890abcdef12345678"),
            "table should summarize closures, not inline them: {human}"
        );

        // …and carried in full in JSON, with money as decimal strings.
        let v: Value = serde_json::from_str(&adl_events_json(&events)).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(
            keys(row),
            [
                "bad_debt_absorbed_by_fund",
                "bankruptcy_price",
                "counterparty_closures",
                "market_id",
                "sequence",
                "target_account",
                "timestamp",
            ]
        );
        assert_eq!(row["bankruptcy_price"], json!("42000.5"));
        assert_eq!(row["sequence"], json!(991));
        let closure = &row["counterparty_closures"].as_array().unwrap()[0];
        assert_eq!(closure["position_closed"], json!("0.25"));
        assert_eq!(closure["settlement_amount"], json!("10500.125"));

        // The empty case renders a note, not an empty table.
        assert_eq!(adl_events(&[]), "No ADL events returned.");
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
    fn health_json_shape_and_defaults() {
        // A full `GET /status` snapshot round-trips its three fields verbatim.
        let health: HealthStatus = serde_json::from_value(json!({
            "status": "degraded",
            "timestamp_ms": 1776033900000i64,
            "services": {"indexer": "ok", "engine": "degraded"}
        }))
        .unwrap();

        let v: Value = serde_json::from_str(&health_json(&health)).unwrap();
        assert_eq!(keys(&v), ["services", "status", "timestamp_ms"]);
        assert_eq!(v["status"], json!("degraded"));
        assert_eq!(v["timestamp_ms"], json!(1776033900000i64));
        assert_eq!(v["services"]["engine"], json!("degraded"));

        // Every field defaults (serde `default`) when the server omits it, so a
        // slim payload still decodes and renders rather than erroring.
        let empty: HealthStatus = serde_json::from_value(json!({})).unwrap();
        let v: Value = serde_json::from_str(&health_json(&empty)).unwrap();
        assert_eq!(v["status"], json!(""));
        assert_eq!(v["timestamp_ms"], json!(0));
        assert_eq!(v["services"], Value::Null);
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

    #[test]
    fn market_summaries_json_uses_decimal_strings_and_null_last_trade() {
        // `last_trade_price` and `volume_24h` arrive as JSON numbers via the
        // SDK's float adapter; a market with no trades sends `null` for the price.
        let summaries: Vec<MarketSummary> = serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP",
            "last_trade_price": null,
            "volume_24h": 1234.5,
            "trade_count": 42,
            "status": "halted",
            "halt_reason": "maintenance",
            "halted_at": 1_700_000_000_000i64,
            "adl_event_count": 3
        }]))
        .unwrap();
        let v: Value = serde_json::from_str(&market_summaries_json(&summaries)).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(
            keys(row),
            [
                "adl_event_count",
                "halt_reason",
                "halted_at",
                "last_trade_price",
                "market_id",
                "status",
                "trade_count",
                "volume_24h",
            ]
        );
        // Money is a decimal string; an absent price is JSON null; counts stay numbers.
        assert_eq!(row["last_trade_price"], Value::Null);
        assert_eq!(row["volume_24h"], json!("1234.5"));
        assert_eq!(row["trade_count"], json!(42));
        assert_eq!(row["status"], json!("halted"));
        // `halted_at` is rendered as an ISO-8601 string (not raw unix-ms) to
        // match the `datetime` fields used elsewhere.
        assert_eq!(row["halted_at"], json!("2023-11-14T22:13:20Z"));
    }

    #[test]
    fn ms_to_iso8601_formats_unix_millis_as_utc() {
        assert_eq!(ms_to_iso8601(1_700_000_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(ms_to_iso8601(0), "1970-01-01T00:00:00Z");
        // Leap-year day (2024-02-29).
        assert_eq!(ms_to_iso8601(1_709_208_000_000), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn mark_price_json_is_a_decimal_string() {
        // The mark-price endpoint sends the price as a decimal string.
        let mp: MarkPrice = serde_json::from_value(json!({
            "market_id": "BTC-USDX-PERP",
            "mark_price": "84000.5"
        }))
        .unwrap();
        let v: Value = serde_json::from_str(&mark_price_json(&mp)).unwrap();
        assert_eq!(keys(&v), ["mark_price", "market_id"]);
        assert_eq!(v["mark_price"], json!("84000.5"));
    }

    #[test]
    fn market_status_json_preserves_optional_halt_fields() {
        // An active market reports no halt reason / time.
        let status: MarketStatus = serde_json::from_value(json!({
            "market_id": "BTC-USDX-PERP",
            "status": "active",
            "halt_reason": null,
            "halted_at": null,
            "adl_event_count": 0
        }))
        .unwrap();
        let v: Value = serde_json::from_str(&market_status_json(&status)).unwrap();
        assert_eq!(
            keys(&v),
            [
                "adl_event_count",
                "halt_reason",
                "halted_at",
                "market_id",
                "status",
            ]
        );
        assert_eq!(v["halt_reason"], Value::Null);
        assert_eq!(v["halted_at"], Value::Null);
        assert_eq!(v["status"], json!("active"));
    }

    // ───────────────────────── fixtures ─────────────────────────

    fn market_fixture() -> Vec<Market> {
        serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP", "base_asset": "BTC", "quote_asset": "USDX",
            "tick_size": "0.5", "lot_size": "0.001", "min_order_size": "0.001",
            "max_order_size": "100", "initial_margin_rate": "0.05",
            "maintenance_margin_rate": "0.03", "max_leverage": 20
        }]))
        .unwrap()
    }

    fn orderbook_fixture() -> OrderBook {
        serde_json::from_value(json!({
            "symbol": "BTC-USDX-PERP",
            "bids": [[84000.0, 1.5], [83999.5, 2.0]],
            "asks": [[84001.0, 0.5]],
            "timestamp": 1_700_000_000_000i64,
            "datetime": "2023-11-14T22:13:20Z",
            "nonce": 99
        }))
        .unwrap()
    }

    fn trades_fixture() -> Vec<Trade> {
        serde_json::from_value(json!([{
            "id": "t1", "symbol": "BTC-USDX-PERP", "side": "buy",
            "price": 84000.0, "amount": 0.01, "cost": 840.0,
            "timestamp": 1_700_000_000_000i64, "datetime": "2023-11-14T22:13:20Z",
            "is_liquidation": false
        }]))
        .unwrap()
    }

    fn account_fixture() -> AccountSummary {
        serde_json::from_value(json!({
            "balance": "1000", "collateral": "1000", "equity": "1050",
            "available_margin": "900",
            "positions": [{
                "market_id": "BTC-USDX-PERP", "side": "Buy", "size": "0.5",
                "entry_price": "80000", "unrealized_pnl": "50",
                "realized_pnl": "0", "liquidation_price": "60000"
            }]
        }))
        .unwrap()
    }

    fn fills_fixture() -> Vec<Fill> {
        serde_json::from_value(json!([{
            "id": "f1", "order_id": "o1", "market_id": "BTC-USDX-PERP",
            "side": "sell", "price": "84000", "size": "0.01", "fee": "0.42",
            "taker_or_maker": "taker", "timestamp": 1_700_000_000_000i64,
            "is_liquidation": false
        }]))
        .unwrap()
    }

    fn order_fixture() -> Order {
        serde_json::from_value(json!({
            "id": "o1", "market_id": "BTC-USDX-PERP", "account_id": "0xabc",
            "side": "Buy", "order_type": "Limit", "price": "84000",
            "quantity": "0.01", "filled_qty": "0", "status": "Open",
            "time_in_force": "GTC"
        }))
        .unwrap()
    }

    // ───────────────────────── human renderers ─────────────────────────

    #[test]
    fn human_renderers_include_headers_and_counts() {
        let m = markets(&market_fixture());
        assert!(m.contains("MARKET") && m.contains("BTC-USDX-PERP"));
        assert!(m.contains("1 market(s)."));

        let ob = orderbook(&orderbook_fixture());
        assert!(ob.contains("order book") && ob.contains("BID PRICE"));
        assert!(ob.contains("2 bid level(s), 1 ask level(s)."));

        let tr = trades(&trades_fixture());
        assert!(tr.contains("SIDE") && tr.contains("Buy"));
        assert!(tr.contains("1 trade(s)."));

        let bal = balance(&account_fixture());
        assert!(bal.contains("balance") && bal.contains("1050"));
        // Balance embeds the positions table when positions are present.
        assert!(bal.contains("MARKET") && bal.contains("1 position(s)."));

        let f = fills(&fills_fixture());
        assert!(f.contains("ROLE") && f.contains("taker"));
        assert!(f.contains("1 fill(s)."));

        let o = order(&order_fixture());
        assert!(o.contains("status") && o.contains("Open"));
    }

    #[test]
    fn empty_collections_render_friendly_messages() {
        assert_eq!(markets(&[]), "No markets returned.");
        assert_eq!(trades(&[]), "No trades returned.");
        assert_eq!(candles(&[]), "No candles returned.");
        assert_eq!(positions(&[]), "No open positions.");
        assert_eq!(fills(&[]), "No fills returned.");
        assert_eq!(orders(&[]), "No open orders.");
    }

    #[test]
    fn ticker_and_health_human_render() {
        let ticker_v: Ticker = serde_json::from_value(json!({
            "symbol": "BTC-USDX-PERP", "timestamp": 1i64, "datetime": "d",
            "last": 99.0, "bid": null, "ask": null
        }))
        .unwrap();
        let t = ticker(&ticker_v);
        assert!(t.contains("symbol") && t.contains("BTC-USDX-PERP"));
        // Absent optionals show as `-`.
        assert!(t.contains("bid           -"));

        let health_v: HealthStatus = serde_json::from_value(json!({
            "status": "ok", "timestamp_ms": 1776033900000i64,
            "services": {"indexer": "ok"}
        }))
        .unwrap();
        let h = health(&health_v);
        assert!(h.contains("status") && h.contains("ok"));
        assert!(h.contains("timestamp (ms)") && h.contains("1776033900000"));
        // The opaque `services` payload rides through as compact JSON.
        assert!(h.contains("services") && h.contains("indexer"));

        // Missing `status` renders as "unknown"; absent `services` is omitted.
        let empty: HealthStatus = serde_json::from_value(json!({})).unwrap();
        let h = health(&empty);
        assert!(h.contains("unknown"));
        assert!(!h.contains("services"));
    }

    // ───────────────────────── remaining JSON renderers ─────────────────────────

    #[test]
    fn orderbook_json_is_ccxt_level_arrays() {
        let v: Value = serde_json::from_str(&orderbook_json(&orderbook_fixture())).unwrap();
        assert_eq!(v["symbol"], json!("BTC-USDX-PERP"));
        assert_eq!(v["nonce"], json!(99));
        // Levels are [price, size] decimal-string pairs.
        assert_eq!(v["bids"][0], json!(["84000", "1.5"]));
        assert_eq!(v["asks"][0], json!(["84001", "0.5"]));
    }

    #[test]
    fn trades_json_uses_decimal_strings() {
        let v: Value = serde_json::from_str(&trades_json(&trades_fixture())).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(row["side"], json!("Buy"));
        assert_eq!(row["price"], json!("84000"));
        assert_eq!(row["is_liquidation"], json!(false));
    }

    #[test]
    fn candles_json_is_ohlcv_tuples() {
        let candles_v: Vec<Ohlcv> = serde_json::from_value(json!([[
            1_700_000_000_000i64,
            84000.0,
            84100.0,
            83900.0,
            84050.0,
            12.5
        ]]))
        .unwrap();
        let human = candles(&candles_v);
        assert!(human.contains("OPEN") && human.contains("1 candle(s)."));
        let v: Value = serde_json::from_str(&candles_json(&candles_v)).unwrap();
        let row = &v.as_array().unwrap()[0];
        // [ts, o, h, l, c, v] with money as strings, ts as a number.
        assert_eq!(row[0], json!(1_700_000_000_000i64));
        assert_eq!(row[1], json!("84000"));
        assert_eq!(row[5], json!("12.5"));
    }

    #[test]
    fn balance_json_carries_positions_and_decimal_strings() {
        let v: Value = serde_json::from_str(&balance_json(&account_fixture())).unwrap();
        assert_eq!(v["equity"], json!("1050"));
        let pos = &v["positions"][0];
        assert_eq!(pos["market_id"], json!("BTC-USDX-PERP"));
        assert_eq!(pos["liquidation_price"], json!("60000"));
    }

    #[test]
    fn positions_json_nulls_absent_liquidation_price() {
        let ps: Vec<Position> = serde_json::from_value(json!([{
            "market_id": "ETH-USDX-PERP", "side": "Sell", "size": "1",
            "entry_price": "3000", "unrealized_pnl": "-10", "realized_pnl": "0",
            "liquidation_price": null
        }]))
        .unwrap();
        let v: Value = serde_json::from_str(&positions_json(&ps)).unwrap();
        assert_eq!(v[0]["liquidation_price"], Value::Null);
    }

    // ───────────────── portfolio parity (ENG-6460) ─────────────────

    /// A position whose risk fields the server *could* derive.
    fn enriched_position_fixture() -> Vec<Position> {
        serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP", "side": "Buy", "size": "0.5",
            "entry_price": "80000", "unrealized_pnl": "50", "realized_pnl": "0",
            "liquidation_price": "60000",
            "leverage": null, "leverage_error": "margin_state_not_mirrored",
            "notional_value": "40050", "notional_value_error": null,
            "roe": "0.025", "roe_error": null,
            "margin_used": "2002.5", "margin_used_error": null,
            "max_leverage": 20, "max_leverage_error": null,
            "funding_paid": "1.25"
        }]))
        .unwrap()
    }

    #[test]
    fn positions_json_carries_every_enriched_field() {
        let v: Value = serde_json::from_str(&positions_json(&enriched_position_fixture())).unwrap();
        let row = &v.as_array().unwrap()[0];
        // Money stays a decimal string; a count stays a JSON number.
        assert_eq!(row["notional_value"], json!("40050"));
        assert_eq!(row["margin_used"], json!("2002.5"));
        assert_eq!(row["roe"], json!("0.025"));
        assert_eq!(row["funding_paid"], json!("1.25"));
        assert_eq!(row["max_leverage"], json!(20));
        // An underivable field is null WITH its reason — never 0, and never a
        // reason without a null.
        assert_eq!(row["leverage"], Value::Null);
        assert_eq!(row["leverage_error"], json!("margin_state_not_mirrored"));
        assert_eq!(row["roe_error"], Value::Null);
    }

    /// A server that predates the enriched fields (or omits them) must still
    /// render, with every enriched value null rather than defaulted to zero.
    #[test]
    fn positions_json_nulls_absent_enriched_fields_rather_than_zeroing_them() {
        let ps: Vec<Position> = serde_json::from_value(json!([{
            "market_id": "ETH-USDX-PERP", "side": "Sell", "size": "1",
            "entry_price": "3000", "unrealized_pnl": "-10", "realized_pnl": "0"
        }]))
        .unwrap();
        let v: Value = serde_json::from_str(&positions_json(&ps)).unwrap();
        for field in [
            "leverage",
            "notional_value",
            "roe",
            "margin_used",
            "max_leverage",
            "funding_paid",
            "liquidation_price",
        ] {
            assert_eq!(v[0][field], Value::Null, "{field} must be null, not 0");
        }
    }

    #[test]
    fn positions_human_shows_risk_detail_and_explains_a_blank() {
        let out = positions(&enriched_position_fixture());
        assert!(out.contains("NOTIONAL") && out.contains("40050"));
        assert!(out.contains("MARGIN USED") && out.contains("2002.5"));
        assert!(out.contains("ROE") && out.contains("0.025"));
        assert!(out.contains("MAX LEV") && out.contains("20x"));
        assert!(out.contains("FUNDING PAID") && out.contains("1.25"));
        // The one field the server could not derive is explained, and the note
        // says plainly that a blank is not a zero.
        assert!(
            out.contains("leverage (margin_state_not_mirrored): BTC-USDX-PERP"),
            "{out}"
        );
        assert!(out.contains("not zero"), "{out}");
        assert!(out.contains("1 position(s)."));
    }

    /// `leverage` is currently never derivable, so its reason would repeat once
    /// per position. Group by (field, reason) and list the markets instead.
    #[test]
    fn positions_human_groups_a_shared_reason_across_markets() {
        let ps: Vec<Position> = serde_json::from_value(json!([
            {"market_id": "BTC-USDX-PERP", "side": "Buy", "size": "1",
             "entry_price": "1", "unrealized_pnl": "0", "realized_pnl": "0",
             "leverage": null, "leverage_error": "margin_state_not_mirrored"},
            {"market_id": "ETH-USDX-PERP", "side": "Sell", "size": "1",
             "entry_price": "1", "unrealized_pnl": "0", "realized_pnl": "0",
             "leverage": null, "leverage_error": "margin_state_not_mirrored",
             "roe": null, "roe_error": "mark_price_unavailable"}
        ]))
        .unwrap();
        let out = positions(&ps);
        assert!(
            out.contains("leverage (margin_state_not_mirrored): BTC-USDX-PERP, ETH-USDX-PERP"),
            "{out}"
        );
        // A reason only one market reports stays on its own line.
        assert!(
            out.contains("roe (mark_price_unavailable): ETH-USDX-PERP"),
            "{out}"
        );
        assert_eq!(
            out.matches("margin_state_not_mirrored").count(),
            1,
            "the shared reason should print once:\n{out}"
        );
    }

    /// With nothing underivable there is no explanation block to print.
    #[test]
    fn positions_human_omits_the_note_when_nothing_is_missing() {
        let ps: Vec<Position> = serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP", "side": "Buy", "size": "0.5",
            "entry_price": "80000", "unrealized_pnl": "50", "realized_pnl": "0",
            "leverage": "5", "notional_value": "40050", "roe": "0.025",
            "margin_used": "2002.5", "max_leverage": 20, "funding_paid": "0"
        }]))
        .unwrap();
        assert!(!positions(&ps).contains("Not computable"));
    }

    fn summary_fixture() -> AccountPortfolioSummary {
        serde_json::from_value(json!({
            "collateral": "1000", "total_equity": "1050",
            "total_unrealized_pnl": "50", "total_realized_pnl_24h": "10",
            "total_volume_24h": "5000", "open_positions_count": 1,
            "open_orders_count": 2, "margin_used": "200",
            "available_margin": "800", "withdrawable": "800",
            "early_access_allowed": true
        }))
        .unwrap()
    }

    #[test]
    fn account_summary_surfaces_withdrawable() {
        let human = account_summary(&summary_fixture());
        assert!(human.contains("withdrawable") && human.contains("800"));
        assert!(human.contains("total equity") && human.contains("1050"));
        // Nothing is missing, so no footnote.
        assert!(!human.contains("does not mean zero"));

        let v: Value = serde_json::from_str(&account_summary_json(&summary_fixture())).unwrap();
        assert_eq!(v["withdrawable"], json!("800"));
        assert_eq!(v["open_positions_count"], json!(1));
        assert_eq!(v["early_access_allowed"], json!(true));
    }

    /// The spec gives the summary schema no `required` array, so every field may
    /// be absent. An absent one must read as "not reported" (`null` / `-`), never
    /// as `0` — a zeroed aggregate would make an underwater account look flat.
    #[test]
    fn account_summary_never_substitutes_zero_for_an_absent_field() {
        let empty: AccountPortfolioSummary = serde_json::from_value(json!({})).unwrap();
        let v: Value = serde_json::from_str(&account_summary_json(&empty)).unwrap();
        for field in [
            "collateral",
            "total_equity",
            "total_unrealized_pnl",
            "total_realized_pnl_24h",
            "total_volume_24h",
            "open_positions_count",
            "open_orders_count",
            "margin_used",
            "available_margin",
            "withdrawable",
            "early_access_allowed",
        ] {
            assert_eq!(v[field], Value::Null, "{field} must be null, not 0");
        }
        let human = account_summary(&empty);
        assert!(human.contains("withdrawable        -"), "{human}");
        assert!(human.contains("does not mean zero"), "{human}");
    }

    #[test]
    fn account_state_pairs_the_summary_with_its_positions() {
        let state: AccountState = serde_json::from_value(json!({
            "summary": {"total_equity": "1050", "withdrawable": "800",
                        "open_positions_count": 1},
            "positions": [{
                "market_id": "BTC-USDX-PERP", "side": "Buy", "size": "0.5",
                "entry_price": "80000", "unrealized_pnl": "50", "realized_pnl": "0"
            }]
        }))
        .unwrap();
        let human = account_state(&state);
        assert!(human.contains("withdrawable") && human.contains("800"));
        assert!(human.contains("MARKET") && human.contains("1 position(s)."));

        let v: Value = serde_json::from_str(&account_state_json(&state)).unwrap();
        assert_eq!(v["summary"]["withdrawable"], json!("800"));
        assert_eq!(v["positions"][0]["market_id"], json!("BTC-USDX-PERP"));
        // The two halves come from one read, so the reported count and the list
        // agree — surface both rather than recomputing one from the other.
        assert_eq!(
            v["summary"]["open_positions_count"],
            json!(v["positions"].as_array().unwrap().len())
        );
    }

    fn fees_fixture(maker_fee_bps: i32) -> AccountFees {
        serde_json::from_value(json!({
            "maker_fee_bps": maker_fee_bps, "taker_fee_bps": 5, "tier": "base",
            "schedule": "standard", "volume_30d": "123456.78",
            "volume_30d_estimated": false, "discounts": []
        }))
        .unwrap()
    }

    #[test]
    fn account_fees_labels_a_negative_maker_fee_as_a_rebate() {
        let human = account_fees(&fees_fixture(-2));
        assert!(human.contains("-2 bps (rebate paid to you)"), "{human}");
        assert!(human.contains("taker fee") && human.contains("5 bps"));
        assert!(human.contains("discounts") && human.contains("none"));
        // The rate is scoped to a schedule, not a venue-wide guarantee.
        assert!(human.contains("`standard` schedule"), "{human}");

        // A positive fee is not mislabelled.
        assert!(!account_fees(&fees_fixture(3)).contains("rebate"));

        let v: Value = serde_json::from_str(&account_fees_json(&fees_fixture(-2))).unwrap();
        // Bps stay signed JSON numbers; volume stays an exact decimal string.
        assert_eq!(v["maker_fee_bps"], json!(-2));
        assert_eq!(v["taker_fee_bps"], json!(5));
        assert_eq!(v["volume_30d"], json!("123456.78"));
        assert_eq!(v["volume_30d_estimated"], json!(false));
        assert_eq!(v["discounts"], json!([]));
    }

    #[test]
    fn account_fees_flags_an_estimated_volume() {
        let fees: AccountFees = serde_json::from_value(json!({
            "maker_fee_bps": 0, "taker_fee_bps": 5, "tier": "base",
            "schedule": "standard", "volume_30d": "42",
            "volume_30d_estimated": true, "discounts": [{"kind": "promo"}]
        }))
        .unwrap();
        let human = account_fees(&fees);
        assert!(human.contains("may undercount"), "{human}");
        // The discount shape isn't fixed by the spec yet, so it rides through.
        assert!(human.contains("promo"), "{human}");
        let v: Value = serde_json::from_str(&account_fees_json(&fees)).unwrap();
        assert_eq!(v["discounts"][0]["kind"], json!("promo"));
    }

    fn history_fixture(window: &str) -> PortfolioHistory {
        serde_json::from_value(json!({
            "window": window,
            "cadence_ms": 300000i64,
            "points": [
                {"timestamp_ms": 1_700_000_000_000i64, "equity": "1000",
                 "pnl": "0", "volume": "0"},
                {"timestamp_ms": 1_700_000_300_000i64, "equity": "1050",
                 "pnl": "50", "volume": "12000"}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn portfolio_history_renders_the_served_window_and_points() {
        let human = portfolio_history(&history_fixture("day"));
        assert!(human.contains("window          day"), "{human}");
        assert!(human.contains("cadence         300000 ms"), "{human}");
        assert!(human.contains("2023-11-14T22:13:20Z"), "{human}");
        assert!(human.contains("1050") && human.contains("12000"));
        assert!(human.contains("2 point(s), oldest first."), "{human}");

        let v: Value = serde_json::from_str(&portfolio_history_json(&history_fixture("day")))
            .expect("valid JSON");
        assert_eq!(v["window"], json!("day"));
        assert_eq!(v["cadence_ms"], json!(300000i64));
        let p = &v["points"][1];
        assert_eq!(p["timestamp_ms"], json!(1_700_000_300_000i64));
        assert_eq!(p["datetime"], json!("2023-11-14T22:18:20Z"));
        assert_eq!(p["equity"], json!("1050"));
        assert_eq!(p["pnl"], json!("50"));
        assert_eq!(p["volume"], json!("12000"));
    }

    /// A window added to a later spec still decodes; the CLI reports the served
    /// label rather than dropping it or claiming the requested one.
    #[test]
    fn portfolio_history_reports_an_unknown_window_verbatim() {
        let human = portfolio_history(&history_fixture("quarter"));
        assert!(
            human.contains("quarter (unknown to this CLI build)"),
            "{human}"
        );
        let v: Value =
            serde_json::from_str(&portfolio_history_json(&history_fixture("quarter"))).unwrap();
        assert_eq!(v["window"], json!("quarter"));
    }

    /// An empty series says so instead of rendering a headerless table — but the
    /// window/cadence it was served under stay visible.
    #[test]
    fn portfolio_history_handles_an_empty_series() {
        let empty: PortfolioHistory = serde_json::from_value(json!({
            "window": "all", "cadence_ms": 86400000i64, "points": []
        }))
        .unwrap();
        let human = portfolio_history(&empty);
        assert!(human.contains("No points in this window."), "{human}");
        assert!(human.contains("window          all"), "{human}");
        let v: Value = serde_json::from_str(&portfolio_history_json(&empty)).unwrap();
        assert_eq!(v["points"], json!([]));
    }

    /// Free-form server strings reach a terminal that interprets ESC sequences,
    /// so control bytes are neutralized before printing. JSON is untouched —
    /// `serde_json` escapes them itself, and machine consumers don't interpret.
    #[test]
    fn server_strings_cannot_smuggle_terminal_escapes() {
        let evil = "day\u{1b}[2K\u{1b}[31mwithdrawable  999999";
        let history: PortfolioHistory = serde_json::from_value(json!({
            "window": evil, "cadence_ms": 1i64, "points": []
        }))
        .unwrap();
        let human = portfolio_history(&history);
        assert!(
            !human.contains('\u{1b}'),
            "escape byte reached stdout: {human:?}"
        );
        assert!(human.contains("day?[2K?[31m"), "{human}");
        // The JSON view keeps the value verbatim (escaped by serde_json).
        let raw = portfolio_history_json(&history);
        assert!(!raw.contains('\u{1b}'), "raw escape in JSON: {raw:?}");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["window"], json!(evil));

        // Same for a position's `side` and a `*_error` reason.
        let ps: Vec<Position> = serde_json::from_value(json!([{
            "market_id": "BTC-USDX-PERP", "side": "Buy\u{1b}[31m", "size": "1",
            "entry_price": "1", "unrealized_pnl": "0", "realized_pnl": "0",
            "roe": null, "roe_error": "mark_price_unavailable\u{1b}[2J"
        }]))
        .unwrap();
        assert!(!positions(&ps).contains('\u{1b}'));

        // And for the fee schedule's open strings.
        let fees: AccountFees = serde_json::from_value(json!({
            "maker_fee_bps": 0, "taker_fee_bps": 0, "tier": "base\u{1b}[0m",
            "schedule": "standard\u{7f}", "volume_30d": "0",
            "volume_30d_estimated": false, "discounts": []
        }))
        .unwrap();
        let human = account_fees(&fees);
        assert!(
            !human.contains('\u{1b}') && !human.contains('\u{7f}'),
            "{human:?}"
        );
    }

    #[test]
    fn fills_json_preserves_taker_or_maker() {
        let v: Value = serde_json::from_str(&fills_json(&fills_fixture())).unwrap();
        let row = &v.as_array().unwrap()[0];
        assert_eq!(row["taker_or_maker"], json!("taker"));
        assert_eq!(row["fee"], json!("0.42"));
        assert_eq!(row["side"], json!("Sell"));
    }

    #[test]
    fn order_result_counts_immediate_fills() {
        let resp: OrderResponse = serde_json::from_value(json!({
            "order": {
                "id": "o1", "market_id": "BTC-USDX-PERP", "side": "Buy",
                "order_type": "Market", "quantity": "0.01", "filled_qty": "0.01",
                "status": "Filled", "time_in_force": "IOC"
            },
            "fills": [{"x": 1}, {"y": 2}]
        }))
        .unwrap();
        let human = order_result(&resp);
        assert!(human.contains("immediate fills") && human.contains('2'));
        let v: Value = serde_json::from_str(&order_result_json(&resp)).unwrap();
        assert_eq!(v["order"]["status"], json!("Filled"));
        assert_eq!(v["fills"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn orders_human_table_lists_rows() {
        let os = vec![order_fixture()];
        let out = orders(&os);
        assert!(out.contains("ID") && out.contains("STATUS"));
        assert!(out.contains("o1") && out.contains("BTC-USDX-PERP"));
        assert!(out.contains("1 order(s)."));
    }

    #[test]
    fn cancel_pairs_a_note_with_the_pretty_body() {
        let body = json!({"cancelled": true});
        let out = cancel(&body, "cancelled order o1.");
        assert!(out.starts_with("cancelled order o1."));
        // The server body is pretty-printed beneath the note.
        assert!(out.contains("\"cancelled\": true"));
    }

    fn batch_fixture() -> Vec<OrderResult> {
        // A non-atomic batch where the second order rejected; the array keeps
        // request order and reports each outcome independently.
        serde_json::from_value(json!([
            {
                "outcome": "ok",
                "order": {
                    "id": "o1", "market_id": "BTC-USDX-PERP", "side": "Buy",
                    "order_type": "Limit", "price": "84000", "quantity": "0.01",
                    "filled_qty": "0", "status": "Open", "time_in_force": "GTC"
                },
                "fills": []
            },
            {
                "outcome": "err",
                "error": "INSUFFICIENT_MARGIN",
                "message": "not enough margin"
            }
        ]))
        .unwrap()
    }

    #[test]
    fn order_batch_tallies_and_renders_each_outcome() {
        let out = order_batch(&batch_fixture(), "submitted 2 order(s).");
        assert!(out.contains("submitted 2 order(s)."));
        assert!(out.contains("1 placed, 1 rejected."));
        assert!(out.contains("[0] OK") && out.contains("o1"));
        assert!(out.contains("[1] REJECTED  INSUFFICIENT_MARGIN: not enough margin"));
    }

    #[test]
    fn order_batch_json_preserves_outcome_tags() {
        let v: Value = serde_json::from_str(&order_batch_json(&batch_fixture())).unwrap();
        let rows = v.as_array().unwrap();
        assert_eq!(rows[0]["outcome"], json!("ok"));
        assert_eq!(rows[0]["order"]["id"], json!("o1"));
        assert_eq!(rows[1]["outcome"], json!("err"));
        assert_eq!(rows[1]["error"], json!("INSUFFICIENT_MARGIN"));
    }

    // ───────────────────────── bridge ─────────────────────────

    fn bridge_assets_fixture() -> BridgeAssetsResponse {
        serde_json::from_value(json!({
            "chains": [{
                "chain": "base",
                "chain_id": 8453,
                "deposit_assets": [{
                    "symbol": "USDC",
                    "decimals": 6,
                    "min_amount": "10",
                    "confirmations": 12,
                    "fee": "0",
                    "contract_address": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
                }],
                // A withdraw-only entry with the optional halves absent, so the
                // `-` rendering and the `null` JSON are both exercised.
                "withdraw_assets": [{
                    "symbol": "USDX",
                    "decimals": 18,
                    "min_amount": "1",
                    "confirmations": 12
                }]
            }]
        }))
        .unwrap()
    }

    fn bridge_deposit_fixture() -> BridgeDeposit {
        serde_json::from_value(json!({
            "id": "dep_1",
            "account_id": "0xabc",
            "chain": "base",
            "asset": "USDC",
            "amount": "125.5",
            "address": "0xdead",
            "status": "confirming",
            "confirmations": 3,
            "required_confirmations": 12,
            "tx_hash": "0xfeed",
            "created_at": 1_700_000_000_000i64,
            "updated_at": 1_700_000_060_000i64
        }))
        .unwrap()
    }

    #[test]
    fn bridge_assets_lists_both_directions_and_marks_withdraw_as_unavailable() {
        let out = bridge_assets(&bridge_assets_fixture());
        assert!(out.contains("base (8453)"), "chain id is shown: {out}");
        assert!(out.contains("deposit") && out.contains("withdraw"));
        assert!(out.contains("1 chain(s), 2 asset(s)."), "tally: {out}");
        // The withdraw row must not read as something the CLI can do.
        assert!(
            out.contains("no withdrawal command"),
            "withdraw rows need the caveat: {out}"
        );
        // An absent fee/contract is `-`, never a fabricated zero or empty cell.
        assert!(out.contains('-'), "missing optionals render as `-`: {out}");
    }

    #[test]
    fn bridge_assets_json_keeps_money_as_strings_and_nulls_absent_optionals() {
        let v: Value = serde_json::from_str(&bridge_assets_json(&bridge_assets_fixture())).unwrap();
        let chain = &v["chains"][0];
        assert_eq!(chain["chain"], json!("base"));
        assert_eq!(chain["chain_id"], json!(8453));
        let dep = &chain["deposit_assets"][0];
        assert_eq!(dep["min_amount"], json!("10"), "decimals stay strings");
        assert_eq!(dep["fee"], json!("0"));
        assert_eq!(dep["decimals"], json!(6), "counts stay JSON numbers");
        let wd = &chain["withdraw_assets"][0];
        assert_eq!(wd["fee"], Value::Null, "an absent fee is null, not 0");
        assert_eq!(wd["contract_address"], Value::Null);
    }

    #[test]
    fn bridge_assets_renders_an_empty_response_without_a_table() {
        let empty: BridgeAssetsResponse = serde_json::from_value(json!({ "chains": [] })).unwrap();
        assert_eq!(bridge_assets(&empty), "No bridge chains.");
    }

    #[test]
    fn bridge_deposit_address_json_shape() {
        let a: BridgeDepositAddress = serde_json::from_value(json!({
            "address": "0xdead",
            "chain": "base",
            "accepts": ["USDC", "USDX"],
            "account_id": "0xabc",
            "created_at": 1_700_000_000_000i64
        }))
        .unwrap();
        let v: Value = serde_json::from_str(&bridge_deposit_address_json(&a)).unwrap();
        assert_eq!(
            keys(&v),
            ["accepts", "account_id", "address", "chain", "created_at"]
        );
        assert_eq!(
            v["created_at"],
            json!("2023-11-14T22:13:20Z"),
            "timestamps are ISO-8601 UTC, as in market_status"
        );
        assert!(bridge_deposit_address(&a).contains("USDC,USDX"));
    }

    #[test]
    fn bridge_deposits_report_confirmations_as_seen_over_required() {
        let out = bridge_deposits(&[bridge_deposit_fixture()]);
        assert!(out.contains("3/12"), "conf as seen/required: {out}");
        assert!(out.contains("1 deposit(s)."));
        // The tx hash belongs to the detail view, not the table.
        assert!(
            !out.contains("0xfeed"),
            "tx hash stays out of the table: {out}"
        );
        assert!(bridge_deposit(&bridge_deposit_fixture()).contains("0xfeed"));
    }

    /// A deposit not yet seen on chain reports neither count. `0/0` would read as
    /// "confirmed, nothing required", so both halves must show `-`.
    #[test]
    fn an_unseen_deposit_shows_dashes_not_zeroes_for_confirmations() {
        let mut d = bridge_deposit_fixture();
        d.confirmations = None;
        d.required_confirmations = None;
        d.tx_hash = None;
        assert!(bridge_deposits(&[d.clone()]).contains("-/-"));
        let detail = bridge_deposit(&d);
        assert!(detail.contains("-/-"), "detail view too: {detail}");

        let v: Value = serde_json::from_str(&bridge_deposit_json(&d)).unwrap();
        assert_eq!(v["confirmations"], Value::Null);
        assert_eq!(v["required_confirmations"], Value::Null);
        assert_eq!(v["tx_hash"], Value::Null);
        assert_eq!(v["credited_at"], Value::Null, "uncredited is null, not 0");
    }

    #[test]
    fn bridge_deposit_json_shape() {
        let v: Value =
            serde_json::from_str(&bridge_deposit_json(&bridge_deposit_fixture())).unwrap();
        assert_eq!(
            keys(&v),
            [
                "account_id",
                "address",
                "amount",
                "asset",
                "chain",
                "confirmations",
                "created_at",
                "credited_at",
                "id",
                "required_confirmations",
                "status",
                "tx_hash",
                "updated_at",
            ]
        );
        assert_eq!(v["amount"], json!("125.5"), "money stays a string");
        assert_eq!(v["created_at"], json!("2023-11-14T22:13:20Z"));
    }

    /// Chain, asset and status are open strings the server chooses, so an escape
    /// sequence in one must not reach the terminal intact.
    #[test]
    fn bridge_output_neutralizes_control_characters() {
        let mut d = bridge_deposit_fixture();
        d.status = "credited\u{1b}[2K".to_string();
        for out in [bridge_deposits(&[d.clone()]), bridge_deposit(&d)] {
            assert!(!out.contains('\u{1b}'), "ESC must not survive: {out:?}");
            assert!(out.contains("credited?[2K"));
        }
    }

    #[test]
    fn empty_bridge_lists_say_so_and_point_somewhere() {
        assert_eq!(bridge_deposits(&[]), "No bridge deposits.");
        let addrs = bridge_deposit_addresses(&[]);
        assert!(
            addrs.contains("--chain"),
            "the empty case should name the command that fixes it: {addrs}"
        );
    }

    // ── ENG-9198: renderers for the five mutations ported from #74 ──

    #[test]
    fn order_preview_rejection_is_not_an_error() {
        let rejected: OrderPreview = serde_json::from_value(json!({
            "accepted": false,
            "reject_reason": "insufficient margin",
        }))
        .unwrap();
        let out = order_preview(&rejected);
        assert!(out.contains("REJECTED"), "{out}");
        assert!(out.contains("insufficient margin"), "{out}");

        let accepted: OrderPreview = serde_json::from_value(json!({
            "accepted": true,
            "required_initial_margin": "500",
        }))
        .unwrap();
        let out = order_preview(&accepted);
        assert!(out.contains("ACCEPTED"), "{out}");
        // Says out loud that nothing was submitted, so an accepted preview is
        // never mistaken for a placed order.
        assert!(out.contains("nothing was placed"), "{out}");

        let silent: OrderPreview = serde_json::from_value(json!({})).unwrap();
        let out = order_preview(&silent);
        assert!(!out.contains("ACCEPTED"), "{out}");
        assert!(out.contains("NOT accepted"), "{out}");
    }

    #[test]
    fn server_strings_in_mutation_renderers_are_control_char_safe() {
        let esc = "BTC\u{1b}[2K\u{1b}[31mCREDITED";
        let pv: OrderPreview =
            serde_json::from_value(json!({ "accepted": false, "reject_reason": esc })).unwrap();
        assert!(!order_preview(&pv).contains('\u{1b}'));

        let m: MarginAdjustment = serde_json::from_value(json!({
            "market_id": esc, "allocated_margin": "1", "collateral": "2",
        }))
        .unwrap();
        assert!(!margin_adjustment(&m).contains('\u{1b}'));
    }

    #[test]
    fn mutation_json_renderers_keep_decimals_as_strings() {
        // Money round-trips as the exact string the exchange sent, never as a
        // JSON float.
        let m: MarginAdjustment = serde_json::from_value(json!({
            "market_id": "BTC-USDX-PERP",
            "allocated_margin": "1000.5",
            "collateral": "2000.25",
        }))
        .unwrap();
        let v: Value = serde_json::from_str(&margin_adjustment_json(&m)).unwrap();
        assert_eq!(v["allocated_margin"], json!("1000.5"));
        assert_eq!(keys(&v), ["allocated_margin", "collateral", "market_id"]);

        let d: DepositResponse = serde_json::from_value(json!({ "balance": "123.456" })).unwrap();
        let v: Value = serde_json::from_str(&deposit_created_json(&d)).unwrap();
        assert_eq!(v["balance"], json!("123.456"));

        let f: FaucetResponse = serde_json::from_value(json!({
            "amount": "1000", "available_at_ms": 1_700_000_000_000_i64,
        }))
        .unwrap();
        let v: Value = serde_json::from_str(&faucet_json(&f)).unwrap();
        assert_eq!(v["amount"], json!("1000"));
        assert_eq!(v["available_at_ms"], json!(1_700_000_000_000_i64));
    }
}
