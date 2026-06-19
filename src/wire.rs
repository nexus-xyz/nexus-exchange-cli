//! Wire types for the endpoints the SDK does not yet model.
//!
//! Two encodings show up on the wire and we preserve both faithfully:
//!
//! * **Market data** (`orderbook`, `trades`, `candles`) is CCXT-shaped and sends
//!   prices/sizes as JSON *numbers* — kept as `f64`.
//! * **Account data** (`balance`, `positions`, `fills`, `orders`) sends money as
//!   decimal *strings* — kept as `String` so no precision is lost and so JSON
//!   output round-trips the exact value the exchange sent.
//!
//! All response structs ignore unknown fields, so a forward-compatible server
//! that adds fields won't break deserialization.

use serde::{Deserialize, Serialize};

// ───────────────────────── public market data ─────────────────────────

/// One side of the book is a list of `[price, amount]` pairs.
pub type Level = [f64; 2];

#[derive(Debug, Clone, Deserialize)]
pub struct OrderBook {
    pub symbol: String,
    #[serde(default)]
    pub bids: Vec<Level>,
    #[serde(default)]
    pub asks: Vec<Level>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub datetime: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trade {
    pub id: String,
    pub symbol: String,
    pub price: f64,
    pub amount: f64,
    #[serde(default)]
    pub cost: Option<f64>,
    pub side: String,
    pub timestamp: i64,
    #[serde(default)]
    pub datetime: Option<String>,
    #[serde(default)]
    pub is_liquidation: bool,
}

/// OHLCV candle: `[timestamp_ms, open, high, low, close, volume]`.
pub type Candle = [f64; 6];

// ───────────────────────── authenticated account ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub market_id: String,
    pub side: String,
    pub size: String,
    pub entry_price: String,
    pub unrealized_pnl: String,
    pub realized_pnl: String,
    pub liquidation_price: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Balance {
    pub balance: String,
    pub collateral: String,
    pub equity: String,
    pub available_margin: String,
    #[serde(default)]
    pub positions: Vec<Position>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Fill {
    pub id: String,
    pub order_id: String,
    pub market_id: String,
    pub side: String,
    pub price: String,
    pub size: String,
    pub fee: String,
    #[serde(default)]
    pub taker_or_maker: Option<String>,
    pub timestamp: i64,
    #[serde(default)]
    pub is_liquidation: bool,
}

/// An order. The live API diverges from the published spec in two ways we
/// tolerate here: the price field is sent as `limit_price` (accepted via a serde
/// alias), and `account_id` is a 20-byte address array rather than a string
/// (kept as a raw [`Value`] and rendered as `0x…` hex by the output layer).
#[derive(Debug, Clone, Deserialize)]
pub struct Order {
    pub id: String,
    pub market_id: String,
    #[serde(default)]
    pub account_id: Option<serde_json::Value>,
    pub side: String,
    pub order_type: String,
    #[serde(default, alias = "limit_price")]
    pub price: Option<String>,
    pub quantity: String,
    #[serde(default)]
    pub filled_qty: Option<String>,
    pub status: String,
    pub time_in_force: String,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub updated_at: Option<i64>,
}

impl Order {
    /// Render `account_id` as a `0x…` hex address when it arrives as a byte
    /// array, otherwise pass through whatever the server sent (e.g. a string).
    pub fn account_hex(&self) -> Option<String> {
        match &self.account_id {
            Some(serde_json::Value::Array(bytes)) => {
                let mut s = String::from("0x");
                for b in bytes {
                    let byte = u8::try_from(b.as_u64()?).ok()?;
                    s.push_str(&format!("{byte:02x}"));
                }
                Some(s)
            }
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

/// `POST /orders` response: the resting/closed order plus any immediate fills.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResult {
    pub order: Order,
    #[serde(default)]
    pub fills: Vec<serde_json::Value>,
}

/// Request body for `POST /orders`. Serialized to the exact bytes that are both
/// signed and sent, so the signature always matches the payload.
#[derive(Debug, Clone, Serialize)]
pub struct NewOrder {
    pub market_id: String,
    pub side: String,
    pub order_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    pub quantity: String,
    pub time_in_force: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_only: Option<bool>,
}

/// `POST /ws/token` response. The server also returns `expires_at`, but the
/// token is single-use and consumed immediately, so we only need the token
/// itself (unknown fields are ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct WsToken {
    pub token: String,
}
