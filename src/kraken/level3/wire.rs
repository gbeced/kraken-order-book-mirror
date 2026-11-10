use crate::common::ts::Ts;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer};

// https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/level3

pub (super) type OrderId = [u8; 19];

#[derive(Deserialize)]
pub (super) struct Envelope<T> {
    pub data: Vec<T>,
}

#[derive(Deserialize)]
pub (super) struct Payload<T> {
    // symbol: String,
    pub checksum: u32,
    #[serde(deserialize_with = "deserialize_ts")]
    pub timestamp: Ts,
    pub bids: Vec<T>,
    pub asks: Vec<T>,
}

#[derive(Deserialize)]
pub (super) struct Order {
    #[serde(deserialize_with = "deserialize_order_id")]
    pub order_id: OrderId,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub order_qty: Decimal,
    #[serde(deserialize_with = "deserialize_ts")]
    pub timestamp: Ts,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub (super) enum EventType {
    Add,
    Modify,
    Delete,
}

#[derive(Deserialize)]
pub (super) struct OrderEvent {
    #[serde(deserialize_with = "deserialize_order_id")]
    pub order_id: OrderId,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub order_qty: Decimal,
    #[serde(deserialize_with = "deserialize_ts")]
    pub timestamp: Ts,
    pub event: EventType,
}

fn deserialize_ts<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Ts, D::Error> {
    let dt = DateTime::<Utc>::deserialize(deserializer)?;
    Ts::try_from(dt).map_err(serde::de::Error::custom)
}

fn deserialize_order_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<OrderId, D::Error> {
    let s = <&str>::deserialize(deserializer)?;

    if s.len() > size_of::<OrderId>() {
        Err(serde::de::Error::invalid_length(s.len(), &"a 19-byte order id"))
    }
    else {
        let mut ret: OrderId = [0; _];
        let n = s.len().min(ret.len());
        ret[..n].copy_from_slice(&s.as_bytes()[..n]);
        Ok(ret)
    }
}