//! Paper trading (ENG-20366): `nexus --paper order place` simulates the order
//! against the live public order book instead of sending it.
//!
//! The market's book is fetched (`GET /markets/{market_id}/orderbook`, public,
//! no credentials) and the order is walked against it: a market order takes
//! levels until it fills or the book runs out, a limit order takes only the
//! levels its price crosses. `IOC` drops what is left, `FOK` fills all or
//! nothing, `PostOnly` is rejected if it would cross, and a `GTC` (or a
//! non-crossing `PostOnly`) limit reports that its remainder would rest.
//!
//! Deliberately NOT simulated, and said so in every result: margin and balance
//! checks, fees, funding, liquidation and positions. Nothing persists between
//! invocations, so a "resting" paper order is a report, not a tracked order.

use nexus_exchange::types::{Decimal, OrderBook, OrderRequest, OrderType, Side, TimeInForce};
use serde_json::json;

pub const NOTE: &str = "Paper mode: simulated against a snapshot of the live public order book; \
     no order reached the exchange. Margin, fees, funding, liquidation and positions are not \
     simulated, and nothing is kept between invocations, so a resting remainder is not tracked.";

/// The outcome of walking one order against one book snapshot.
#[derive(Debug, PartialEq)]
pub struct Simulated {
    pub status: &'static str,
    pub reason: Option<String>,
    /// `(price, size)` per level taken, best first.
    pub fills: Vec<(Decimal, Decimal)>,
    pub filled: Decimal,
    pub remaining: Decimal,
    pub average_price: Option<Decimal>,
    /// A `GTC` limit's unfilled remainder, which would rest on the book.
    pub rests: bool,
}

/// Walk `order` against `book`. Pure, so the matching rules are tested offline.
pub fn simulate(book: &OrderBook, order: &OrderRequest) -> Simulated {
    let buy = order.side == Side::Buy;
    let limit = (order.order_type == OrderType::Limit)
        .then_some(order.price)
        .flatten();
    let mut levels: Vec<(Decimal, Decimal)> = if buy { &book.asks } else { &book.bids }
        .iter()
        .map(|l| (l.price(), l.amount()))
        .filter(|(p, _)| limit.is_none_or(|l| if buy { *p <= l } else { *p >= l }))
        .collect();
    // Best level first, whatever order the snapshot came in.
    levels.sort_by(|a, b| if buy { a.0.cmp(&b.0) } else { b.0.cmp(&a.0) });

    let size = order.quantity;
    let tif = order.time_in_force;
    let reject = |reason: String| Simulated {
        status: "rejected",
        reason: Some(reason),
        fills: vec![],
        filled: Decimal::ZERO,
        remaining: size,
        average_price: None,
        rests: false,
    };
    if order.order_type == OrderType::Limit && tif == TimeInForce::PostOnly && !levels.is_empty() {
        return reject("PostOnly order would cross the book".into());
    }
    let available: Decimal = levels.iter().map(|(_, q)| *q).sum();
    if tif == TimeInForce::Fok && available < size {
        return reject(format!("FOK: only {available} available at this price"));
    }

    let mut fills = vec![];
    let mut left = size;
    let mut notional = Decimal::ZERO;
    for (price, qty) in levels {
        if left <= Decimal::ZERO {
            break;
        }
        let take = qty.min(left);
        fills.push((price, take));
        notional += price * take;
        left -= take;
    }
    let filled = size - left;
    // A PostOnly that got this far did not cross, so it rests like a GTC.
    let rests = order.order_type == OrderType::Limit
        && matches!(tif, TimeInForce::Gtc | TimeInForce::PostOnly)
        && left > Decimal::ZERO;
    let status = match (left > Decimal::ZERO, filled > Decimal::ZERO, rests) {
        (false, _, _) => "filled",
        (true, true, true) => "partially_filled",
        (true, false, true) => "open",
        (true, true, false) => "partially_filled_remainder_cancelled",
        (true, false, false) => "cancelled",
    };
    Simulated {
        status,
        reason: None,
        fills,
        filled,
        remaining: left,
        average_price: (filled > Decimal::ZERO)
            .then(|| (notional / filled).round_dp(10).normalize()),
        rests,
    }
}

/// The JSON document for `--output json`.
pub fn to_json(order: &OrderRequest, s: &Simulated) -> String {
    let value = json!({
        "simulated": true,
        "market_id": order.market_id,
        "side": order.side,
        "type": order.order_type,
        "price": order.price.map(|p| p.to_string()),
        "quantity": order.quantity.to_string(),
        "time_in_force": order.time_in_force,
        "status": s.status,
        "reason": s.reason,
        "filled_size": s.filled.to_string(),
        "remaining_size": s.remaining.to_string(),
        "average_price": s.average_price.map(|p| p.to_string()),
        "fills": s.fills.iter().map(|(p, q)| json!({"price": p.to_string(), "size": q.to_string()})).collect::<Vec<_>>(),
        "rests": s.rests,
        "note": NOTE,
    });
    serde_json::to_string_pretty(&value).expect("JSON value is always serializable")
}

/// The human-readable rendering.
pub fn to_human(order: &OrderRequest, s: &Simulated) -> String {
    let mut out = format!(
        "PAPER (simulated, nothing sent)\n{:<16}{}\n{:<16}{:?} {:?}\n{:<16}{}\n{:<16}{} of {}\n",
        "market",
        order.market_id,
        "order",
        order.side,
        order.order_type,
        "status",
        s.status,
        "filled",
        s.filled,
        order.quantity,
    );
    if let Some(reason) = &s.reason {
        out.push_str(&format!("{:<16}{reason}\n", "reason"));
    }
    if let Some(avg) = s.average_price {
        out.push_str(&format!("{:<16}{avg}\n", "average price"));
    }
    for (p, q) in &s.fills {
        out.push_str(&format!("{:<16}{q} @ {p}\n", "fill"));
    }
    out.push_str(NOTE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn book() -> OrderBook {
        serde_json::from_value(json!({
            "symbol": "M", "timestamp": 0, "datetime": "", "nonce": 0,
            "bids": [[99.0, 1.0], [98.0, 2.0]],
            "asks": [[102.0, 2.0], [101.0, 1.0]],
        }))
        .unwrap()
    }

    fn limit(side: Side, price: &str, qty: &str, tif: TimeInForce) -> OrderRequest {
        OrderRequest::limit("M", side, d(price), d(qty), tif)
    }

    #[test]
    fn market_buy_walks_the_asks_best_first_for_a_vwap() {
        let s = simulate(&book(), &OrderRequest::market("M", Side::Buy, d("2")));
        assert_eq!(s.status, "filled");
        assert_eq!(s.fills, vec![(d("101"), d("1")), (d("102"), d("1"))]);
        assert_eq!(s.average_price, Some(d("101.5")));
    }

    #[test]
    fn market_order_bigger_than_the_book_cancels_the_rest() {
        let s = simulate(&book(), &OrderRequest::market("M", Side::Sell, d("5")));
        assert_eq!(s.status, "partially_filled_remainder_cancelled");
        assert_eq!((s.filled, s.remaining), (d("3"), d("2")));
    }

    #[test]
    fn crossing_gtc_limit_fills_what_crosses_and_rests_the_rest() {
        let s = simulate(&book(), &limit(Side::Buy, "101", "2", TimeInForce::Gtc));
        assert_eq!(s.status, "partially_filled");
        assert_eq!(s.filled, d("1"));
        assert!(s.rests);
    }

    #[test]
    fn tif_rules() {
        let b = book();
        assert_eq!(
            simulate(&b, &limit(Side::Buy, "100", "2", TimeInForce::Gtc)).status,
            "open"
        );
        assert_eq!(
            simulate(&b, &limit(Side::Buy, "100", "2", TimeInForce::Ioc)).status,
            "cancelled"
        );
        assert_eq!(
            simulate(&b, &limit(Side::Buy, "101", "2", TimeInForce::Fok)).status,
            "rejected"
        );
        assert_eq!(
            simulate(&b, &limit(Side::Buy, "101", "1", TimeInForce::PostOnly)).status,
            "rejected"
        );
        assert_eq!(
            simulate(&b, &limit(Side::Buy, "100", "1", TimeInForce::PostOnly)).status,
            "open"
        );
    }
}
