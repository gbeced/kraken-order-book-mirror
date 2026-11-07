use super::{units, wire};
use crate::common::ts::Ts;
use crate::kraken::level3::wire::EventType;
use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::ops::Index;
use std::error::Error;

pub struct Order {
    pub order_id: String,
    pub limit_price: u64,
    pub order_qty: u64,
    pub timestamp: Ts,
}

pub struct OrderEvent {
    pub order_id: String,
    pub limit_price: u64,
    pub order_qty: u64,
    pub timestamp: Ts,
    pub event: wire::EventType,
}


// Orders are sorted by limit_price (asc/desc) + timestamp(asc).
pub struct Orders {
    orders: Vec<Order>,
    sort_asc: bool,
}

pub struct OrderBook {
    pub asks: Orders,
    pub bids: Orders,
    pub timestamp: Ts,
    price_scale: u8,
    qty_scale: u8,
}

impl Order {
    fn from_event(event: OrderEvent) -> Order {
        // assert_eq!(event.event, wire::EventType::Add);
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
    pub fn new(orders: Vec<Order>, sort_asc: bool) -> Orders {
        Orders { orders, sort_asc }
    }

    fn cmp_price(&self, price_1: u64, price_2: u64) -> Ordering {
        let mut ret = price_1.cmp(&price_2);
        if ! self.sort_asc {
            ret = ret.reverse();
        }
        ret
    }

    pub fn len(&self) -> usize {
        self.orders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    pub fn iter<'a>(&'a self) -> std::slice::Iter<'a, Order> {
        self.orders.iter()
    }

    pub fn apply_events(&mut self, events: Vec<OrderEvent>) -> Result<(), Box<dyn Error>> {
        // Events are sorted by price level, but TIMESTAMP ORDER WITHIN THE SAME PRICE LEVEL IS NOT GUARANTEED.
        let mut events = events;
        timestamp_sort(& mut events);

        let mut i: usize = 0;
        for event in events {
            match event.event {
                EventType::Add => {
                    // Find insert location, starting at i-th position.
                    let delta = match self.orders[i..].iter().enumerate().find(
                        |(_, o)| self.cmp_price(event.limit_price, o.limit_price).is_lt() || (event.limit_price == o.limit_price && event.timestamp < o.timestamp) 
                    ) {
                        Some((pos, _)) => pos,
                        None => self.orders.len() - i,
                    };
                    i += delta;
                    self.orders.insert(i, Order::from_event(event));
                }
                EventType::Modify => {
                    // Find order by id, starting at i-th position.
                    let Some((delta, order)) = self.orders[i..].iter_mut().enumerate().find(
                        |(_, o)| o.order_id == event.order_id
                    ) else {
                        return Err("Order not found".into());
                    };
                    i += delta;
                    
                    // If the timestamp is the same, we can udpate in place.
                    if order.timestamp == event.timestamp {
                        order.order_qty = event.order_qty;
                        i += 1;
                    }
                    else {
                        // Timestamp changed, so order has to be relocated.
                        order.order_qty = event.order_qty;
                        order.timestamp = event.timestamp;
                        // Walk forward until we find the final position for the updated order.
                        let mut final_pos = i + 1;
                        for order in self.orders[final_pos..].iter() {
                            if event.limit_price != order.limit_price || event.timestamp < order.timestamp {
                                break;
                            }
                            final_pos += 1;
                        }
                        // Move the order to the end of the range, and bring the rest back forward.
                        self.orders[i..final_pos].rotate_left(1);
                    }
                }
                EventType::Delete => {
                    // Find order by id, starting at i-th position.
                    let Some((delta, _)) = self.orders[i..].iter().enumerate().find(
                        |(_, o)| o.order_id == event.order_id
                    ) else {
                        return Err("Order not found".into());
                    };
                    i += delta;
                    self.orders.remove(i);
                }
            }
        }

        Ok(())
    }

    pub fn insert_order(&mut self, order: Order) {
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

    pub fn remove_order(&mut self, order_id: &str, limit_price: u64) -> bool {
        if let Some((idx, _)) = self.find_order(order_id, limit_price) {
            self.orders.remove(idx);
            true
        } else {
            false
        }
    }

    pub fn find_order<'a>(
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

    pub fn modify_order(&mut self, order: Order) -> bool {
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

    pub fn find_price_level(&self, level: usize) -> Option<usize> {
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

    pub fn truncate_price_level(&mut self, max_levels: usize) {
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
    pub fn new(
        asks: Orders,
        bids: Orders,
        timestamp: Ts,
        price_scale: u8,
        qty_scale: u8,
    ) -> OrderBook {
        OrderBook {
            asks,
            bids,
            timestamp,
            price_scale,
            qty_scale,
        }
    }

    pub fn price_to_decimal(&self, price: u64) -> Decimal {
        units::to_decimal(price, self.price_scale)
    }

    pub fn qty_to_decimal(&self, qty: u64) -> Decimal {
        units::to_decimal(qty, self.qty_scale)
    }
}

fn timestamp_sort(events: & mut [OrderEvent]) {
    for i in 1..events.len() {
        let prev = &events[i - 1];
        let curr = &events[i];

        // If we find an out of order event within a price level, move it back were it belongs.
        if prev.limit_price == curr.limit_price && curr.timestamp < prev.timestamp {
            // Very unlikely.
            reorder_last(& mut events[..=i]);
        }
    }
}

#[cold]
fn reorder_last(events: & mut [OrderEvent]) {
    let last = events.last().unwrap();
    let mut i = events.len() - 1;
    // Get to the position where the last element should be, and move it there by rotating.
    while i > 0 && last.limit_price == events[i - 1].limit_price && last.timestamp < events[i - 1].timestamp {
        i -= 1;
    }
    let last_pos = events.len();
    events[i..last_pos].rotate_right(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use units::{to_decimal, to_units};
    use uuid::Uuid;

    // BTC/USD tick 0.1 and step 1e-8. The fixtures use these scales.
    const PRICE_SCALE: u8 = 1;
    const QTY_SCALE: u8 = 8;

    fn build_order(limit_price: Decimal) -> Order {
        Order {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price: to_units(limit_price, PRICE_SCALE).expect("price units"),
            order_qty: to_units(Decimal::from(1), QTY_SCALE).expect("qty units"),
            timestamp: Ts::try_from(chrono::Utc::now()).unwrap(),
        }
    }

    fn build_orders(prices: &[Decimal], sort_asc: bool) -> Orders {
        Orders::new(
            prices
                .iter()
                .map(|price| build_order(*price))
                .collect(),
            sort_asc,
        )
    }

    fn build_event(order_id: &str, limit_price: &str, timestamp: Ts, type_: wire::EventType) -> OrderEvent {
        OrderEvent {
            order_id: order_id.to_string(),
            limit_price: to_units(Decimal::from_str(limit_price).unwrap(), PRICE_SCALE).unwrap(),
            order_qty: to_units(Decimal::from(1), QTY_SCALE).unwrap(),
            timestamp: timestamp,
            event: type_
        }
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
        assert_eq!(
            to_decimal(bids[0].limit_price, PRICE_SCALE),
            Decimal::from(1000)
        );
        assert_eq!(
            to_decimal(bids[9].limit_price, PRICE_SCALE),
            Decimal::from(991)
        );

        // Nothing should change.
        bids.truncate_price_level(10);

        assert_eq!(bids.len(), 10);
        assert_eq!(
            to_decimal(bids[0].limit_price, PRICE_SCALE),
            Decimal::from(1000)
        );
        assert_eq!(
            to_decimal(bids[9].limit_price, PRICE_SCALE),
            Decimal::from(991)
        );

        // Just one level removed.
        bids.truncate_price_level(9);

        assert_eq!(bids.len(), 9);
        assert_eq!(
            to_decimal(bids[0].limit_price, PRICE_SCALE),
            Decimal::from(1000)
        );
        assert_eq!(
            to_decimal(bids[8].limit_price, PRICE_SCALE),
            Decimal::from(992)
        );

        // Leave only the first level.
        bids.truncate_price_level(1);

        assert_eq!(bids.len(), 1);
        assert_eq!(
            to_decimal(bids[0].limit_price, PRICE_SCALE),
            Decimal::from(1000)
        );

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
                crowded.insert_order(build_order(Decimal::from(price)));
            }
        }
        crowded.truncate_price_level(2);

        assert_eq!(crowded.len(), 3);
        assert_eq!(
            to_decimal(crowded[2].limit_price, PRICE_SCALE),
            Decimal::from(90)
        );

        crowded.truncate_price_level(1);

        assert_eq!(crowded.len(), 2);
        assert_eq!(
            to_decimal(crowded[1].limit_price, PRICE_SCALE),
            Decimal::from(100)
        );
    }

    #[test]
    fn orders_insert_and_delete() {
        let prices: Vec<Decimal> = (0..3)
            .into_iter()
            .map(|i| Decimal::from(1) + Decimal::from(i))
            .collect();
        let mut orders = build_orders(&prices, true);

        for value in ["0.5", "1.5", "3"] {
            orders.insert_order(build_order(Decimal::from_str(value).unwrap()));
        }

        assert_eq!(
            to_decimal(orders[0].limit_price, PRICE_SCALE),
            Decimal::from_str("0.5").unwrap()
        );
        assert_eq!(
            to_decimal(orders[2].limit_price, PRICE_SCALE),
            Decimal::from_str("1.5").unwrap()
        );
        assert_eq!(
            to_decimal(orders[4].limit_price, PRICE_SCALE),
            Decimal::from_str("3").unwrap()
        );

        // Remove the 2nd order.
        let order_to_delete = &orders[2];
        assert!(orders.remove_order(
            &order_to_delete.order_id.clone(),
            order_to_delete.limit_price
        ));
    }

    #[test]
    fn timestamp_sort_on_single_price_range() {
        let now = Ts::now();
        let mut events = [
            build_event("2", "1000", Ts(now.0 + 1), wire::EventType::Modify),
            build_event("0", "1000", Ts(now.0 - 1), wire::EventType::Add),
            build_event("1", "1000", now, wire::EventType::Delete),
        ];
        timestamp_sort(& mut events);

        assert_eq!(events.len(), 3);
        events.iter().enumerate().for_each(|(i, e)| {
            assert_eq!(e.order_id, i.to_string());
        });
    }

    #[test]
    fn timestamp_sort_on_multiple_price_ranges() {
        let now = Ts::now();
        let mut events = [
            build_event("2", "1000", Ts(now.0 + 1), wire::EventType::Modify),
            build_event("0", "1000", Ts(now.0 - 1), wire::EventType::Add),
            build_event("1", "1000", now, wire::EventType::Delete),
            build_event("5", "2000", Ts(now.0 + 1), wire::EventType::Delete),
            build_event("3", "2000", Ts(now.0 - 1), wire::EventType::Modify),
            build_event("4", "2000", now, wire::EventType::Add),
        ];
        timestamp_sort(& mut events);

        assert_eq!(events.len(), 6);
        events.iter().enumerate().for_each(|(i, e)| {
            assert_eq!(e.order_id, i.to_string());
        });
    }

}
