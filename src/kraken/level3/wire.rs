use crate::common::ts::Ts;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

// https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/level3

pub(super) type OrderId = [u8; 19];

#[derive(Deserialize)]
pub(super) struct Envelope<T> {
    pub data: Vec<T>,
}

#[derive(Deserialize)]
pub(super) struct Payload<T> {
    // symbol: String,
    pub checksum: u32,
    #[serde(deserialize_with = "deserialize_ts")]
    pub timestamp: Ts,
    pub bids: Vec<T>,
    pub asks: Vec<T>,
}

// Prices and quantities stay as raw JSON number text, borrowed from the frame.
// The scales are only known at runtime, so `try_from_wire` turns them into units.
#[derive(Deserialize)]
pub(super) struct Order<'a> {
    #[serde(deserialize_with = "deserialize_order_id")]
    pub order_id: OrderId,
    #[serde(borrow)]
    pub limit_price: &'a RawValue,
    #[serde(borrow)]
    pub order_qty: &'a RawValue,
    #[serde(deserialize_with = "deserialize_ts")]
    pub timestamp: Ts,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum EventType {
    Add,
    Modify,
    Delete,
}

#[derive(Deserialize)]
pub(super) struct OrderEvent<'a> {
    #[serde(deserialize_with = "deserialize_order_id")]
    pub order_id: OrderId,
    #[serde(borrow)]
    pub limit_price: &'a RawValue,
    #[serde(borrow)]
    pub order_qty: &'a RawValue,
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
        Err(serde::de::Error::invalid_length(
            s.len(),
            &"a 19-byte order id",
        ))
    } else {
        let mut ret: OrderId = [0; _];
        let n = s.len().min(ret.len());
        ret[..n].copy_from_slice(&s.as_bytes()[..n]);
        Ok(ret)
    }
}
