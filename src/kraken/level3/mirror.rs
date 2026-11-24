use super::{order_book, wire};
use crate::common::error::Error;
use crate::common::units;
use std::cell::RefCell;

type SnapshotMsg<'a> = wire::Envelope<wire::Payload<wire::Order<'a>>>;
type UpdateMsg<'a> = wire::Envelope<wire::Payload<wire::OrderEvent<'a>>>;
type Snapshot = wire::Payload<order_book::Order>;
type Update = wire::Payload<order_book::OrderEvent>;

pub struct OrderBookMirror {
    context: MirrorContext,
    state: MirrorState,
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

    pub fn is_valid(&self) -> bool {
        ! matches!(self.state, MirrorState::Invalid)
    }

    pub fn on_message(&mut self, msg: &str) -> Result<(), Error> {
        if let Ok(update_msg) = serde_json::from_str::<UpdateMsg>(msg) {
            self.process_update(update_msg)
        } else if let Ok(snapshot_msg) = serde_json::from_str::<SnapshotMsg>(msg) {
            self.process_snapshot(snapshot_msg)
        } else {
            let preview: String = msg.chars().take(50).collect();
            Err(Error::UnexpectedMessage(format!(
                "Unexpected message '{} ...'", preview
            )))
        }
    }

    pub fn get_order_book(&self) -> Option<&order_book::OrderBook> {
        match self.state {
            MirrorState::WaitingUpdate { ref updater } => Some(updater.get_order_book()),
            _ => None,
        }
    }

    /// CRC32 of the current book, as `on_message` computes it to verify a
    /// frame. `None` until a snapshot is applied, or once the state is invalid.
    pub fn calculate_checksum(&self) -> Option<u32> {
        match self.state {
            MirrorState::WaitingUpdate { ref updater } => Some(updater.calculate_checksum()),
            _ => None,
        }
    }

    #[cold]
    fn process_snapshot(&mut self, snapshot_msg: SnapshotMsg<'_>) -> Result<(), Error> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        snapshot_msg.data.into_iter().try_for_each(|wire_order| {
            let snapshot = Snapshot::try_from_wire(wire_order, price_scale, qty_scale)?;
            self.state.on_snapshot(&self.context, snapshot)
        })
    }

    fn process_update(&mut self, update_msg: UpdateMsg<'_>) -> Result<(), Error> {
        let price_scale = self.context.price_scale;
        let qty_scale = self.context.qty_scale;
        update_msg.data.into_iter().try_for_each(|wire_update| {
            let update = Update::try_from_wire(wire_update, price_scale, qty_scale)?;
            self.state.on_update(&self.context, update)
        })
    }
}
struct MirrorContext {
    max_depth: usize,
    price_scale: u8,
    qty_scale: u8,
}

enum MirrorState {
    WaitingSnapshot,
    WaitingUpdate { updater: OrderBookUpdater },
    Invalid,
}

impl MirrorState {
    fn on_snapshot(&mut self, context: &MirrorContext, snapshot: Snapshot) -> Result<(), Error> {
        match self {
            MirrorState::WaitingSnapshot => {
                match OrderBookUpdater::new(
                    snapshot, context.max_depth, context.price_scale, context.qty_scale,
                ) {
                    Ok(updater) => {
                        *self = MirrorState::WaitingUpdate { updater};
                        Ok(())
                    }
                    Err(e) => {
                        *self = MirrorState::Invalid;
                        Err(e)
                    }
                }
            }
            MirrorState::WaitingUpdate { .. } => {
                *self = MirrorState::Invalid;
                Err(Error::UnexpectedMessage(
                    "Expected update but received snapshot".into(),
                ))
            }
            MirrorState::Invalid => Err(Error::UnexpectedMessage(
                "Received snapshot in an invalid state".into(),
            )),
        }
    }

    fn on_update(&mut self, _context: &MirrorContext, update: Update) -> Result<(), Error> {
        match self {
            MirrorState::WaitingSnapshot => {
                *self = MirrorState::Invalid;
                Err(Error::UnexpectedMessage(
                    "Expected snapshot but received update".into(),
                ))
            }
            MirrorState::WaitingUpdate { updater } => {
                updater.update(update).inspect_err(|_| {
                    // If update failed then the order book is now in an invalid state and we should drop it.
                    *self = MirrorState::Invalid;
                })
            }
            MirrorState::Invalid => Err(Error::UnexpectedMessage(
                "Received update in an invalid state".into(),
            )),
        }
    }
}

struct OrderBookUpdater {
    order_book: order_book::OrderBook,
    max_depth: usize,
    serializer_buff: RefCell<String>,
}

impl OrderBookUpdater {
    fn new(
        snapshot: Snapshot,
        max_depth: usize,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<OrderBookUpdater, Error> {
        let asks = order_book::Asks::new(snapshot.asks);
        let bids = order_book::Bids::new(snapshot.bids);
        let order_book =
            order_book::OrderBook::new(asks, bids, snapshot.timestamp, price_scale, qty_scale);
        let checksum = snapshot.checksum;

        let ret = OrderBookUpdater {
            order_book,
            max_depth,
            serializer_buff: RefCell::new(String::with_capacity(max_depth * 2 * 10)),
        };
        let calculated = ret.calculate_checksum();
        if calculated != checksum {
            return Err(Error::ChecksumMismatch {
                expected: checksum.to_string(),
                calculated: calculated.to_string(),
            });
        }

        Ok(ret)
    }

    fn update(&mut self, update: Update) -> Result<(), Error> {
        // If any of these fail, the order book ends up in an invalid state.
        self.order_book.asks.apply_events(update.asks)?;
        self.order_book.asks.truncate_price_level(self.max_depth);
        self.order_book.bids.apply_events(update.bids)?;
        self.order_book.bids.truncate_price_level(self.max_depth);

        let calculated = self.calculate_checksum();
        if calculated != update.checksum {
            return Err(Error::ChecksumMismatch {
                expected: update.checksum.to_string(),
                calculated: calculated.to_string(),
            });
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
        let mut buff = self.serializer_buff.borrow_mut();

        buff.clear();
        for order in obook
            .asks
            .take_up_to_price_level(CHECKSUM_PRICE_LEVELS)
            .chain(obook.bids.take_up_to_price_level(CHECKSUM_PRICE_LEVELS))
        {
            units::serialize_units(order.limit_price, &mut buff);
            units::serialize_units(order.order_qty, &mut buff);
        }
        crc32fast::hash(buff.as_bytes())
    }
}

impl Snapshot {
    fn try_from_wire(
        snapshot: wire::Payload<wire::Order<'_>>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<Snapshot, Error> {
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
        update: wire::Payload<wire::OrderEvent<'_>>,
        price_scale: u8,
        qty_scale: u8,
    ) -> Result<Update, Error> {
        Ok(Update {
            checksum: update.checksum,
            timestamp: update.timestamp,
            bids: update
                .bids
                .into_iter()
                .map(|wire_update| {
                    order_book::OrderEvent::try_from_wire(wire_update, price_scale, qty_scale)
                })
                .collect::<Result<Vec<_>, _>>()?,
            asks: update
                .asks
                .into_iter()
                .map(|wire_event| {
                    order_book::OrderEvent::try_from_wire(wire_event, price_scale, qty_scale)
                })
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::{build_order, build_order_event};
    use super::*;
    use crate::common::ts::Ts;
    use rust_decimal::Decimal;

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
        let bids: Vec<order_book::OrderEvent> = (20..25)
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
        let asks: Vec<order_book::OrderEvent> = (20..25)
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
        let mut updater = OrderBookUpdater::new(snapshot, 10, 0, 0).unwrap();

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
        let bids: Vec<order_book::OrderEvent> = (0..5)
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
        let asks: Vec<order_book::OrderEvent> = (0..5)
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

    #[test]
    fn updater_fails_with_invalid_snapshot() {
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
        let snapshot = Snapshot {
            checksum: 0,
            timestamp: snapshot_ts,
            bids,
            asks,
        };

        match OrderBookUpdater::new(snapshot, 10, 0, 0) {
            Err(Error::ChecksumMismatch { expected, calculated }) => {
                assert_eq!(expected, "0");
                assert_eq!(calculated, "1913449638");

            },
            Err(other) => {
                panic!("ChecksumMismatch was expected, got {}", other);

            }
            Ok(_) => {
                panic!("ChecksumMismatch was expected")
            }
        }
    }
}
