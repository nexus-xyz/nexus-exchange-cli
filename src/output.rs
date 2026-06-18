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
}
