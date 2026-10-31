//! Pure order-book analytics. Everything here takes decimal `(price, qty)` orders
//! (asks ascending, bids descending, one entry per order) and returns plain values.

use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

pub(super) type Orders<'a> = &'a [(Decimal, Decimal)];

const BPS: i64 = 10_000;
pub(super) const BAND_BPS: [i64; 5] = [5, 10, 25, 50, 100];
pub(super) const IMPACT_NOTIONALS: [i64; 3] = [10_000, 100_000, 1_000_000];
pub(super) const BIG_ORDER_MULTIPLE: i64 = 10;

#[derive(Clone, Copy)]
pub(super) struct DepthBand {
    pub bid: Decimal,
    pub ask: Decimal,
}

#[derive(Clone, Copy)]
pub(super) struct Panel {
    pub mid: Decimal,
    pub spread: Decimal,
    pub spread_bps: Decimal,
    pub micro: Decimal,
}

pub(super) fn panel_from_book(asks: Orders, bids: Orders) -> Option<Panel> {
    let (best_bid, _) = *bids.first()?;
    let (best_ask, _) = *asks.first()?;
    let mid = (best_bid + best_ask) / Decimal::from(2);
    if mid.is_zero() {
        return None;
    }

    let bid_touch = sum_touch(bids, best_bid);
    let ask_touch = sum_touch(asks, best_ask);
    let touch = bid_touch + ask_touch;
    if touch.is_zero() {
        return None;
    }
    let micro = (best_ask * bid_touch + best_bid * ask_touch) / touch;
    let spread = best_ask - best_bid;
    let spread_bps = spread / mid * Decimal::from(BPS);

    Some(Panel {
        mid,
        spread,
        spread_bps,
        micro,
    })
}

/// Quantity resting within `bps` basis points of `mid` on each side.
/// Bids are descending and asks ascending, so each walk stops at the first price outside the band.
fn band(asks: Orders, bids: Orders, mid: Decimal, bps: i64) -> DepthBand {
    let offset = Decimal::new(bps, 4);
    let floor = mid * (Decimal::ONE - offset);
    let ceil = mid * (Decimal::ONE + offset);
    DepthBand {
        bid: sum_while(bids, |price| price >= floor),
        ask: sum_while(asks, |price| price <= ceil),
    }
}

pub(super) fn sum_touch(orders: Orders, price: Decimal) -> Decimal {
    let mut qty = Decimal::ZERO;
    for (level_price, level_qty) in orders {
        if *level_price != price {
            break;
        }
        qty += *level_qty;
    }
    qty
}

pub(super) fn sum_while(orders: Orders, mut keep: impl FnMut(Decimal) -> bool) -> Decimal {
    let mut qty = Decimal::ZERO;
    for (price, level_qty) in orders {
        if !keep(*price) {
            break;
        }
        qty += *level_qty;
    }
    qty
}

/// Quantity in the first `levels` distinct price levels.
pub(super) fn sum_levels(orders: Orders, levels: usize) -> Decimal {
    let mut qty = Decimal::ZERO;
    let mut seen = 0;
    let mut last: Option<Decimal> = None;
    for (price, level_qty) in orders {
        if last != Some(*price) {
            seen += 1;
            if seen > levels {
                break;
            }
            last = Some(*price);
        }
        qty += *level_qty;
    }
    qty
}

pub(super) fn imbalance(bid: Decimal, ask: Decimal) -> Decimal {
    let total = bid + ask;
    if total.is_zero() {
        Decimal::ZERO
    } else {
        (bid - ask) / total
    }
}

// ----- ladder -------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Level {
    pub price: Decimal,
    pub qty: Decimal,
    pub orders: usize,
    pub cum: Decimal,
}

/// Aggregate one side into at most `rows` price levels, best first. `step` re-buckets
/// prices (asks round up, bids round down) so a trader can zoom out.
pub(super) fn ladder(
    orders: Orders,
    is_ask: bool,
    step: Option<Decimal>,
    rows: usize,
) -> Vec<Level> {
    let mut out: Vec<Level> = Vec::with_capacity(rows);
    for &(price, qty) in orders {
        let bucket = match step {
            None => price,
            Some(step) if is_ask => (price / step).ceil() * step,
            Some(step) => (price / step).floor() * step,
        };
        match out.last_mut() {
            Some(level) if level.price == bucket => {
                level.qty += qty;
                level.orders += 1;
            }
            _ => {
                if out.len() == rows {
                    break;
                }
                out.push(Level {
                    price: bucket,
                    qty,
                    orders: 1,
                    cum: Decimal::ZERO,
                });
            }
        }
    }
    let mut cum = Decimal::ZERO;
    for level in &mut out {
        cum += level.qty;
        level.cum = cum;
    }
    out
}

// ----- analysis -----------------------------------------------------------------------

pub(super) struct ImbalanceRow {
    pub label: &'static str,
    pub bid: Decimal,
    pub ask: Decimal,
}

pub(super) struct BandRow {
    pub bps: i64,
    pub bid: Decimal,
    pub ask: Decimal,
}

pub(super) struct ImpactRow {
    pub notional: i64,
    pub buy_bps: Option<Decimal>,
    pub sell_bps: Option<Decimal>,
}

pub(super) struct BigOrder {
    pub price: Decimal,
    pub qty: Decimal,
    /// Signed distance from mid in bps (asks positive, bids negative).
    pub from_mid_bps: Decimal,
    /// Order size divided by the side's average order size.
    pub multiple: Decimal,
}

impl BigOrder {
    pub fn is_wall(&self) -> bool {
        self.multiple >= Decimal::from(BIG_ORDER_MULTIPLE)
    }
}

pub(super) struct Analysis {
    pub panel: Panel,
    pub tick: Decimal,
    pub price_dp: u32,
    pub touch: Touch,
    pub spread_ticks: Decimal,
    pub micro_offset_ticks: Decimal,
    pub bid_orders: usize,
    pub ask_orders: usize,
    pub avg_bid_order: Decimal,
    pub avg_ask_order: Decimal,
    pub imbalances: [ImbalanceRow; 5],
    pub l10_imbalance: Decimal,
    pub bands: Vec<BandRow>,
    pub impact: Vec<ImpactRow>,
    pub big_bid: Option<BigOrder>,
    pub big_ask: Option<BigOrder>,
}

impl Analysis {
    pub fn is_locked(&self) -> bool {
        self.touch.bid_price == self.touch.ask_price
    }

    pub fn is_crossed(&self) -> bool {
        self.touch.bid_price > self.touch.ask_price
    }

    pub fn from_book(asks: Orders, bids: Orders) -> Option<Analysis> {
        let panel = panel_from_book(asks, bids)?;
        let (best_bid, _) = bids[0];
        let (best_ask, _) = asks[0];
        let price_dp = best_bid.scale().max(best_ask.scale());
        let tick = Decimal::new(1, price_dp);
        let touch = Touch {
            bid_price: best_bid,
            bid_qty: sum_touch(bids, best_bid),
            ask_price: best_ask,
            ask_qty: sum_touch(asks, best_ask),
        };
        let mid = panel.mid;

        let bands: Vec<BandRow> = BAND_BPS
            .iter()
            .map(|&bps| {
                let DepthBand { bid, ask } = band(asks, bids, mid, bps);
                BandRow { bps, bid, ask }
            })
            .collect();
        let band_at = |bps: i64| {
            let row = bands.iter().find(|row| row.bps == bps).expect("known band");
            (row.bid, row.ask)
        };
        let (half_bid, half_ask) = band_at(50);
        let (one_bid, one_ask) = band_at(100);
        let l10 = (sum_levels(bids, 10), sum_levels(asks, 10));
        let imbalances = [
            ImbalanceRow {
                label: "L1",
                bid: touch.bid_qty,
                ask: touch.ask_qty,
            },
            ImbalanceRow {
                label: "L5",
                bid: sum_levels(bids, 5),
                ask: sum_levels(asks, 5),
            },
            ImbalanceRow {
                label: "L10",
                bid: l10.0,
                ask: l10.1,
            },
            ImbalanceRow {
                label: "0.5%",
                bid: half_bid,
                ask: half_ask,
            },
            ImbalanceRow {
                label: "1.0%",
                bid: one_bid,
                ask: one_ask,
            },
        ];

        let impact = IMPACT_NOTIONALS
            .iter()
            .map(|&notional| ImpactRow {
                notional,
                buy_bps: slippage_bps(asks, Decimal::from(notional), mid, true),
                sell_bps: slippage_bps(bids, Decimal::from(notional), mid, false),
            })
            .collect();

        let bid_total: Decimal = bids.iter().map(|(_, qty)| *qty).sum();
        let ask_total: Decimal = asks.iter().map(|(_, qty)| *qty).sum();
        let avg = |total: Decimal, n: usize| total / Decimal::from(n.max(1));
        let avg_bid_order = avg(bid_total, bids.len());
        let avg_ask_order = avg(ask_total, asks.len());

        Some(Analysis {
            panel,
            tick,
            price_dp,
            touch,
            spread_ticks: panel.spread / tick,
            micro_offset_ticks: (panel.micro - mid) / tick,
            bid_orders: bids.len(),
            ask_orders: asks.len(),
            avg_bid_order,
            avg_ask_order,
            imbalances,
            l10_imbalance: imbalance(l10.0, l10.1),
            bands,
            impact,
            big_bid: biggest_order(bids, mid, avg_bid_order),
            big_ask: biggest_order(asks, mid, avg_ask_order),
        })
    }
}

/// Slippage of a market order of `notional` quote units versus mid, in bps.
/// `None` when the visible book cannot fill it.
pub(super) fn slippage_bps(
    orders: Orders,
    notional: Decimal,
    mid: Decimal,
    is_buy: bool,
) -> Option<Decimal> {
    let mut remaining = notional;
    let mut base = Decimal::ZERO;
    for &(price, qty) in orders {
        if price.is_zero() {
            continue;
        }
        let value = price * qty;
        if value >= remaining {
            base += remaining / price;
            remaining = Decimal::ZERO;
            break;
        }
        remaining -= value;
        base += qty;
    }
    if !remaining.is_zero() || base.is_zero() || mid.is_zero() {
        return None;
    }
    let vwap = notional / base;
    let diff = if is_buy { vwap - mid } else { mid - vwap };
    Some(diff / mid * Decimal::from(BPS))
}

fn biggest_order(orders: Orders, mid: Decimal, avg: Decimal) -> Option<BigOrder> {
    let mut best: Option<(Decimal, Decimal)> = None;
    for &(price, qty) in orders {
        if best.is_none_or(|(_, best_qty)| qty > best_qty) {
            best = Some((price, qty));
        }
    }
    let (price, qty) = best?;
    let signed = price - mid;
    Some(BigOrder {
        price,
        qty,
        from_mid_bps: signed / mid * Decimal::from(BPS),
        multiple: if avg.is_zero() {
            Decimal::ZERO
        } else {
            qty / avg
        },
    })
}

// ----- order flow imbalance -----------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Touch {
    pub bid_price: Decimal,
    pub bid_qty: Decimal,
    pub ask_price: Decimal,
    pub ask_qty: Decimal,
}

/// One step of order flow imbalance (Cont, Kukanov, Stoikov): positive when buying
/// pressure at the touch grew, negative when selling pressure did.
pub(super) fn ofi_step(prev: &Touch, cur: &Touch) -> Decimal {
    let bid = if cur.bid_price > prev.bid_price {
        cur.bid_qty
    } else if cur.bid_price == prev.bid_price {
        cur.bid_qty - prev.bid_qty
    } else {
        -prev.bid_qty
    };
    let ask = if cur.ask_price < prev.ask_price {
        cur.ask_qty
    } else if cur.ask_price == prev.ask_price {
        cur.ask_qty - prev.ask_qty
    } else {
        -prev.ask_qty
    };
    bid - ask
}

// ----- small statistics ---------------------------------------------------------------

/// Nearest-rank percentile of an ascending slice.
pub(super) fn percentile(sorted: &[i64], pct: u32) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (sorted.len() * pct as usize).div_ceil(100).max(1);
    Some(sorted[rank.min(sorted.len()) - 1])
}

/// Map a series to `0..=7` block heights using its own min and max.
pub(super) fn scale_series(values: &[f64]) -> Vec<u8> {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    values
        .iter()
        .map(|value| {
            if range <= f64::EPSILON {
                0
            } else {
                (((value - min) / range) * 7.0).round() as u8
            }
        })
        .collect()
}

pub(super) fn to_f64(value: Decimal) -> f64 {
    value.to_f64().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(value: &str) -> Decimal {
        value.parse().unwrap()
    }

    fn book(levels: &[(&str, &str)]) -> Vec<(Decimal, Decimal)> {
        levels.iter().map(|(p, q)| (d(p), d(q))).collect()
    }

    fn sample() -> (Vec<(Decimal, Decimal)>, Vec<(Decimal, Decimal)>) {
        let asks = book(&[
            ("100.2", "1"),
            ("100.2", "2"),
            ("100.3", "4"),
            ("100.5", "3"),
        ]);
        let bids = book(&[
            ("100.1", "2"),
            ("100.0", "1"),
            ("100.0", "1"),
            ("99.5", "8"),
        ]);
        (asks, bids)
    }

    #[test]
    fn ladder_aggregates_orders_per_level() {
        let (asks, _) = sample();
        let levels = ladder(&asks, true, None, 10);
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0].price, d("100.2"));
        assert_eq!(levels[0].qty, d("3"));
        assert_eq!(levels[0].orders, 2);
        assert_eq!(levels[2].cum, d("10"));
    }

    #[test]
    fn ladder_limits_rows() {
        let (asks, _) = sample();
        assert_eq!(ladder(&asks, true, None, 2).len(), 2);
    }

    #[test]
    fn ladder_groups_asks_up_and_bids_down() {
        let (asks, bids) = sample();
        let ask_levels = ladder(&asks, true, Some(d("0.5")), 10);
        assert_eq!(ask_levels[0].price, d("100.5"));
        assert_eq!(ask_levels[0].qty, d("10"));
        let bid_levels = ladder(&bids, false, Some(d("0.5")), 10);
        assert_eq!(bid_levels[0].price, d("100.0"));
        assert_eq!(bid_levels[0].qty, d("4"));
        assert_eq!(bid_levels[1].price, d("99.5"));
    }

    #[test]
    fn sum_levels_counts_distinct_prices() {
        let (asks, _) = sample();
        assert_eq!(sum_levels(&asks, 1), d("3"));
        assert_eq!(sum_levels(&asks, 2), d("7"));
        assert_eq!(sum_levels(&asks, 99), d("10"));
    }

    #[test]
    fn analysis_tick_spread_and_imbalance() {
        let (asks, bids) = sample();
        let a = Analysis::from_book(&asks, &bids).unwrap();
        assert_eq!(a.tick, d("0.1"));
        assert_eq!(a.price_dp, 1);
        assert_eq!(a.spread_ticks, d("1"));
        assert_eq!(a.touch.bid_qty, d("2"));
        assert_eq!(a.touch.ask_qty, d("3"));
        // L1: (2 - 3) / 5
        assert_eq!(
            imbalance(a.imbalances[0].bid, a.imbalances[0].ask),
            d("-0.2")
        );
        assert_eq!(a.bid_orders, 4);
        assert_eq!(a.ask_orders, 4);
        assert!(!a.is_crossed() && !a.is_locked());
    }

    #[test]
    fn microprice_leans_toward_the_thin_side() {
        let (asks, bids) = sample();
        let a = Analysis::from_book(&asks, &bids).unwrap();
        // Bid touch 2, ask touch 3 -> micro = (100.2*2 + 100.1*3) / 5 = 100.14, below mid 100.15.
        assert_eq!(a.panel.micro, d("100.14"));
        assert!(a.micro_offset_ticks < Decimal::ZERO);
    }

    #[test]
    fn bands_use_basis_points_from_mid() {
        let (asks, bids) = sample();
        let a = Analysis::from_book(&asks, &bids).unwrap();
        // 25 bps of 100.15 = 0.25: asks up to 100.40 (7), bids down to 99.90 (4).
        let row = a.bands.iter().find(|row| row.bps == 25).unwrap();
        assert_eq!(row.ask, d("7"));
        assert_eq!(row.bid, d("4"));
    }

    #[test]
    fn slippage_is_vwap_against_mid() {
        let asks = book(&[("101", "1"), ("102", "10")]);
        let mid = d("100");
        // Buy 203 quote units: 1 @ 101 (101) + 102 / 102 = 1 @ 102 -> vwap 203 / 2 = 101.5
        let bps = slippage_bps(&asks, d("203"), mid, true).unwrap();
        assert_eq!(bps, d("150"));
        assert!(slippage_bps(&asks, d("1000000"), mid, true).is_none());
    }

    #[test]
    fn sell_slippage_is_positive_when_bids_are_below_mid() {
        let bids = book(&[("99", "10")]);
        let bps = slippage_bps(&bids, d("99"), d("100"), false).unwrap();
        assert_eq!(bps, d("100"));
    }

    #[test]
    fn biggest_order_flags_walls() {
        let asks = book(&[("101", "1"), ("102", "1"), ("103", "30")]);
        let avg = d("32") / d("3");
        let big = biggest_order(&asks, d("100"), avg).unwrap();
        assert_eq!(big.price, d("103"));
        assert_eq!(big.from_mid_bps, d("300"));
        assert!(!big.is_wall());
        let big = biggest_order(&asks, d("100"), d("1")).unwrap();
        assert!(big.is_wall());
    }

    fn touch(bp: &str, bq: &str, ap: &str, aq: &str) -> Touch {
        Touch {
            bid_price: d(bp),
            bid_qty: d(bq),
            ask_price: d(ap),
            ask_qty: d(aq),
        }
    }

    #[test]
    fn ofi_positive_when_bid_size_grows() {
        let prev = touch("100", "5", "101", "5");
        let cur = touch("100", "8", "101", "5");
        assert_eq!(ofi_step(&prev, &cur), d("3"));
    }

    #[test]
    fn ofi_negative_when_ask_size_grows_or_bid_retreats() {
        let prev = touch("100", "5", "101", "5");
        assert_eq!(ofi_step(&prev, &touch("100", "5", "101", "9")), d("-4"));
        assert_eq!(ofi_step(&prev, &touch("99.9", "7", "101", "5")), d("-5"));
    }

    #[test]
    fn ofi_positive_when_ask_lifts() {
        let prev = touch("100", "5", "101", "5");
        // Ask level of 5 disappeared and the next one is higher: buyers lifted it.
        assert_eq!(ofi_step(&prev, &touch("100", "5", "101.1", "2")), d("5"));
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];
        assert_eq!(percentile(&values, 50), Some(50));
        assert_eq!(percentile(&values, 99), Some(100));
        assert_eq!(percentile(&[], 50), None);
    }

    #[test]
    fn scale_series_spans_zero_to_seven() {
        assert_eq!(scale_series(&[1.0, 2.0, 3.0]), vec![0, 4, 7]);
        assert_eq!(scale_series(&[5.0, 5.0]), vec![0, 0]);
    }

    #[test]
    fn crossed_and_locked_books_are_flagged() {
        let asks = book(&[("100", "1")]);
        let locked = Analysis::from_book(&asks, &book(&[("100", "1")])).unwrap();
        assert!(locked.is_locked());
        let crossed = Analysis::from_book(&asks, &book(&[("100.5", "1")])).unwrap();
        assert!(crossed.is_crossed());
    }
}
