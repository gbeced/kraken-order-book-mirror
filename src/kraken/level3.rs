use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::error::Error;

// https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/level3

#[derive(Deserialize)]
struct OrderBookMsg<TOrderBook> {
    data: Vec<TOrderBook>,
}

#[derive(Deserialize, Debug)]
pub struct OrderBook<TOrder> {
    // pub symbol: String,
    pub checksum: u32,
    pub timestamp: DateTime<Utc>,
    pub bids: Vec<TOrder>,
    pub asks: Vec<TOrder>,
}

#[derive(Deserialize, Debug)]
pub struct Order {
    pub order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    pub order_qty: Decimal,
    pub timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EventType {
    Add,
    Modify,
    Delete,
}

#[derive(Deserialize)]
struct OrderEvent {
    order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    order_qty: Decimal,
    timestamp: DateTime<Utc>,
    event: EventType,
}

type OrderBookSnapshot = OrderBook<Order>;
type OrderBookUpdate = OrderBook<OrderEvent>;
type OrderBookSnapshotMsg = OrderBookMsg<OrderBookSnapshot>;
type OrderBookUpdateMsg = OrderBookMsg<OrderBookUpdate>;

struct OrderBookUpdater {
    max_depth: usize,
    snapshot: OrderBookSnapshot,
}

enum MirrorState {
    WaitingSnapshot,
    WaitingUpdate { updater: OrderBookUpdater },
    Invalid,
}

struct MirrorContext {
    max_depth: usize,
}

pub struct OrderBookMirror {
    context: MirrorContext,
    state: MirrorState,
}

impl OrderBookSnapshot {
    fn calculate_checksum(&self) -> u32 {
        // https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2

        // Kraken always hashes the top 10 price levels, whatever depth was subscribed.
        const CHECKSUM_PRICE_LEVELS: usize = 10;
        let ask_count =
            find_price_level(&self.asks, CHECKSUM_PRICE_LEVELS).unwrap_or(self.asks.len());
        let bid_count =
            find_price_level(&self.bids, CHECKSUM_PRICE_LEVELS).unwrap_or(self.bids.len());

        let mut buff = String::with_capacity((ask_count + bid_count) * 6 * 2);
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
    fn update(&mut self, update: OrderBookUpdate) -> Result<(), Box<dyn Error>> {
        if self.snapshot.timestamp > update.timestamp {
            return Err("Update is old".into());
        }

        // If any of these fail, the order book ends up in an invalid state.
        self.snapshot.timestamp = update.timestamp;
        self.snapshot.checksum = update.checksum;
        update_orders(self.snapshot.asks.as_mut(), update.asks, true)?;
        update_orders(self.snapshot.bids.as_mut(), update.bids, false)?;
        truncate_price_level(self.snapshot.asks.as_mut(), self.max_depth);
        truncate_price_level(self.snapshot.bids.as_mut(), self.max_depth);

        let calculated = self.snapshot.calculate_checksum();
        if calculated != self.snapshot.checksum {
            return Err(format!(
                "Checksum mismatch after update. Calculated: {}. Update: {}",
                calculated, self.snapshot.checksum
            )
            .into());
        }
        Ok(())
    }
}

impl MirrorState {
    fn on_snapshot(
        &mut self,
        context: &MirrorContext,
        snapshot: OrderBookSnapshot,
    ) -> Result<(), Box<dyn Error>> {
        match self {
            MirrorState::WaitingSnapshot => {
                *self = MirrorState::WaitingUpdate {
                    updater: OrderBookUpdater {
                        max_depth: context.max_depth,
                        snapshot,
                    },
                };
                Ok(())
            }
            MirrorState::WaitingUpdate { .. } => {
                *self = MirrorState::Invalid;
                Err("Expected update but received snapshot".into())
            }
            MirrorState::Invalid => Err("Received snapshot in an invalid state".into()),
        }
    }

    fn on_update(
        &mut self,
        _context: &MirrorContext,
        update: OrderBookUpdate,
    ) -> Result<(), Box<dyn Error>> {
        match self {
            MirrorState::WaitingSnapshot => {
                *self = MirrorState::Invalid;
                Err("Expected snapshot but received update".into())
            }
            MirrorState::WaitingUpdate { updater } => {
                updater.update(update).inspect_err(|_| {
                    // If update failed then the order book is now in an invalid state and we should drop it.
                    *self = MirrorState::Invalid;
                })
            }
            MirrorState::Invalid => Err("Received update in an invalid state".into()),
        }
    }
}

impl OrderBookMirror {
    pub fn new(max_depth: usize) -> OrderBookMirror {
        OrderBookMirror {
            context: MirrorContext { max_depth },
            state: MirrorState::WaitingSnapshot,
        }
    }

    pub fn on_message(&mut self, msg: &str) -> Result<(), Box<dyn Error>> {
        if let Ok(update_msg) = serde_json::from_str::<OrderBookUpdateMsg>(msg) {
            self.process_update(update_msg)
        } else if let Ok(snapshot_msg) = serde_json::from_str::<OrderBookSnapshotMsg>(msg) {
            self.process_snapshot(snapshot_msg)
        } else {
            Err("Unexpected message".into())
        }
    }

    pub fn get_order_book(&self) -> Option<&OrderBookSnapshot> {
        match self.state {
            MirrorState::WaitingUpdate { ref updater } => Some(&updater.snapshot),
            _ => None,
        }
    }

    #[cold]
    fn process_snapshot(
        &mut self,
        snapshot_msg: OrderBookSnapshotMsg,
    ) -> Result<(), Box<dyn Error>> {
        snapshot_msg
            .data
            .into_iter()
            .try_for_each(|snapshot| self.state.on_snapshot(&self.context, snapshot))
    }

    fn process_update(&mut self, update_msg: OrderBookUpdateMsg) -> Result<(), Box<dyn Error>> {
        update_msg
            .data
            .into_iter()
            .try_for_each(|update| self.state.on_update(&self.context, update))
    }
}

fn insert_order(orders: &mut Vec<Order>, event: OrderEvent, sort_asc: bool) {
    let order = Order {
        order_id: event.order_id,
        limit_price: event.limit_price,
        order_qty: event.order_qty,
        timestamp: event.timestamp,
    };

    // Orders are sorted by limit_price (asc/desc) + timestamp(asc).
    let idx = orders.partition_point(|o| match sort_asc {
        true => {
            o.limit_price < order.limit_price
                || o.limit_price == order.limit_price && o.timestamp < order.timestamp
        }
        false => {
            o.limit_price > order.limit_price
                || o.limit_price == order.limit_price && o.timestamp < order.timestamp
        }
    });
    orders.insert(idx, order);
}

fn find_order<'a>(
    orders: &'a mut [Order],
    event: &OrderEvent,
    sort_asc: bool,
) -> Option<(usize, &'a mut Order)> {
    // Orders are sorted by limit_price (asc/desc) + timestamp(asc).
    // First we locate the price level using a binary search first.
    let idx = orders.partition_point(|o| match sort_asc {
        true => o.limit_price < event.limit_price,
        false => o.limit_price > event.limit_price,
    });
    // Then we do a linear search within the price level.
    for (idx, order) in (idx..).zip(orders.iter_mut().skip(idx)) {
        if order.order_id == event.order_id {
            return Some((idx, order));
        } else if order.limit_price != event.limit_price {
            return None;
        }
    }
    None
}

fn modify_order(orders: &mut Vec<Order>, event: OrderEvent, sort_asc: bool) -> bool {
    if let Some((mut idx, order)) = find_order(orders, &event, sort_asc) {
        if order.timestamp == event.timestamp {
            // Update the qty in place.
            order.order_qty = event.order_qty;
        } else {
            // Remove the order and continue doing a linear search for the insertion point within the price level, or
            // at the end of it.
            assert!(event.timestamp > order.timestamp);
            orders.remove(idx);
            for order in orders.iter().skip(idx) {
                if event.timestamp < order.timestamp || order.limit_price != event.limit_price {
                    break;
                }
                idx += 1;
            }

            let order = Order {
                order_id: event.order_id,
                limit_price: event.limit_price,
                order_qty: event.order_qty,
                timestamp: event.timestamp,
            };
            orders.insert(idx, order);
        }
        true
    } else {
        false
    }
}

fn remove_order(orders: &mut Vec<Order>, event: &OrderEvent, sort_asc: bool) -> bool {
    if let Some((idx, _)) = find_order(orders, event, sort_asc) {
        orders.remove(idx);
        true
    } else {
        false
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

fn update_orders(
    orders: &mut Vec<Order>,
    order_events: Vec<OrderEvent>,
    sort_asc: bool,
) -> Result<(), Box<dyn Error>> {
    for order_event in order_events {
        match order_event.event {
            // A new order added to the book.
            EventType::Add => {
                insert_order(orders, order_event, sort_asc);
            }
            // The order has been removed from the book, i.e. full fill or cancel.
            EventType::Delete => {
                if !remove_order(orders, &order_event, sort_asc) {
                    return Err(format!("Order not found {}", order_event.order_id).into());
                }
            }
            // The order quantity has been modified, i.e. a fill.
            EventType::Modify => {
                if !modify_order(orders, order_event, sort_asc) {
                    return Err("Order not found".into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    fn test_file_path(fname: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(fname)
    }

    fn read_test_file(fname: &str) -> String {
        let path = test_file_path(fname);
        std::fs::read_to_string(path).unwrap()
    }

    fn open_test_file(fname: &str) -> BufReader<File> {
        let path = test_file_path(fname);
        let file = File::open(path).unwrap();
        BufReader::new(file)
    }

    fn build_order(limit_price: Decimal) -> Order {
        Order {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price,
            order_qty: Decimal::from(1),
            timestamp: Utc::now(),
        }
    }

    fn build_order_event(limit_price: Decimal) -> OrderEvent {
        OrderEvent {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price,
            order_qty: Decimal::from(1),
            timestamp: Utc::now(),
            event: EventType::Add,
        }
    }

    #[test]
    fn find_price_levels() {
        // Load and check the snapshot.
        let snapshot_msg: OrderBookSnapshotMsg =
            serde_json::from_str(&read_test_file("level3_snapshot.json")).unwrap();
        let snapshot = &snapshot_msg.data[0];

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
        let mid_price = Decimal::from(1000);
        let mut bids: Vec<Order> = (0..10)
            .into_iter()
            .map(|i| build_order(mid_price - Decimal::from(i)))
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
    fn truncate_crowded_price_levels() {
        // Several orders share a price.
        let mut crowded = Vec::new();
        for (price, count) in [(100, 2), (90, 1), (80, 2)] {
            for _ in 0..count {
                crowded.push(build_order(Decimal::from(price)));
            }
        }
        truncate_price_level(crowded.as_mut(), 2);

        assert_eq!(crowded.len(), 3);
        assert_eq!(crowded[2].limit_price, Decimal::from(90));

        truncate_price_level(crowded.as_mut(), 1);

        assert_eq!(crowded.len(), 2);
        assert_eq!(crowded[1].limit_price, Decimal::from(100));
    }

    #[test]
    fn order_book_updater_truncating_update() {
        let first_ask = Decimal::from(1010);
        let first_bid = Decimal::from(1000);
        let bids: Vec<Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_bid - Decimal::from(i)))
            .collect();
        let asks: Vec<Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_ask + Decimal::from(i)))
            .collect();
        let snapshot = OrderBookSnapshot {
            checksum: 1913449638,
            timestamp: Utc::now(),
            bids,
            asks,
        };

        assert_eq!(snapshot.asks.len(), 10);
        assert_eq!(snapshot.bids.len(), 10);
        assert_eq!(
            snapshot.asks[9].limit_price,
            Decimal::from(first_ask) + Decimal::from(19)
        );
        assert_eq!(
            snapshot.asks[0].limit_price,
            Decimal::from(first_ask) + Decimal::from(10)
        );
        assert_eq!(
            snapshot.bids[0].limit_price,
            Decimal::from(first_bid) - Decimal::from(10)
        );
        assert_eq!(
            snapshot.bids[9].limit_price,
            Decimal::from(first_bid) - Decimal::from(19)
        );
        assert_eq!(snapshot.calculate_checksum(), 1913449638);

        // This update should end up being ignored since all levels should get inserted past the end.
        let bids: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| build_order_event(first_bid - Decimal::from(i)))
            .collect();
        let asks: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| build_order_event(first_ask + Decimal::from(i)))
            .collect();
        let update = OrderBookUpdate {
            checksum: 1913449638,
            timestamp: Utc::now(),
            bids,
            asks,
        };
        let mut updater = OrderBookUpdater {
            max_depth: 10,
            snapshot,
        };
        updater.update(update).unwrap();
        let snapshot = &updater.snapshot;

        assert_eq!(snapshot.asks.len(), 10);
        assert_eq!(snapshot.bids.len(), 10);
        assert_eq!(
            snapshot.asks[9].limit_price,
            Decimal::from(first_ask) + Decimal::from(19)
        );
        assert_eq!(
            snapshot.asks[0].limit_price,
            Decimal::from(first_ask) + Decimal::from(10)
        );
        assert_eq!(
            snapshot.bids[0].limit_price,
            Decimal::from(first_bid) - Decimal::from(10)
        );
        assert_eq!(
            snapshot.bids[9].limit_price,
            Decimal::from(first_bid) - Decimal::from(19)
        );
        assert_eq!(snapshot.calculate_checksum(), 1913449638);

        // This update should get the snapshot truncated since 5 levels should have been inserted right at the
        // beginning.
        let bids: Vec<OrderEvent> = (0..5)
            .into_iter()
            .map(|i| build_order_event(first_bid - Decimal::from(i)))
            .collect();
        let asks: Vec<OrderEvent> = (0..5)
            .into_iter()
            .map(|i| build_order_event(first_ask + Decimal::from(i)))
            .collect();
        let update = OrderBookUpdate {
            checksum: 3473600046,
            timestamp: Utc::now(),
            bids,
            asks,
        };
        updater.update(update).unwrap();
        let snapshot = &updater.snapshot;

        assert_eq!(snapshot.asks.len(), 10);
        assert_eq!(snapshot.bids.len(), 10);
        assert_eq!(
            snapshot.asks[9].limit_price,
            Decimal::from(first_ask) + Decimal::from(14)
        );
        assert_eq!(snapshot.asks[0].limit_price, Decimal::from(first_ask));
        assert_eq!(snapshot.bids[0].limit_price, Decimal::from(first_bid));
        assert_eq!(
            snapshot.bids[9].limit_price,
            Decimal::from(first_bid) - Decimal::from(14)
        );
        assert_eq!(snapshot.calculate_checksum(), 3473600046);
    }

    #[test]
    fn mirror_processing_messages() {
        let mut mirror = OrderBookMirror::new(10);
        let file_names = [
            "level3_snapshot.json",
            "level3_update_1.json",
            "level3_update_2.json",
        ];

        for file_name in file_names {
            mirror.on_message(&read_test_file(file_name)).unwrap();
        }
    }

    #[test]
    fn mirror_processing_fails_with_snapshot_expected() {
        let mut mirror = OrderBookMirror::new(10);

        assert_eq!(
            mirror
                .on_message(&read_test_file("level3_update_1.json"))
                .expect_err("Error expected")
                .to_string(),
            "Expected snapshot but received update"
        );
    }

    #[test]
    fn mirror_processing_fails_with_update_expected() {
        let mut mirror = OrderBookMirror::new(10);

        mirror
            .on_message(&read_test_file("level3_snapshot.json"))
            .unwrap();
        assert_eq!(
            mirror
                .on_message(&read_test_file("level3_snapshot.json"))
                .expect_err("Error expected")
                .to_string(),
            "Expected update but received snapshot"
        );
    }

    #[test]
    fn mirror_checksum_mismatch_after_update() {
        let mut mirror = OrderBookMirror::new(10);
        let mut reader = open_test_file("btc_usd_stream.log");
        let mut line = String::new();

        while reader.read_line(&mut line).unwrap() > 0 {
            if line.len() > 0 {
                mirror.on_message(&line).unwrap();
            }
            line.clear();
        }

        let snapshot = mirror.get_order_book().expect("No order book");
        assert_eq!(snapshot.checksum, 1906173362);
        assert_eq!(snapshot.calculate_checksum(), 1906173362);
    }

    #[test]
    fn insert_and_delete_order() {
        let mut orders: Vec<Order> = (0..3)
            .into_iter()
            .map(|i| build_order(Decimal::from(1) + Decimal::from(i)))
            .collect();

        for value in ["0.5", "1.5", "3"] {
            let event = build_order_event(Decimal::from_str_exact(value).unwrap());
            insert_order(&mut orders, event, true);
        }
        assert_eq!(
            orders.get(0).unwrap().limit_price,
            Decimal::from_str_exact("0.5").unwrap()
        );
        assert_eq!(
            orders.get(2).unwrap().limit_price,
            Decimal::from_str_exact("1.5").unwrap()
        );
        assert_eq!(
            orders.get(4).unwrap().limit_price,
            Decimal::from_str_exact("3").unwrap()
        );

        // Remove the 2nd order.
        let order_to_delete = orders.get(2).unwrap();
        let event = OrderEvent {
            order_id: order_to_delete.order_id.clone(),
            limit_price: order_to_delete.limit_price.clone(),
            order_qty: order_to_delete.order_qty.clone(),
            timestamp: Utc::now(),
            event: EventType::Delete,
        };
        assert!(remove_order(&mut orders, &event, true));
    }
}
