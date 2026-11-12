use super::units::to_units;
use super::{order_book, wire};
use crate::common::ts::Ts;
use rust_decimal::Decimal;
use uuid::Uuid;

pub(super) fn random_order_id() -> wire::OrderId {
    // OrderId is a fixed-size byte array, so take the first bytes of a random hex string.
    let hex = Uuid::new_v4().simple().to_string();
    hex.as_bytes()[..size_of::<wire::OrderId>()]
        .try_into()
        .expect("order id length")
}

pub(super) fn build_order(
    limit_price: Decimal,
    price_scale: u8,
    qty_scale: u8,
) -> order_book::Order {
    order_book::Order {
        order_id: random_order_id(),
        limit_price: to_units(limit_price, price_scale).expect("price units"),
        order_qty: to_units(Decimal::from(1), qty_scale).expect("qty units"),
        timestamp: Ts::try_from(chrono::Utc::now()).unwrap(),
    }
}

pub(super) fn build_order_event(
    event: wire::EventType,
    limit_price: Decimal,
    price_scale: u8,
    qty_scale: u8,
) -> order_book::OrderEvent {
    order_book::OrderEvent {
        order_id: random_order_id(),
        limit_price: to_units(limit_price, price_scale).expect("price units"),
        order_qty: to_units(Decimal::from(1), qty_scale).expect("qty units"),
        timestamp: Ts::try_from(chrono::Utc::now()).unwrap(),
        event,
    }
}
