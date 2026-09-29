use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::error::Error;
use std::path::{Path, PathBuf};

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
        // Use at most 10 levels when calculating the checksum.
        let max_level = 10;

        let ask_count =
            find_price_level(&self.asks, max_level + 1).map_or(self.asks.len(), |i| i - 1);
        let bid_count =
            find_price_level(&self.bids, max_level + 1).map_or(self.bids.len(), |i| i - 1);

        let mut buff = String::with_capacity(max_level * 6 * 2 + max_level * 6 * 2);
        for order in self
            .asks
            .iter()
            .take(ask_count)
            .chain(self.bids.iter().take(bid_count))
        {
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
        update_helper(self.order_book.asks.as_mut(), update.asks, true)?;
        update_helper(self.order_book.bids.as_mut(), update.bids, false)?;
        truncate_price_level(self.order_book.asks.as_mut(), self.max_depth);
        truncate_price_level(self.order_book.bids.as_mut(), self.max_depth);
        Ok(())
    }
}

fn find_price_level(orders: &[Order], level: usize) -> Option<usize> {
    // Returns the position for the 1st order at the given price level.

    if level >= orders.len() {
        return None;
    }

    // Find the price leel.
    let mut price_level: (Decimal, usize) = (orders[0].limit_price, 0);
    for (index, order) in orders.iter().enumerate() {
        if order.limit_price != price_level.0 {
            // Update the price level.
            price_level.0 = order.limit_price;
            price_level.1 += 1;
        }
        // Stop once we reached the price level.
        if price_level.1 == level {
            return Some(index);
        }
    }
    None
}

fn truncate_price_level(orders: &mut Vec<Order>, max_levels: usize) {
    // Truncates orders leaving only the first max_levels.
    if let Some(truncate_pos) = find_price_level(orders, max_levels) {
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

    fn data_file_path(fname: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(fname)
    }

    #[test]
    fn find_price_levels() {
        // Load and check the snapshot.
        let snapshot = OrderBookSnapshot::from_json_str(
            &std::fs::read_to_string(data_file_path("level3_snapshot.json")).unwrap(),
        )
        .unwrap();

        assert_eq!(find_price_level(&snapshot.bids, 0), Some(0));
        assert_eq!(find_price_level(&snapshot.bids, 1), Some(4));
        assert_eq!(find_price_level(&snapshot.bids, 9), Some(12));
        assert_eq!(find_price_level(&snapshot.bids, 10), None);

        let empty_vec: Vec<Order> = Vec::new();
        assert_eq!(find_price_level(&empty_vec, 0), None);
        assert_eq!(find_price_level(&empty_vec, 1), None);
    }

    #[test]
    fn truncate_price_levels() {
        let now = Utc::now();
        let mid_price = Decimal::from(1000);
        let qty = Decimal::from(1);
        let mut bids: Vec<Order> = (0..10)
            .into_iter()
            .map(|i| Order {
                order_id: format!("buy_{i}"),
                limit_price: mid_price - Decimal::from(i),
                order_qty: qty,
                timestamp: now,
            })
            .collect();

        assert_eq!(bids.len(), 10);
        assert_eq!(bids[0].limit_price, Decimal::from(1000));
        assert_eq!(bids[9].limit_price, Decimal::from(991));

        // Nothing should change.
        truncate_price_level(bids.as_mut(), 10);

        assert_eq!(bids.len(), 10);
        assert_eq!(bids[0].limit_price, Decimal::from(1000));
        assert_eq!(bids[9].limit_price, Decimal::from(991));

        // Just one level removed.
        truncate_price_level(bids.as_mut(), 9);

        assert_eq!(bids.len(), 9);
        assert_eq!(bids[0].limit_price, Decimal::from(1000));
        assert_eq!(bids[8].limit_price, Decimal::from(992));

        // Leave only the first level.
        truncate_price_level(bids.as_mut(), 1);

        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].limit_price, Decimal::from(1000));

        // No levels at all.
        truncate_price_level(bids.as_mut(), 0);

        assert_eq!(bids.len(), 0);
    }

    #[test]
    fn truncate_order_book_snapshot() {
        let now = Utc::now();
        let mid_price = Decimal::from(1000);
        let qty = Decimal::from(1);

        let mut bids: Vec<Order> = (0..14)
            .into_iter()
            .map(|i| Order {
                order_id: format!("buy_{i}"),
                limit_price: mid_price - Decimal::from(i + 1),
                order_qty: qty,
                timestamp: now,
            })
            .collect();
        let mut asks: Vec<Order> = (0..14)
            .into_iter()
            .map(|i| Order {
                order_id: format!("sell_{i}"),
                limit_price: mid_price + Decimal::from(i + 1),
                order_qty: qty,
                timestamp: now,
            })
            .collect();

        let snapshot = OrderBookSnapshot {
            symbol: String::from("BTC/USD"),
            checksum: 0,
            timestamp: now,
            bids,
            asks,
        };
        assert_eq!(snapshot.asks.len(), 14);
        assert_eq!(snapshot.bids.len(), 14);
        assert_eq!(snapshot.asks[13].limit_price, Decimal::from(1014));
        assert_eq!(snapshot.asks[0].limit_price, Decimal::from(1001));
        assert_eq!(snapshot.bids[0].limit_price, Decimal::from(999));
        assert_eq!(snapshot.bids[13].limit_price, Decimal::from(986));
        assert_eq!(snapshot.calculate_checksum(), 2215799079);

        // Apply an empty update just to get the orderbook truncated.
        let mut updater = OrderBookUpdater::new(snapshot, 10);
        let update = OrderBookUpdate {
            symbol: String::from("BTC/USD"),
            checksum: 0,
            timestamp: now,
            bids: Vec::new(),
            asks: Vec::new(),
        };
        updater.apply(update).unwrap();

        assert_eq!(updater.order_book.asks.len(), 10);
        assert_eq!(updater.order_book.bids.len(), 10);
        assert_eq!(updater.order_book.asks[9].limit_price, Decimal::from(1010));
        assert_eq!(updater.order_book.asks[0].limit_price, Decimal::from(1001));
        assert_eq!(updater.order_book.bids[0].limit_price, Decimal::from(999));
        assert_eq!(updater.order_book.bids[9].limit_price, Decimal::from(990));
        assert_eq!(updater.order_book.calculate_checksum(), 2215799079);
    }

    #[test]
    fn snapshot_plus_update() {
        // Load and check the snapshot.
        let snapshot = OrderBookSnapshot::from_json_str(
            &std::fs::read_to_string(data_file_path("level3_snapshot.json")).unwrap(),
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

        // Load and check the update.
        let update = OrderBookUpdate::from_json_str(
            &std::fs::read_to_string(data_file_path("level3_update_1.json")).unwrap(),
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

        // Apply the update to the snapshot.
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
