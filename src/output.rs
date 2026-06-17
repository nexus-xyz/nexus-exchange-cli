//! Human-readable rendering of SDK response types.
//!
//! The SDK's wire types are deserialize-only, so we format them by hand rather
//! than re-serializing.

use nexus_exchange::types::{HealthStatus, Market, Ticker};
use serde_json::{json, Value};

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

/// Render a single ticker as aligned key/value lines.
pub fn ticker(t: &Ticker) -> String {
    let rows = [
        ("symbol", t.symbol.clone()),
        ("datetime", t.datetime.clone()),
        ("last", opt(t.last)),
        ("mark price", opt(t.mark_price)),
        ("index price", opt(t.index_price)),
        ("bid", opt(t.bid)),
        ("ask", opt(t.ask)),
        ("high", opt(t.high)),
        ("low", opt(t.low)),
        ("open", opt(t.open)),
        ("close", opt(t.close)),
        ("change", opt(t.change)),
        ("percentage", opt(t.percentage)),
        ("base volume", opt(t.base_volume)),
        ("quote volume", opt(t.quote_volume)),
    ];
    rows.iter()
        .map(|(k, v)| format!("{k:<14}{v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

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

/// Format an optional value, showing `-` when absent.
fn opt<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|d| d.to_string()).unwrap_or_else(|| "-".to_string())
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

/// Render a single ticker as pretty JSON.
pub fn ticker_json(t: &Ticker) -> String {
    let value = json!({
        "symbol": t.symbol,
        "datetime": t.datetime,
        "last": opt_json(t.last),
        "mark_price": opt_json(t.mark_price),
        "index_price": opt_json(t.index_price),
        "bid": opt_json(t.bid),
        "ask": opt_json(t.ask),
        "high": opt_json(t.high),
        "low": opt_json(t.low),
        "open": opt_json(t.open),
        "close": opt_json(t.close),
        "change": opt_json(t.change),
        "percentage": opt_json(t.percentage),
        "base_volume": opt_json(t.base_volume),
        "quote_volume": opt_json(t.quote_volume),
    });
    pretty(&value)
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

/// Render an optional value as a JSON string, or `null` when absent.
fn opt_json<T: std::fmt::Display>(v: Option<T>) -> Value {
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
