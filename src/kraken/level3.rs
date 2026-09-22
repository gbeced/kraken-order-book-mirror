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
    event: Option<String>,
}

#[derive(Deserialize)]
struct OrderBook {
    symbol: String,
    checksum: u32,
    timestamp: DateTime<Utc>,
    bids: Vec<Order>,
    asks: Vec<Order>,
}

impl OrderBook {
    pub fn from_json_str(raw: &str) -> Result<OrderBook, Box<dyn Error>> {
        Ok(serde_json::from_str(raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_from_snapshot() {
        let order_book = OrderBook::from_json_str(
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
        assert_eq!(order_book.bids[0].event, None);
    }

    #[test]
    fn new_from_update() {
        let order_book = OrderBook::from_json_str(
            &std::fs::read_to_string("tests/data/level3_update_1.json").unwrap(),
        )
        .unwrap();

        assert_eq!(order_book.symbol, String::from("BTC/USD"));
        assert_eq!(order_book.checksum, 782538988);
        assert_eq!(
            order_book.timestamp.to_string(),
            "2026-09-15 21:28:25.964171032 UTC"
        );
        assert_eq!(order_book.asks.len(), 0);
        assert_eq!(order_book.bids.len(), 3);
        assert_eq!(order_book.bids[0].event, Some(String::from("delete")));
    }

}
