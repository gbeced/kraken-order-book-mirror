use super::{units, wire};
use crate::common::ts::Ts;
use rust_decimal::Decimal;
use std::error::Error;
use std::ops::Index;
use std::marker::PhantomData;

pub struct OrderBook {
    pub asks: Orders<Asc>,
    pub bids: Orders<Desc>,
    pub timestamp: Ts,
    price_scale: u8,
    qty_scale: u8,
}

impl OrderBook {
    pub(super) fn new(
        asks: Orders<Asc>,
        bids: Orders<Desc>,
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

pub struct Order {
    pub order_id: wire::OrderId,
    pub limit_price: u64,
    pub order_qty: u64,
    pub timestamp: Ts,
}

impl Order {
    pub(super) fn from_event(event: OrderEvent) -> Order {
        // Not using From trait because according to the docs the general expectation is that the conversion is lossless.
        Order {
            order_id: event.order_id,
            limit_price: event.limit_price,
            order_qty: event.order_qty,
            timestamp: event.timestamp,
        }
    }

    pub(super) fn try_from_wire(
        wire_order: wire::Order<'_>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<Order, Box<dyn Error>> {
        Ok(Order {
            order_id: wire_order.order_id,
            limit_price: units::parse_units(wire_order.limit_price.get(), price_scale)?,
            order_qty: units::parse_units(wire_order.order_qty.get(), qty_scale)?,
            timestamp: wire_order.timestamp,
        })
    }
}

pub(super) struct OrderEvent {
    pub order_id: wire::OrderId,
    pub limit_price: u64,
    pub order_qty: u64,
    pub timestamp: Ts,
    pub event: wire::EventType,
}

impl OrderEvent {
    pub(super) fn try_from_wire(
        wire_update: wire::OrderEvent<'_>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<OrderEvent, Box<dyn Error>> {
        Ok(OrderEvent {
            order_id: wire_update.order_id,
            limit_price: units::parse_units(wire_update.limit_price.get(), price_scale)?,
            order_qty: units::parse_units(wire_update.order_qty.get(), qty_scale)?,
            timestamp: wire_update.timestamp,
            event: wire_update.event,
        })
    }
}

pub trait SortOrder {
    fn key_before(p1: &u64, t1: &Ts, o2: &Order) -> bool;

    #[inline(always)]
    fn order_before(o1: &Order, o2: &Order) -> bool {
        Self::key_before(&o1.limit_price, &o1.timestamp, o2)
    }
}

pub struct Asc;
pub struct Desc;

impl SortOrder for Asc {
    #[inline(always)]
    fn key_before(p1: &u64, t1: &Ts, o2: &Order) -> bool {
        *p1 < o2.limit_price || *p1 == o2.limit_price && *t1 < o2.timestamp
    }
}

impl SortOrder for Desc {
    #[inline(always)]
    fn key_before(p1: &u64, t1: &Ts, o2: &Order) -> bool {
        *p1 > o2.limit_price || *p1 == o2.limit_price && *t1 < o2.timestamp
    }
}

// Orders are sorted by limit_price (asc/desc) + timestamp(asc).
pub struct Orders<O: SortOrder> {
    orders: Vec<Order>,
    phantom: PhantomData<O>,
}

pub type Bids = Orders<Desc>;
pub type Asks = Orders<Asc>;

impl<O: SortOrder> Orders<O> {
    pub(super) fn new(orders: Vec<Order>) -> Self {
        Self { orders, phantom: PhantomData }
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

    pub(super) fn insert_order(&mut self, order: Order) {
        let idx = self.orders
            .iter()
            .position(|o| O::order_before(&order, o))
            .unwrap_or(self.orders.len());
        self.orders.insert(idx, order);
    }

    pub(super) fn remove_order(&mut self, order_id: &wire::OrderId, limit_price: u64) -> bool {
        if let Some((idx, _)) = self.find_order(order_id, limit_price) {
            self.orders.remove(idx);
            true
        } else {
            false
        }
    }

    pub(super) fn find_order<'a>(
        &'a mut self,
        order_id: &wire::OrderId,
        limit_price: u64,
    ) -> Option<(usize, &'a mut Order)> {

        // Get to the beggining of the price level.
        let begin = self.orders
            .iter()
            .position(|o| O::key_before(&limit_price, &Ts(0), o))
            .unwrap_or(self.orders.len());

        // Search by order id within the price level.
        for (i, o) in self.orders.iter_mut().skip(begin).enumerate() {
            if limit_price != o.limit_price {
                return None;
            }
            else if order_id == &o.order_id {
                return Some((begin + i, o));
            }
        };
        None
    }

    pub(super) fn modify_order(&mut self, order: Order) -> bool {
        if let Some((idx, prev_order)) = self.find_order(&order.order_id, order.limit_price) {
            prev_order.order_qty = order.order_qty;
            if prev_order.timestamp != order.timestamp {
                // The order has to be relocated.
                prev_order.timestamp = order.timestamp;
                // Continue doing a linear search for the destination point within the price level, or at the end of it.
                let end = match self.orders[idx + 1..].iter().enumerate().find(|(_, o)| {
                    order.timestamp < o.timestamp || o.limit_price != order.limit_price
                }) {
                    Some((pos, _)) => idx + 1 + pos,
                    None => self.orders.len(),
                };
                // Move the order at the end.
                self.orders[idx..end].rotate_left(1);
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

    pub(super) fn truncate_price_level(&mut self, max_levels: usize) {
        // Truncates orders leaving only the first max_levels.
        if let Some(truncate_pos) = self.find_price_level(max_levels) {
            self.orders.drain(truncate_pos..);
        }
    }

    pub(super) fn apply_events(&mut self, events: Vec<OrderEvent>) -> Result<(), Box<dyn Error>> {
        for event in events {
            match event.event {
                // A new order added to the book.
                wire::EventType::Add => {
                    self.insert_order(Order::from_event(event));
                }
                // The order has been removed from the book, i.e. full fill or cancel.
                wire::EventType::Delete => {
                    if !self.remove_order(&event.order_id, event.limit_price) {
                        return Err(format!(
                            "Order not found {}",
                            std::str::from_utf8(&event.order_id)?
                        )
                        .into());
                    }
                }
                // The order quantity has been modified, i.e. a fill.
                wire::EventType::Modify => {
                    if !self.modify_order(Order::from_event(event)) {
                        return Err("Order not found".into());
                    }
                }
            }
        }
        Ok(())
    }
}

impl<O: SortOrder> Index<usize> for Orders<O> {
    type Output = Order;

    fn index(&self, index: usize) -> &Self::Output {
        &self.orders[index]
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::build_order;
    use super::*;
    use std::str::FromStr;
    use units::to_decimal;

    // BTC/USD tick 0.1 and step 1e-8. The fixtures use these scales.
    const PRICE_SCALE: u8 = 1;
    const QTY_SCALE: u8 = 8;

    fn build_orders<O: SortOrder> (prices: &[Decimal]) -> Orders<O> {
        Orders::<O>::new(
            prices
                .iter()
                .map(|price| build_order(*price, PRICE_SCALE, QTY_SCALE))
                .collect()
        )
    }

    #[test]
    fn orders_find_price_levels() {
        let prices = [
            "75650.0", "75650.0", "75650.0", "75650.0", "75647.4", "75646.9", "75644.8", "75644.7",
            "75643.7", "75642.3", "75640.9", "75639.9", "75639.3",
        ]
        .map(|price| Decimal::from_str(price).expect("Invalid value"));
        let bids = build_orders::<Desc>(&prices);

        assert_eq!(bids.find_price_level(0), Some(0));
        assert_eq!(bids.find_price_level(1), Some(4));
        assert_eq!(bids.find_price_level(9), Some(12));
        assert_eq!(bids.find_price_level(10), None);

        let empty_bids = Orders::<Desc>::new(Vec::new());

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
        let mut bids = build_orders::<Desc>(&prices);

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
        let mut crowded = Orders::<Desc>::new(Vec::new());
        for (price, count) in [(100, 2), (90, 1), (80, 2)] {
            for _ in 0..count {
                crowded.insert_order(build_order(Decimal::from(price), PRICE_SCALE, QTY_SCALE));
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
        let mut orders = build_orders::<Asc>(&prices);

        for value in ["0.5", "1.5", "3"] {
            orders.insert_order(build_order(
                Decimal::from_str(value).unwrap(),
                PRICE_SCALE,
                QTY_SCALE,
            ));
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
}
