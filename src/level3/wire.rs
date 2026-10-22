use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;

// https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/level3

#[derive(Deserialize)]
pub struct Envelope<T> {
    pub data: Vec<T>,
}

#[derive(Deserialize)]
pub struct Payload<T> {
    // symbol: String,
    pub checksum: u32,
    pub timestamp: DateTime<Utc>,
    pub bids: Vec<T>,
    pub asks: Vec<T>,
}

#[derive(Deserialize)]
pub struct WireOrder {
    pub order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub order_qty: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Add,
    Modify,
    Delete,
}

#[derive(Deserialize)]
pub struct WireOrderEvent {
    pub order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub order_qty: Decimal,
    pub timestamp: DateTime<Utc>,
    pub event: EventType,
}
