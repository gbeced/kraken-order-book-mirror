use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::error::Error;

#[derive(Deserialize)]
struct Order {
    order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    order_qty: Decimal,
    timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
struct OrderBookSnapshot {
    symbol: String,
    checksum: u32,
    timestamp: DateTime<Utc>,
    bids: Vec<Order>,
    asks: Vec<Order>,
}

impl OrderBookSnapshot {
    pub fn from_json_str(raw: &str) -> Result<OrderBookSnapshot, Box<dyn Error>> {
        Ok(serde_json::from_str(raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ok() {
        let now = Utc::now();
        let order_book = OrderBookSnapshot {
            symbol: String::from("BTC/USDT"),
            checksum: 0,
            timestamp: now,
            bids: Vec::new(),
            asks: Vec::new(),
        };

        assert_eq!(order_book.asks.len(), 0);
        assert_eq!(order_book.bids.len(), 0);
        assert_eq!(order_book.checksum, 0);
    }

    #[test]
    fn new_from_snapshot() {
        let order_book = OrderBookSnapshot::from_json_str(
            &std::fs::read_to_string("tests/data/level3_snapshot.json").unwrap(),
        )
        .unwrap();

        assert_eq!(order_book.symbol, String::from("BTC/USD"));
        assert_eq!(order_book.checksum, 30690248);
        assert_eq!(
            order_book.timestamp.to_string(),
            "2026-09-15 21:28:25.745828658 UTC"
        );
        assert_eq!(order_book.asks.len(), 18);
        assert_eq!(order_book.bids.len(), 13);
    }
}
