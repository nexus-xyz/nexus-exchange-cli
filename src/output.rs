//! Human-readable rendering of SDK response types.
//!
//! The SDK's wire types are deserialize-only, so we format them by hand rather
//! than re-serializing.

use nexus_exchange::types::{HealthStatus, Market, Ticker};

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
