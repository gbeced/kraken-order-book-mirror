use super::units;
use super::{order_book, wire};
use crate::common::ts::Ts;
use std::cell::RefCell;
use std::error::Error;

struct OrderEvent {
    order_id: String,
    limit_price: u64,
    order_qty: u64,
    timestamp: Ts,
    event: wire::EventType,
}

type SnapshotMsg = wire::Envelope<wire::Payload<wire::Order>>;
type UpdateMsg = wire::Envelope<wire::Payload<wire::OrderEvent>>;
type Snapshot = wire::Payload<order_book::Order>;
type Update = wire::Payload<OrderEvent>;

struct OrderBookUpdater {
    order_book: order_book::OrderBook,
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

impl OrderEvent {
    fn try_from_wire(
        wire_update: wire::OrderEvent,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<OrderEvent, Box<dyn Error>> {
        Ok(OrderEvent {
            order_id: wire_update.order_id,
            limit_price: units::to_units(wire_update.limit_price, price_scale)?,
            order_qty: units::to_units(wire_update.order_qty, qty_scale)?,
            timestamp: wire_update.timestamp,
            event: wire_update.event,
        })
    }
}

impl Snapshot {
    fn try_from_wire(
        snapshot: wire::Payload<wire::Order>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<Snapshot, Box<dyn Error>> {
        Ok(Snapshot {
            checksum: snapshot.checksum,
            timestamp: snapshot.timestamp,
            bids: snapshot
                .bids
                .into_iter()
                .map(|wire_order| {
                    order_book::Order::try_from_wire(wire_order, price_scale, qty_scale)
                })
                .collect::<Result<Vec<_>, _>>()?,
            asks: snapshot
                .asks
                .into_iter()
                .map(|wire_order| {
                    order_book::Order::try_from_wire(wire_order, price_scale, qty_scale)
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl Update {
    fn try_from_wire(
        update: wire::Payload<wire::OrderEvent>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<Update, Box<dyn Error>> {
        Ok(Update {
            checksum: update.checksum,
            timestamp: update.timestamp,
            bids: update
                .bids
                .into_iter()
                .map(|wire_update| OrderEvent::try_from_wire(wire_update, price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
            asks: update
                .asks
                .into_iter()
                .map(|wire_event| OrderEvent::try_from_wire(wire_event, price_scale, qty_scale))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl OrderBookUpdater {
    fn new(
        snapshot: Snapshot,
        max_depth: usize,
        price_scale: u8,
        qty_scale: u8,
    ) -> OrderBookUpdater {
        let asks = order_book::Orders::new(snapshot.asks, true);
        let bids = order_book::Orders::new(snapshot.bids, false);
        let order_book =
            order_book::OrderBook::new(asks, bids, snapshot.timestamp, price_scale, qty_scale);
        OrderBookUpdater {
            order_book,
            max_depth,
            serializer_buff: RefCell::new(String::with_capacity(max_depth * 2 * 10)),
        }
    }

    fn update(&mut self, update: Update) -> Result<(), Box<dyn Error>> {
        // If any of these fail, the order book ends up in an invalid state.
        apply_events(&mut self.order_book.asks, update.asks)?;
        self.order_book.asks.truncate_price_level(self.max_depth);
        apply_events(&mut self.order_book.bids, update.bids)?;
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

    pub fn get_order_book(&self) -> &order_book::OrderBook {
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
            units::serialize_units(order.limit_price, &mut buff);
            units::serialize_units(order.order_qty, &mut buff);
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

    pub fn get_order_book(&self) -> Option<&order_book::OrderBook> {
        match self.state {
            MirrorState::WaitingUpdate { ref updater } => Some(updater.get_order_book()),
            _ => None,
        }
    }

    #[cold]
    fn process_snapshot(&mut self, snapshot_msg: SnapshotMsg) -> Result<(), Box<dyn Error>> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        snapshot_msg.data.into_iter().try_for_each(|wire_order| {
            let snapshot = Snapshot::try_from_wire(wire_order, price_scale, qty_scale)?;
            self.state.on_snapshot(&self.context, snapshot)
        })
    }

    fn process_update(&mut self, update_msg: UpdateMsg) -> Result<(), Box<dyn Error>> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        update_msg.data.into_iter().try_for_each(|wire_update| {
            let update = Update::try_from_wire(wire_update, price_scale, qty_scale)?;
            self.state.on_update(&self.context, update)
        })
    }
}

impl order_book::Order {
    fn from_event(event: OrderEvent) -> order_book::Order {
        // assert_eq!(event.event, wire::EventType::Add);
        // Not using From trait because the general expectation is that the conversion is lossless.
        order_book::Order {
            order_id: event.order_id,
            limit_price: event.limit_price,
            order_qty: event.order_qty,
            timestamp: event.timestamp,
        }
    }

    fn try_from_wire(
        wire_order: wire::Order,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<order_book::Order, Box<dyn Error>> {
        Ok(order_book::Order {
            order_id: wire_order.order_id,
            limit_price: units::to_units(wire_order.limit_price, price_scale)?,
            order_qty: units::to_units(wire_order.order_qty, qty_scale)?,
            timestamp: wire_order.timestamp,
        })
    }
}

fn apply_events(
    orders: &mut order_book::Orders,
    events: Vec<OrderEvent>,
) -> Result<(), Box<dyn Error>> {
    for event in events {
        match event.event {
            // A new order added to the book.
            wire::EventType::Add => {
                orders.insert_order(order_book::Order::from_event(event));
            }
            // The order has been removed from the book, i.e. full fill or cancel.
            wire::EventType::Delete => {
                if !orders.remove_order(&event.order_id, event.limit_price) {
                    return Err(format!("Order not found {}", event.order_id).into());
                }
            }
            // The order quantity has been modified, i.e. a fill.
            wire::EventType::Modify => {
                if !orders.modify_order(order_book::Order::from_event(event)) {
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
    use rust_decimal::Decimal;
    use units::to_units;
    use uuid::Uuid;

    fn build_order(limit_price: Decimal, price_scale: u8, qty_scale: u8) -> order_book::Order {
        order_book::Order {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price: to_units(limit_price, price_scale).expect("price units"),
            order_qty: to_units(Decimal::from(1), qty_scale).expect("qty units"),
            timestamp: Ts::try_from(chrono::Utc::now()).unwrap(),
        }
    }

    fn build_order_event(
        event: wire::EventType,
        limit_price: Decimal,
        price_scale: u8,
        qty_scale: u8,
    ) -> OrderEvent {
        OrderEvent {
            order_id: Uuid::new_v4().simple().to_string(),
            limit_price: to_units(limit_price, price_scale).expect("price units"),
            order_qty: to_units(Decimal::from(1), qty_scale).expect("qty units"),
            timestamp: Ts::try_from(chrono::Utc::now()).unwrap(),
            event,
        }
    }

    #[test]
    fn updater_processing_updates() {
        // Build the snapshot.
        let price_scale: u8 = 0;
        let qty_scale: u8 = 0;
        let first_ask = Decimal::from(1010);
        let first_bid = Decimal::from(1000);
        let bids: Vec<order_book::Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_bid - Decimal::from(i), price_scale, qty_scale))
            .collect();
        let asks: Vec<order_book::Order> = (10..20)
            .into_iter()
            .map(|i| build_order(first_ask + Decimal::from(i), price_scale, qty_scale))
            .collect();
        let snapshot_ts = Ts::try_from("2026-09-15T21:28:25.745828658Z").unwrap();
        let first_update_ts = Ts::try_from("2026-09-15T21:28:25.964171032Z").unwrap();
        let second_update_ts = Ts::try_from("2026-09-15T21:28:25.965086460Z").unwrap();
        let snapshot = Snapshot {
            checksum: 1913449638,
            timestamp: snapshot_ts,
            bids,
            asks,
        };

        // This update should end up being ignored since all levels should get inserted past the end.
        let bids: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| {
                build_order_event(
                    wire::EventType::Add,
                    first_bid - Decimal::from(i),
                    price_scale,
                    qty_scale,
                )
            })
            .collect();
        let asks: Vec<OrderEvent> = (20..25)
            .into_iter()
            .map(|i| {
                build_order_event(
                    wire::EventType::Add,
                    first_ask + Decimal::from(i),
                    price_scale,
                    qty_scale,
                )
            })
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
            .map(|i| {
                build_order_event(
                    wire::EventType::Add,
                    first_bid - Decimal::from(i),
                    price_scale,
                    qty_scale,
                )
            })
            .collect();
        let asks: Vec<OrderEvent> = (0..5)
            .into_iter()
            .map(|i| {
                build_order_event(
                    wire::EventType::Add,
                    first_ask + Decimal::from(i),
                    price_scale,
                    qty_scale,
                )
            })
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
}
