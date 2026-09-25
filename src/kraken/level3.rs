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
struct OrderEvent {
    order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    order_qty: Decimal,
    timestamp: DateTime<Utc>,
    event: String,
}

#[derive(Deserialize)]
struct OrderBook<TOrder> {
    symbol: String,
    checksum: u32,
    timestamp: DateTime<Utc>,
    bids: Vec<TOrder>,
    asks: Vec<TOrder>,
}

type OrderBookSnapshot = OrderBook<Order>;
type OrderBookUpdate = OrderBook<OrderEvent>;

struct OrderBookUpdater {
    order_book: OrderBookSnapshot,
    max_depth: usize,
}

impl<'de, TOrder: Deserialize<'de>> OrderBook<TOrder> {
    fn from_json_str(raw: &'de str) -> Result<OrderBook<TOrder>, Box<dyn Error>> {
        Ok(serde_json::from_str(raw)?)
    }
}

impl OrderBookSnapshot {
    fn calculate_checksum(&self) -> u32 {
        let mut buff = String::with_capacity(self.asks.len() * 6 * 2 + self.bids.len() * 6 * 2);
        for order in self.asks.iter().chain(self.bids.iter()) {
            buff.push_str(&serialize_decimal(&order.limit_price));
            buff.push_str(&serialize_decimal(&order.order_qty));
        }
        crc32fast::hash(buff.as_bytes())
    }
}

impl OrderBookUpdater {
    fn new(snapshot: OrderBookSnapshot, max_depth: usize) -> Self {
        OrderBookUpdater {
            order_book: snapshot,
            max_depth,
        }
    }

    fn apply(&mut self, update: OrderBookUpdate) -> Result<(), Box<dyn Error>> {
        if self.order_book.timestamp > update.timestamp {
            return Err("Update is old".into());
        }

        self.order_book.timestamp = update.timestamp;
        self.order_book.checksum = update.checksum;
        update_helper(self.order_book.bids.as_mut(), update.bids, false)?;
        update_helper(self.order_book.asks.as_mut(), update.asks, true)?;
        truncate_price_level(self.order_book.bids.as_mut(), self.max_depth);
        Ok(())
    }
}

fn truncate_price_level(orders: &mut Vec<Order>, max_depth: usize) {
    if orders.len() <= max_depth {
        return;
    }

    // Find the truncate position.
    let mut level: (Decimal, usize) = (orders[0].limit_price, 0);
    let mut truncate_pos: Option<usize> = None;
    for (index, order) in orders.iter().enumerate() {
        if order.limit_price != level.0 {
            // Update the price level.
            level.0 = order.limit_price;
            level.1 += 1;
        }
        // Stop once we reached the truncate position.
        if level.1 > max_depth {
            truncate_pos = Some(index);
            break;
        }
    }
    if let Some(truncate_pos) = truncate_pos {
        orders.drain(truncate_pos..);
    }
}

fn serialize_decimal(value: &Decimal) -> String {
    let mut value = value.to_string().replace(".", "");
    // Remove leading zeroes.
    while value.starts_with('0') {
        value.remove(0);
    }
    value
}

fn update_helper(
    orders: &mut Vec<Order>,
    order_events: Vec<OrderEvent>,
    sort_asc: bool,
) -> Result<(), Box<dyn Error>> {
    for order_event in order_events {
        match order_event.event.as_str() {
            "add" => {
                let order = Order {
                    order_id: order_event.order_id,
                    limit_price: order_event.limit_price,
                    order_qty: order_event.order_qty,
                    timestamp: order_event.timestamp,
                };

                // Orders are sorted by limit_price.
                let idx = orders.partition_point(|o| match sort_asc {
                    true => o.limit_price <= order.limit_price,
                    false => o.limit_price >= order.limit_price,
                });
                orders.insert(idx, order);
            }
            "delete" => {
                match orders
                    .iter()
                    .position(|o| o.order_id == order_event.order_id)
                {
                    Some(index) => {
                        orders.remove(index);
                    }
                    None => {
                        return Err(format!("Order not found {}", order_event.order_id).into());
                    }
                }
            }
            "modify" => {
                match orders
                    .iter()
                    .position(|o| o.order_id == order_event.order_id)
                {
                    Some(index) => {
                        orders[index].order_qty = order_event.order_qty;
                    }
                    None => {
                        return Err(format!("Order not found {}", order_event.order_id).into());
                    }
                }
            }
            _ => {
                return Err(format!("Unknown event {}", order_event.event).into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_plus_update() {
        // Load and check snapshot.
        let snapshot = OrderBookSnapshot::from_json_str(
            &std::fs::read_to_string("tests/data/level3_snapshot.json").unwrap(),
        )
        .unwrap();

        assert_eq!(snapshot.symbol, String::from("BTC/USD"));
        assert_eq!(snapshot.checksum, 30690248);
        assert_eq!(snapshot.calculate_checksum(), 30690248);
        assert_eq!(
            snapshot.timestamp.to_string(),
            "2026-09-15 21:28:25.745828658 UTC"
        );
        assert_eq!(snapshot.asks.len(), 18);
        assert_eq!(snapshot.bids.len(), 13);

        // Load and check update.
        let update = OrderBookUpdate::from_json_str(
            &std::fs::read_to_string("tests/data/level3_update_1.json").unwrap(),
        )
        .unwrap();

        assert_eq!(update.symbol, String::from("BTC/USD"));
        assert_eq!(update.checksum, 782538988);
        assert_eq!(
            update.timestamp.to_string(),
            "2026-09-15 21:28:25.964171032 UTC"
        );
        assert_eq!(update.asks.len(), 0);
        assert_eq!(update.bids.len(), 3);
        assert_eq!(update.bids[0].event, "delete");

        // Update snapshot with update.
        let mut updater = OrderBookUpdater::new(snapshot, 10);
        updater.apply(update).unwrap();

        assert_eq!(updater.order_book.checksum, 782538988);
        assert_eq!(updater.order_book.calculate_checksum(), 782538988);
        assert_eq!(
            updater.order_book.timestamp.to_string(),
            "2026-09-15 21:28:25.964171032 UTC"
        );
        assert_eq!(updater.order_book.asks.len(), 18);
        assert_eq!(updater.order_book.bids.len(), 14);
    }
}
