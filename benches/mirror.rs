use std::fmt::Write;

use rust_order_book_mirror::kraken::level3::mirror::OrderBookMirror;

const DEPTH: usize = 1000;
const PRICE_SCALE: u8 = 1;
const QTY_SCALE: u8 = 8;
// Orders per level, cycled over the book. Shaped after
// `tests/data/btc_usd_stream.log`: median 2, tail around 10, and size-1 levels
// that a single delete clears.
const LEVEL_SIZES: [usize; 10] = [1, 2, 1, 3, 2, 1, 6, 2, 1, 10];
// Seconds between the first order of adjacent levels. Larger than any level
// size, so an added order takes the next slot and sorts after the others.
const LEVEL_STRIDE: usize = 16;
// Ticks between adjacent levels. The free tick between two levels lets an add
// open a new price level inside the book.
const LEVEL_TICKS: usize = 2;
const MIDDLE_LEVEL: usize = 50;

const STREAM_LOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/btc_usd_stream.log");
// The log was captured with depth 10.
const STREAM_DEPTH: usize = 10;

fn main() {
    divan::main();
}

// Frame patterns seen in the captured stream (4891 updates):
// - single add, new level or joining one: ~50%
// - delete that clears a level, then adds that backfill the deepest level: ~27%
// - single delete that leaves the level non-empty (cancel): ~22%
// - single modify at the touch (partial fill): ~1.4%
#[derive(Clone, Copy)]
enum Pattern {
    AddNewLevel,
    AddJoin,
    Cancel,
    Fill,
    ClearBackfill(usize),
}

#[derive(Clone, Copy)]
enum Place {
    Begin,
    Middle,
    End,
}

#[derive(Clone, Copy)]
struct Case {
    pattern: Pattern,
    place: Place,
}

const fn case(pattern: Pattern, place: Place) -> Case {
    Case { pattern, place }
}

const CASES: [Case; 16] = [
    case(Pattern::AddNewLevel, Place::Begin),
    case(Pattern::AddNewLevel, Place::Middle),
    case(Pattern::AddNewLevel, Place::End),
    case(Pattern::AddJoin, Place::Begin),
    case(Pattern::AddJoin, Place::Middle),
    case(Pattern::AddJoin, Place::End),
    case(Pattern::Cancel, Place::Begin),
    case(Pattern::Cancel, Place::Middle),
    case(Pattern::Cancel, Place::End),
    case(Pattern::Fill, Place::Begin),
    case(Pattern::ClearBackfill(1), Place::Begin),
    case(Pattern::ClearBackfill(1), Place::Middle),
    case(Pattern::ClearBackfill(1), Place::End),
    case(Pattern::ClearBackfill(4), Place::Begin),
    case(Pattern::ClearBackfill(4), Place::Middle),
    case(Pattern::ClearBackfill(4), Place::End),
];

impl std::fmt::Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.pattern {
            Pattern::AddNewLevel => f.write_str("add_new_level")?,
            Pattern::AddJoin => f.write_str("add_join")?,
            Pattern::Cancel => f.write_str("cancel")?,
            Pattern::Fill => f.write_str("fill")?,
            Pattern::ClearBackfill(adds) => write!(f, "clear_backfill_{adds}")?,
        }
        let place = match self.place {
            Place::Begin => "begin",
            Place::Middle => "middle",
            Place::End => "end",
        };
        write!(f, "/{place}")
    }
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

// One update on a book that already holds the snapshot. `with_inputs` rebuilds
// that book outside the timer. `bench_local_refs` keeps the mirror and drops it
// after the timed call, so the sample is `on_message` and not book teardown.
#[divan::bench(args = CASES)]
fn on_update(bencher: divan::Bencher, case: Case) {
    let prepared = prepare(case);
    bencher
        .with_inputs(|| {
            let mut mirror = OrderBookMirror::new(DEPTH, PRICE_SCALE, QTY_SCALE);
            mirror
                .on_message(&prepared.snapshot)
                .unwrap_or_else(|err| panic!("{case} snapshot: {err}"));
            mirror
        })
        .bench_local_refs(|mirror| {
            mirror
                .on_message(divan::black_box(&prepared.update))
                .unwrap_or_else(|err| panic!("{case} update: {err}"));
            divan::black_box(mirror.get_order_book().expect("book").asks.len())
        });
}

// Every update frame of the captured BTC/USD stream, in order, on a depth-10
// book. The snapshot apply is outside the timer.
#[divan::bench]
fn replay_stream(bencher: divan::Bencher) {
    let log = std::fs::read_to_string(STREAM_LOG).expect("stream log");
    let mut lines = log.lines();
    let snapshot = lines.next().expect("snapshot line");
    let updates: Vec<&str> = lines.collect();
    bencher
        .with_inputs(|| {
            let mut mirror = OrderBookMirror::new(STREAM_DEPTH, PRICE_SCALE, QTY_SCALE);
            mirror.on_message(snapshot).expect("snapshot");
            mirror
        })
        .bench_local_refs(|mirror| {
            for update in &updates {
                mirror
                    .on_message(divan::black_box(update))
                    .unwrap_or_else(|err| panic!("stream update: {err}"));
            }
            divan::black_box(mirror.get_order_book().expect("book").asks.len())
        });
}

struct Prepared {
    snapshot: String,
    update: String,
}

fn prepare(case: Case) -> Prepared {
    let snapshot = snapshot_frame();
    let update = stamp_checksum(&snapshot, update_frame(case));
    let mut mirror = OrderBookMirror::new(DEPTH, PRICE_SCALE, QTY_SCALE);
    mirror
        .on_message(&snapshot)
        .unwrap_or_else(|err| panic!("{case} snapshot: {err}"));
    mirror
        .on_message(&update)
        .unwrap_or_else(|err| panic!("{case} stamped update: {err}"));
    let asks = &mirror.get_order_book().expect("book").asks;
    let levels = asks
        .iter()
        .map(|order| order.limit_price)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert_eq!(levels, DEPTH, "{case}: ask levels after update");
    Prepared { snapshot, update }
}

fn stamp_checksum(snapshot: &str, mut update: String) -> String {
    let mut mirror = OrderBookMirror::new(DEPTH, PRICE_SCALE, QTY_SCALE);
    mirror.on_message(snapshot).expect("snapshot");
    let message = match mirror.on_message(&update) {
        Ok(()) => return update,
        Err(err) => err.to_string(),
    };
    let Some((_, after)) = message.split_once("Calculated: ") else {
        panic!("update failed: {message}");
    };
    let calculated = after.split(['.', ' ']).next().unwrap();
    if calculated.is_empty() || !calculated.bytes().all(|byte| byte.is_ascii_digit()) {
        panic!("update failed: {message}");
    }
    let needle = "\"checksum\":0";
    let Some(at) = update.find(needle) else {
        panic!("update is missing checksum");
    };
    update.replace_range(at..at + needle.len(), &format!("\"checksum\":{calculated}"));
    update
}

fn snapshot_frame() -> String {
    let mut out = String::new();
    // Bids high to low, asks low to high. Slots at a price are oldest first.
    out.push_str(
        r#"{"channel":"level3","type":"snapshot","data":[{"checksum":0,"symbol":"BTC/USD","timestamp":"2026-01-01T00:00:00Z","bids":"#,
    );
    push_side(&mut out, 'b', bid_price);
    out.push_str(r#","asks":"#);
    push_side(&mut out, 'a', ask_price);
    out.push_str("}]}");
    out
}

// One ask event as written on the wire.
struct Event {
    kind: &'static str,
    order_id: String,
    price: String,
    qty: &'static str,
    ts_secs: usize,
}

impl Event {
    // An order that the snapshot holds at `level`, `slot`.
    fn existing(kind: &'static str, level: usize, slot: usize, qty: &'static str) -> Event {
        Event {
            kind,
            order_id: order_id('a', level, slot),
            price: ask_price(level),
            qty,
            ts_secs: slot_secs(level, slot),
        }
    }
}

fn update_frame(case: Case) -> String {
    let events = match case.pattern {
        // A new price on the free tick just inside the picked level. The book
        // then has DEPTH + 1 levels and the truncate drops the last one.
        Pattern::AddNewLevel => {
            let level = pick_level(case.place, |_| true);
            let ticks = ask_ticks(level) - 1;
            vec![Event {
                kind: "add",
                order_id: "a-new".to_string(),
                price: format_ticks(ticks),
                qty: "1.00000000",
                ts_secs: slot_secs(DEPTH + 1, 0),
            }]
        }
        // A later order at an existing price, appended after the others.
        Pattern::AddJoin => {
            let level = pick_level(case.place, |_| true);
            vec![Event::existing(
                "add",
                level,
                level_size(level),
                "1.00000000",
            )]
        }
        // A middle order leaves; the level keeps the rest.
        Pattern::Cancel => {
            let level = pick_level(case.place, |size| size >= 3);
            vec![Event::existing(
                "delete",
                level,
                level_size(level) / 2,
                "1.00000000",
            )]
        }
        // A partial fill of the oldest order at the touch: same timestamp,
        // lower quantity, written in place.
        Pattern::Fill => {
            let level = pick_level(case.place, |_| true);
            vec![Event::existing("modify", level, 0, "0.50000000")]
        }
        // The only order at a level leaves. Kraken then sends every order at
        // the price that just entered the top DEPTH, past the deepest level.
        Pattern::ClearBackfill(adds) => {
            let level = pick_level(case.place, |size| size == 1);
            let mut events = vec![Event::existing("delete", level, 0, "1.00000000")];
            events.extend((0..adds).map(|slot| Event::existing("add", DEPTH, slot, "1.00000000")));
            events
        }
    };
    let mut out = String::new();
    out.push_str(
        r#"{"channel":"level3","type":"update","data":[{"checksum":0,"symbol":"BTC/USD","timestamp":"2026-01-01T12:00:00Z","bids":[],"asks":["#,
    );
    for (i, event) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_order(
            &mut out,
            &event.order_id,
            &event.price,
            event.qty,
            event.ts_secs,
            Some(event.kind),
        );
    }
    out.push_str("]}]}");
    out
}

// The first level at `place`, walking toward the inside of the book, whose
// order count satisfies `accept`.
fn pick_level(place: Place, accept: impl Fn(usize) -> bool) -> usize {
    let found = match place {
        Place::Begin => (0..DEPTH).find(|&level| accept(level_size(level))),
        Place::Middle => (MIDDLE_LEVEL..DEPTH).find(|&level| accept(level_size(level))),
        Place::End => (0..DEPTH).rev().find(|&level| accept(level_size(level))),
    };
    found.expect("no level matches")
}

fn push_side(out: &mut String, side: char, price: fn(usize) -> String) {
    out.push('[');
    for level in 0..DEPTH {
        let price = price(level);
        for slot in 0..level_size(level) {
            if level > 0 || slot > 0 {
                out.push(',');
            }
            push_order(
                out,
                &order_id(side, level, slot),
                &price,
                "1.00000000",
                slot_secs(level, slot),
                None,
            );
        }
    }
    out.push(']');
}

fn push_order(
    out: &mut String,
    order_id: &str,
    price: &str,
    qty: &str,
    ts_secs: usize,
    event: Option<&str>,
) {
    out.push('{');
    if let Some(event) = event {
        let _ = write!(out, "\"event\":\"{event}\",");
    }
    let _ = write!(
        out,
        "\"order_id\":\"{order_id}\",\"limit_price\":{price},\"order_qty\":{qty},\"timestamp\":\"{}\"}}",
        timestamp(ts_secs),
    );
}

fn level_size(level: usize) -> usize {
    LEVEL_SIZES[level % LEVEL_SIZES.len()]
}

fn order_id(side: char, level: usize, slot: usize) -> String {
    format!("{side}-{level}-{slot}")
}

fn slot_secs(level: usize, slot: usize) -> usize {
    level * LEVEL_STRIDE + slot
}

fn ask_ticks(level: usize) -> usize {
    1_000_000 + LEVEL_TICKS * level
}

fn ask_price(level: usize) -> String {
    format_ticks(ask_ticks(level))
}

fn bid_price(level: usize) -> String {
    format_ticks(999_999 - LEVEL_TICKS * level)
}

fn format_ticks(ticks: usize) -> String {
    format!("{}.{}", ticks / 10, ticks % 10)
}

fn timestamp(total_secs: usize) -> String {
    let hours = total_secs / 3600;
    let mins = total_secs / 60 % 60;
    let secs = total_secs % 60;
    format!("2026-01-01T{hours:02}:{mins:02}:{secs:02}Z")
}
