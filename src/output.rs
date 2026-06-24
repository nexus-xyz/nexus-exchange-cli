//! Human-readable and JSON rendering of the SDK's response types.
//!
//! The SDK's wire types are mostly deserialize-only, so we format them by hand
//! rather than re-serializing. Money is the SDK's [`Decimal`], rendered as a
//! decimal string in JSON so no precision is lost and the output round-trips the
//! exact value the exchange sent.

use nexus_exchange::types::{
    AccountSummary, Fill, HealthStatus, Market, Ohlcv, Order, OrderBook, OrderResponse, Position,
    PriceLevel, Side, Ticker, Trade,
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
            "events_received": 7, "fills_total": 3, "uptime_seconds": 42,
            "connected": true
        }))
        .unwrap();
        let h = health(&health_v);
        assert!(h.contains("connected") && h.contains("true"));
        // Missing `health` field defaults to "unknown".
        assert!(h.contains("unknown"));
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
}
