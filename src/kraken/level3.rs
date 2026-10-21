use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::cell::RefCell;
use std::error::Error;
use std::ops::Index;

// https://docs.kraken.com/exchange/api-reference/spot-websocket-v2/level3

#[derive(Deserialize)]
struct Envelope<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct Payload<T> {
    // symbol: String,
    checksum: u32,
    timestamp: DateTime<Utc>,
    bids: Vec<T>,
    asks: Vec<T>,
}

#[derive(Deserialize)]
struct WireOrder {
    order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    order_qty: Decimal,
    timestamp: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum EventType {
    Add,
    Modify,
    Delete,
}

#[derive(Deserialize)]
struct WireOrderEvent {
    order_id: String,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    limit_price: Decimal,
    #[serde(with = "rust_decimal::serde::arbitrary_precision")]
    order_qty: Decimal,
    timestamp: DateTime<Utc>,
    event: EventType,
}

type Snapshot = Payload<Order>;
type Update = Payload<OrderEvent>;
type SnapshotMsg = Envelope<Payload<WireOrder>>;
type UpdateMsg = Envelope<Payload<WireOrderEvent>>;

pub struct Order {
    pub order_id: String,
    pub limit_price: u64,
    pub order_qty: u64,
    pub timestamp: DateTime<Utc>,
}

struct OrderEvent {
    order_id: String,
    limit_price: u64,
    order_qty: u64,
    timestamp: DateTime<Utc>,
    event: EventType,
}

// Orders are sorted by limit_price (asc/desc) + timestamp(asc).
pub struct Orders {
    orders: Vec<Order>,
    sort_asc: bool,
}

pub struct OrderBook {
    pub asks: Orders,
    pub bids: Orders,
    pub timestamp: DateTime<Utc>,
    price_scale: u8,
    qty_scale: u8,
}

struct OrderBookUpdater {
    order_book: OrderBook,
    max_depth: usize,
    serializer_buff: RefCell<String>,
}

enum MirrorState {
    WaitingSnapshot,
    WaitingUpdate { updater: OrderBookUpdater },
    Invalid,
}

struct MirrorContext {
    max_depth: usize,
    price_scale: u8,
    qty_scale: u8,
}

pub struct OrderBookMirror {
    context: MirrorContext,
    state: MirrorState,
}

impl Order {
    fn from_event(event: OrderEvent) -> Order {
        // Not using From trait because the general expectation is that the conversion is lossless.
        Order {
            order_id: event.order_id,
            limit_price: event.limit_price,
            order_qty: event.order_qty,
            timestamp: event.timestamp,
        }
    }
}

impl Orders {
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    pub fn iter<'a>(&'a self) -> std::slice::Iter<'a, Order> {
        self.orders.iter()
    }

    fn insert_order(&mut self, order: Order) {
        let idx = self.orders.partition_point(|o| match self.sort_asc {
            true => {
                o.limit_price < order.limit_price
                    || o.limit_price == order.limit_price && o.timestamp < order.timestamp
            }
            false => {
                o.limit_price > order.limit_price
                    || o.limit_price == order.limit_price && o.timestamp < order.timestamp
            }
        });
        self.orders.insert(idx, order);
    }

    fn remove_order(&mut self, order_id: &str, limit_price: u64) -> bool {
        if let Some((idx, _)) = self.find_order(order_id, limit_price) {
            self.orders.remove(idx);
            true
        } else {
            false
        }
    }

    fn find_order<'a>(
        &'a mut self,
        order_id: &str,
        limit_price: u64,
    ) -> Option<(usize, &'a mut Order)> {
        // Orders are sorted by limit_price (asc/desc) + timestamp(asc).
        // First we locate the price level using a binary search first.
        let idx = self.orders.partition_point(|o| match self.sort_asc {
            true => o.limit_price < limit_price,
            false => o.limit_price > limit_price,
        });
        // Then we do a linear search within the price level.
        for (idx, order) in (idx..).zip(self.orders.iter_mut().skip(idx)) {
            if order.order_id == order_id {
                return Some((idx, order));
            } else if order.limit_price != limit_price {
                return None;
            }
        }
        None
    }

    fn modify_order(&mut self, order: Order) -> bool {
        if let Some((mut idx, prev_order)) = self.find_order(&order.order_id, order.limit_price) {
            if prev_order.timestamp == order.timestamp {
                // Update the qty in place.
                prev_order.order_qty = order.order_qty;
            } else {
                // Remove the order and continue doing a linear search for the insertion point within the price level, or
                // at the end of it.
                assert!(order.timestamp > prev_order.timestamp);
                self.orders.remove(idx);
                for existing in self.orders.iter().skip(idx) {
                    if order.timestamp < existing.timestamp
                        || existing.limit_price != order.limit_price
                    {
                        break;
                    }
                    idx += 1;
                }

                self.orders.insert(idx, order);
            }
            true
        } else {
            false
        }
    }

    fn apply_events(&mut self, events: Vec<OrderEvent>) -> Result<(), Box<dyn Error>> {
        for event in events {
            match event.event {
                // A new order added to the book.
                EventType::Add => {
                    self.insert_order(Order::from_event(event));
                }
                // The order has been removed from the book, i.e. full fill or cancel.
                EventType::Delete => {
                    if !self.remove_order(&event.order_id, event.limit_price) {
                        return Err(format!("Order not found {}", event.order_id).into());
                    }
                }
                // The order quantity has been modified, i.e. a fill.
                EventType::Modify => {
                    if !self.modify_order(Order::from_event(event)) {
                        return Err("Order not found".into());
                    }
                }
            }
        }
        Ok(())
    }

    fn find_price_level(&self, level: usize) -> Option<usize> {
        // Returns the position for the 1st order at the given price level.

        if level >= self.orders.len() {
            return None;
        }

        // Find the price leel.
        let mut price_level: (u64, usize) = (self.orders[0].limit_price, 0);
        for (index, order) in self.orders.iter().enumerate() {
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

    fn truncate_price_level(&mut self, max_levels: usize) {
        // Truncates orders leaving only the first max_levels.
        if let Some(truncate_pos) = self.find_price_level(max_levels) {
            self.orders.drain(truncate_pos..);
        }
    }
}

impl Index<usize> for Orders {
    type Output = Order;

    fn index(&self, index: usize) -> &Self::Output {
        &self.orders[index]
    }
}

impl OrderBook {
    pub fn price_to_decimal(&self, price: u64) -> Decimal {
        Decimal::from_i128_with_scale(price as i128, u32::from(self.price_scale))
    }

    pub fn qty_to_decimal(&self, qty: u64) -> Decimal {
        Decimal::from_i128_with_scale(qty as i128, u32::from(self.qty_scale))
    }
}

impl OrderBookUpdater {
    fn new(
        snapshot: Snapshot,
        max_depth: usize,
        price_scale: u8,
        qty_scale: u8,
    ) -> OrderBookUpdater {
        let asks = Orders {
            orders: snapshot.asks,
            sort_asc: true,
        };
        let bids = Orders {
            orders: snapshot.bids,
            sort_asc: false,
        };
        let order_book = OrderBook {
            asks,
            bids,
            timestamp: snapshot.timestamp,
            price_scale,
            qty_scale,
        };
        OrderBookUpdater {
            order_book,
            max_depth,
            serializer_buff: RefCell::new(String::with_capacity(max_depth * 2 * 10)),
        }
    }

    fn update(&mut self, update: Update) -> Result<(), Box<dyn Error>> {
        // If any of these fail, the order book ends up in an invalid state.
        self.order_book.asks.apply_events(update.asks)?;
        self.order_book.asks.truncate_price_level(self.max_depth);
        self.order_book.bids.apply_events(update.bids)?;
        self.order_book.bids.truncate_price_level(self.max_depth);

        let calculated = self.calculate_checksum();
        if calculated != update.checksum {
            return Err(format!(
                "Checksum mismatch after update. Calculated: {}. Update: {}",
                calculated, update.checksum
            )
            .into());
        }
        self.order_book.timestamp = update.timestamp;
        Ok(())
    }

    pub fn get_order_book(&self) -> &OrderBook {
        &self.order_book
    }

    fn calculate_checksum(&self) -> u32 {
        let obook = self.get_order_book();

        // https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2
        // Kraken hashes only the top 10 price levels for the checksum.
        const CHECKSUM_PRICE_LEVELS: usize = 10;
        let ask_count = obook
            .asks
            .find_price_level(CHECKSUM_PRICE_LEVELS)
            .unwrap_or(obook.asks.len());
        let bid_count = obook
            .bids
            .find_price_level(CHECKSUM_PRICE_LEVELS)
            .unwrap_or(obook.bids.len());
        let mut buff = self.serializer_buff.borrow_mut();

        buff.clear();
        for order in obook
            .asks
            .iter()
            .take(ask_count)
            .chain(obook.bids.iter().take(bid_count))
        {
            append_checksum_units(order.limit_price, &mut buff);
            append_checksum_units(order.order_qty, &mut buff);
        }
        crc32fast::hash(buff.as_bytes())
    }
}

impl MirrorState {
    fn on_snapshot(
        &mut self,
        context: &MirrorContext,
        snapshot: Snapshot,
    ) -> Result<(), Box<dyn Error>> {
        match self {
            MirrorState::WaitingSnapshot => {
                *self = MirrorState::WaitingUpdate {
                    updater: OrderBookUpdater::new(
                        snapshot,
                        context.max_depth,
                        context.price_scale,
                        context.qty_scale,
                    ),
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
        update: Update,
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
    pub fn new(max_depth: usize, price_scale: u8, qty_scale: u8) -> OrderBookMirror {
        OrderBookMirror {
            context: MirrorContext {
                max_depth,
                price_scale,
                qty_scale,
            },
            state: MirrorState::WaitingSnapshot,
        }
    }

    pub fn on_message(&mut self, msg: &str) -> Result<(), Box<dyn Error>> {
        if let Ok(update_msg) = serde_json::from_str::<UpdateMsg>(msg) {
            self.process_update(update_msg)
        } else if let Ok(snapshot_msg) = serde_json::from_str::<SnapshotMsg>(msg) {
            self.process_snapshot(snapshot_msg)
        } else {
            Err("Unexpected message".into())
        }
    }

    pub fn get_order_book(&self) -> Option<&OrderBook> {
        match self.state {
            MirrorState::WaitingUpdate { ref updater } => Some(updater.get_order_book()),
            _ => None,
        }
    }

    #[cold]
    fn process_snapshot(&mut self, snapshot_msg: SnapshotMsg) -> Result<(), Box<dyn Error>> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        snapshot_msg.data.into_iter().try_for_each(|wire| {
            let snapshot = wire.try_into_snapshot(price_scale, qty_scale)?;
            self.state.on_snapshot(&self.context, snapshot)
        })
    }

    fn process_update(&mut self, update_msg: UpdateMsg) -> Result<(), Box<dyn Error>> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        update_msg.data.into_iter().try_for_each(|wire| {
            let update = wire.try_into_update(price_scale, qty_scale)?;
            self.state.on_update(&self.context, update)
        })
    }
}

fn decimal_to_units(value: Decimal, scale: u8) -> Result<u64, Box<dyn Error>> {
    if value.is_sign_negative() {
        return Err("negative value".into());
    }
    let scale = u32::from(scale);
    let mantissa = value.mantissa();
    let value_scale = value.scale();
    let units = if value_scale <= scale {
        let factor = 10i128
            .checked_pow(scale - value_scale)
            .ok_or_else(|| -> Box<dyn Error> { "scale factor overflow".into() })?;
        mantissa
            .checked_mul(factor)
            .ok_or_else(|| -> Box<dyn Error> { "unit count overflow".into() })?
    } else {
        let factor = 10i128
            .checked_pow(value_scale - scale)
            .ok_or_else(|| -> Box<dyn Error> { "scale factor overflow".into() })?;
        if mantissa % factor != 0 {
            return Err("value is not a multiple of the increment".into());
        }
        mantissa / factor
    };
    u64::try_from(units).map_err(|_| -> Box<dyn Error> { "unit count does not fit in u64".into() })
}

impl WireOrder {
    fn try_into_order(self, price_scale: u8, qty_scale: u8) -> Result<Order, Box<dyn Error>> {
        Ok(Order {
            order_id: self.order_id,
            limit_price: decimal_to_units(self.limit_price, price_scale)?,
            order_qty: decimal_to_units(self.order_qty, qty_scale)?,
            timestamp: self.timestamp,
        })
    }
}

impl WireOrderEvent {
    fn try_into_event(self, price_scale: u8, qty_scale: u8) -> Result<OrderEvent, Box<dyn Error>> {
        Ok(OrderEvent {
            order_id: self.order_id,
            limit_price: decimal_to_units(self.limit_price, price_scale)?,
            order_qty: decimal_to_units(self.order_qty, qty_scale)?,
            timestamp: self.timestamp,
            event: self.event,
        })
    }
}

impl Payload<WireOrder> {
    fn try_into_snapshot(self, price_scale: u8, qty_scale: u8) -> Result<Snapshot, Box<dyn Error>> {
        Ok(Payload {
            checksum: self.checksum,
            timestamp: self.timestamp,
            bids: self
                .bids
                .into_iter()
                .map(|raw| raw.try_into_order(price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
            asks: self
                .asks
                .into_iter()
                .map(|raw| raw.try_into_order(price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl Payload<WireOrderEvent> {
    fn try_into_update(self, price_scale: u8, qty_scale: u8) -> Result<Update, Box<dyn Error>> {
        Ok(Payload {
            checksum: self.checksum,
            timestamp: self.timestamp,
            bids: self
                .bids
                .into_iter()
                .map(|raw| raw.try_into_event(price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
            asks: self
                .asks
                .into_iter()
                .map(|raw| raw.try_into_event(price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

fn append_checksum_units(mut n: u64, out: &mut String) {
    if n == 0 {
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::path::{Path, PathBuf};
    use std::str::FromStr;
    use uuid::Uuid;

    // BTC/USD tick 0.1 and step 1e-8. The fixtures use these scales.
    const PRICE_SCALE: u8 = 1;
    const QTY_SCALE: u8 = 8;

    fn scaled_book(price_scale: u8, qty_scale: u8) -> OrderBook {
        OrderBook {
            asks: Orders {
                orders: Vec::new(),
                sort_asc: true,
            },
            bids: Orders {
                orders: Vec::new(),
                sort_asc: false,
            },
            timestamp: Utc::now(),
            price_scale: price_scale,
            qty_scale: qty_scale,
        }
    }

    fn units_to_decimal(units: u64, scale: u8) -> Decimal {
        Decimal::from_i128_with_scale(units as i128, u32::from(scale))
    }

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

    fn build_order(limit_price: Decimal, price_scale: u8, qty_scale: u8) -> Order {
        Order {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price: decimal_to_units(limit_price, price_scale).expect("price units"),
            order_qty: decimal_to_units(Decimal::from(1), qty_scale).expect("qty units"),
            timestamp: Utc::now(),
        }
    }

    fn build_order_event(event: EventType, limit_price: Decimal, price_scale: u8, qty_scale: u8) -> OrderEvent {
        OrderEvent {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price: decimal_to_units(limit_price, price_scale).expect("price units"),
            order_qty: decimal_to_units(Decimal::from(1), qty_scale).expect("qty units"),
            timestamp: Utc::now(),
            event,
        }
    }

    fn build_orders(prices: &[Decimal], sort_asc: bool) -> Orders {
        let mut orders = Orders {
            orders: Vec::new(),
            sort_asc,
        };
        for price in prices.iter() {
            orders.insert_order(build_order(*price, PRICE_SCALE, QTY_SCALE));
        }
        orders
    }

    #[test]
    fn order_price_and_qty_round_trip() {
        let price = Decimal::from_str("80796.1").expect("Invalid value");
        let qty = Decimal::from_str("0.00005000").expect("Invalid value");
        let order = Order {
            order_id: "id".to_string(),
            limit_price: decimal_to_units(price, PRICE_SCALE).expect("price units"),
            order_qty: decimal_to_units(qty, QTY_SCALE).expect("qty units"),
            timestamp: Utc::now(),
        };

        let book = scaled_book(PRICE_SCALE, QTY_SCALE);
        assert_eq!(book.price_to_decimal(order.limit_price), price);
        assert_eq!(book.qty_to_decimal(order.order_qty), qty);
    }

    #[test]
    fn orders_find_price_levels() {
        let prices = [
            "75650.0", "75650.0", "75650.0", "75650.0", "75647.4", "75646.9", "75644.8", "75644.7",
            "75643.7", "75642.3", "75640.9", "75639.9", "75639.3",
        ]
        .map(|price| Decimal::from_str(price).expect("Invalid value"));
        let bids = build_orders(&prices, false);

        assert_eq!(bids.find_price_level(0), Some(0));
        assert_eq!(bids.find_price_level(1), Some(4));
        assert_eq!(bids.find_price_level(9), Some(12));
        assert_eq!(bids.find_price_level(10), None);

        let empty_bids = Orders {
            orders: Vec::new(),
            sort_asc: false,
        };

        assert_eq!(empty_bids.find_price_level(0), None);
        assert_eq!(empty_bids.find_price_level(1), None);
    }

    #[test]
    fn orders_truncate_price_levels() {
        let mid_price = Decimal::from(1000);
        let prices: Vec<Decimal> = (0..10)
            .into_iter()
            .map(|i| mid_price - Decimal::from(i))
            .collect();
        let mut bids = build_orders(&prices, false);

        assert_eq!(bids.len(), 10);
        assert_eq!(units_to_decimal(bids[0].limit_price, PRICE_SCALE), Decimal::from(1000));
        assert_eq!(units_to_decimal(bids[9].limit_price, PRICE_SCALE), Decimal::from(991));

        // Nothing should change.
        bids.truncate_price_level(10);

        assert_eq!(bids.len(), 10);
        assert_eq!(units_to_decimal(bids[0].limit_price, PRICE_SCALE), Decimal::from(1000));
        assert_eq!(units_to_decimal(bids[9].limit_price, PRICE_SCALE), Decimal::from(991));

        // Just one level removed.
        bids.truncate_price_level(9);

        assert_eq!(bids.len(), 9);
        assert_eq!(units_to_decimal(bids[0].limit_price, PRICE_SCALE), Decimal::from(1000));
        assert_eq!(units_to_decimal(bids[8].limit_price, PRICE_SCALE), Decimal::from(992));

        // Leave only the first level.
        bids.truncate_price_level(1);

        assert_eq!(bids.len(), 1);
        assert_eq!(units_to_decimal(bids[0].limit_price, PRICE_SCALE), Decimal::from(1000));

        // No levels at all.
        bids.truncate_price_level(0);

        assert_eq!(bids.len(), 0);
    }

    #[test]
    fn orders_truncate_crowded_levels() {
        // Several orders share a price.
        let mut crowded = Orders {
            orders: Vec::new(),
            sort_asc: false,
        };
        for (price, count) in [(100, 2), (90, 1), (80, 2)] {
            for _ in 0..count {
                crowded.insert_order(build_order(Decimal::from(price), PRICE_SCALE, QTY_SCALE));
            }
        }
        crowded.truncate_price_level(2);

        assert_eq!(crowded.len(), 3);
        assert_eq!(units_to_decimal(crowded[2].limit_price, PRICE_SCALE), Decimal::from(90));

        crowded.truncate_price_level(1);

        assert_eq!(crowded.len(), 2);
        assert_eq!(units_to_decimal(crowded[1].limit_price, PRICE_SCALE), Decimal::from(100));
    }

    #[test]
    fn orders_insert_and_delete() {
        let prices: Vec<Decimal> = (0..3)
            .into_iter()
            .map(|i| Decimal::from(1) + Decimal::from(i))
            .collect();
        let mut orders = build_orders(&prices, true);

        for value in ["0.5", "1.5", "3"] {
            let event = build_order_event(
                EventType::Add, Decimal::from_str_exact(value).unwrap(), PRICE_SCALE, QTY_SCALE
            );
            orders.insert_order(Order::from_event(event));
        }

        assert_eq!(
            units_to_decimal(orders[0].limit_price, PRICE_SCALE),
            Decimal::from_str_exact("0.5").unwrap()
        );
        assert_eq!(
            units_to_decimal(orders[2].limit_price, PRICE_SCALE),
            Decimal::from_str_exact("1.5").unwrap()
        );
        assert_eq!(
            units_to_decimal(orders[4].limit_price, PRICE_SCALE),
            Decimal::from_str_exact("3").unwrap()
        );

        // Remove the 2nd order.
        let order_to_delete = &orders[2];
        let event = OrderEvent {
            order_id: order_to_delete.order_id.clone(),
            limit_price: order_to_delete.limit_price,
            order_qty: order_to_delete.order_qty,
            timestamp: Utc::now(),
            event: EventType::Delete,
        };
        assert!(orders.remove_order(&event.order_id, event.limit_price));
    }

    #[test]
    fn order_book_updater_truncating_update() {
        let price_scale: u8 = 0;
        let qty_scale: u8 = 0;
        let first_ask = Decimal::from(1010);
        let first_bid = Decimal::from(1000);
        let bids: Vec<Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_bid - Decimal::from(i), price_scale, qty_scale))
            .collect();
        let asks: Vec<Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_ask + Decimal::from(i), price_scale, qty_scale))
            .collect();
        let snapshot_ts = "2026-09-15T21:28:25.745828658Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        let first_update_ts = "2026-09-15T21:28:25.964171032Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        let second_update_ts = "2026-09-15T21:28:25.965086460Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        let snapshot = Snapshot {
            checksum: 1913449638,
            timestamp: snapshot_ts,
            bids,
            asks,
        };

        assert_eq!(snapshot.asks.len(), 10);
        assert_eq!(snapshot.bids.len(), 10);
        assert_eq!(
            units_to_decimal(snapshot.asks[9].limit_price, price_scale),
            Decimal::from(first_ask) + Decimal::from(19)
        );
        assert_eq!(
            units_to_decimal(snapshot.asks[0].limit_price, price_scale),
            Decimal::from(first_ask) + Decimal::from(10)
        );
        assert_eq!(
            units_to_decimal(snapshot.bids[0].limit_price, price_scale),
            Decimal::from(first_bid) - Decimal::from(10)
        );
        assert_eq!(
            units_to_decimal(snapshot.bids[9].limit_price, price_scale),
            Decimal::from(first_bid) - Decimal::from(19)
        );

        // This update should end up being ignored since all levels should get inserted past the end.
        let bids: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| build_order_event(EventType::Add, first_bid - Decimal::from(i), price_scale, qty_scale))
            .collect();
        let asks: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| build_order_event(EventType::Add, first_ask + Decimal::from(i), price_scale, qty_scale))
            .collect();
        let update = Update {
            checksum: 1913449638,
            timestamp: first_update_ts,
            bids,
            asks,
        };
        let mut updater = OrderBookUpdater::new(snapshot, 10, 0, 0);

        updater.update(update).unwrap();
        let obook = updater.get_order_book();

        assert_eq!(obook.asks.len(), 10);
        assert_eq!(obook.bids.len(), 10);
        assert_eq!(
            obook.price_to_decimal(obook.asks[9].limit_price),
            Decimal::from(first_ask) + Decimal::from(19)
        );
        assert_eq!(
            obook.price_to_decimal(obook.asks[0].limit_price),
            Decimal::from(first_ask) + Decimal::from(10)
        );
        assert_eq!(
            obook.price_to_decimal(obook.bids[0].limit_price),
            Decimal::from(first_bid) - Decimal::from(10)
        );
        assert_eq!(
            obook.price_to_decimal(obook.bids[9].limit_price),
            Decimal::from(first_bid) - Decimal::from(19)
        );
        assert_eq!(obook.timestamp, first_update_ts);
        assert_eq!(updater.calculate_checksum(), 1913449638);

        // This update should get the snapshot truncated since 5 levels should have been inserted right at the
        // beginning.
        let bids: Vec<OrderEvent> = (0..5)
            .into_iter()
            .map(|i| build_order_event(EventType::Add, first_bid - Decimal::from(i), price_scale, qty_scale))
            .collect();
        let asks: Vec<OrderEvent> = (0..5)
            .into_iter()
            .map(|i| build_order_event(EventType::Add, first_ask + Decimal::from(i), price_scale, qty_scale))
            .collect();
        let update = Update {
            checksum: 3473600046,
            timestamp: second_update_ts,
            bids,
            asks,
        };
        updater.update(update).unwrap();
        let obook = updater.get_order_book();

        assert_eq!(obook.asks.len(), 10);
        assert_eq!(obook.bids.len(), 10);
        assert_eq!(
            obook.price_to_decimal(obook.asks[9].limit_price),
            Decimal::from(first_ask) + Decimal::from(14)
        );
        assert_eq!(
            obook.price_to_decimal(obook.asks[0].limit_price),
            Decimal::from(first_ask)
        );
        assert_eq!(
            obook.price_to_decimal(obook.bids[0].limit_price),
            Decimal::from(first_bid)
        );
        assert_eq!(
            obook.price_to_decimal(obook.bids[9].limit_price),
            Decimal::from(first_bid) - Decimal::from(14)
        );
        assert_eq!(obook.timestamp, second_update_ts);
        assert_eq!(updater.calculate_checksum(), 3473600046);
    }

    #[test]
    fn mirror_processing_messages() {
        let mut mirror = OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);
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
        let mut mirror = OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);

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
        let mut mirror = OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);

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
        let mut mirror = OrderBookMirror::new(10, PRICE_SCALE, QTY_SCALE);
        let mut reader = open_test_file("btc_usd_stream.log");
        let mut line = String::new();

        while reader.read_line(&mut line).unwrap() > 0 {
            if line.len() > 0 {
                mirror.on_message(&line).unwrap();
            }
            line.clear();
        }

        match mirror.state {
            MirrorState::WaitingUpdate { updater } => {
                assert_eq!(updater.calculate_checksum(), 1906173362);
            }
            _ => {
                assert!(false, "No order book");
            }
        }
    }
}
