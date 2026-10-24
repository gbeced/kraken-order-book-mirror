use std::fmt::Write;

use rust_order_book_mirror::level3::mirror::OrderBookMirror;

const DEPTH: usize = 100;
const PRICE_SCALE: u8 = 1;
const QTY_SCALE: u8 = 8;
// Seconds between the first order of adjacent levels. An added order uses the
// next slot, so it sorts after the orders already at that price.
const LEVEL_STRIDE: usize = 4;
const ORDERS_SINGLE: usize = 1;
const ORDERS_MULTI: usize = 4;

fn main() {
    divan::main();
}

#[derive(Clone, Copy)]
enum Event {
    Add,
    Modify,
    Delete,
}

#[derive(Clone, Copy)]
enum Place {
    Begin,
    Middle,
    End,
}

#[derive(Clone, Copy)]
enum Shape {
    Single,
    Multi,
}

#[derive(Clone, Copy)]
struct Case {
    event: Event,
    place: Place,
    shape: Shape,
}

const fn case(event: Event, place: Place, shape: Shape) -> Case {
    Case {
        event,
        place,
        shape,
    }
}

const CASES: [Case; 18] = [
    case(Event::Add, Place::Begin, Shape::Single),
    case(Event::Add, Place::Middle, Shape::Single),
    case(Event::Add, Place::End, Shape::Single),
    case(Event::Add, Place::Begin, Shape::Multi),
    case(Event::Add, Place::Middle, Shape::Multi),
    case(Event::Add, Place::End, Shape::Multi),
    case(Event::Modify, Place::Begin, Shape::Single),
    case(Event::Modify, Place::Middle, Shape::Single),
    case(Event::Modify, Place::End, Shape::Single),
    case(Event::Modify, Place::Begin, Shape::Multi),
    case(Event::Modify, Place::Middle, Shape::Multi),
    case(Event::Modify, Place::End, Shape::Multi),
    case(Event::Delete, Place::Begin, Shape::Single),
    case(Event::Delete, Place::Middle, Shape::Single),
    case(Event::Delete, Place::End, Shape::Single),
    case(Event::Delete, Place::Begin, Shape::Multi),
    case(Event::Delete, Place::Middle, Shape::Multi),
    case(Event::Delete, Place::End, Shape::Multi),
];

impl std::fmt::Display for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let event = match self.event {
            Event::Add => "add",
            Event::Modify => "modify",
            Event::Delete => "delete",
        };
        let place = match self.place {
            Place::Begin => "begin",
            Place::Middle => "middle",
            Place::End => "end",
        };
        let shape = match self.shape {
            Shape::Single => "single",
            Shape::Multi => "multi",
        };
        write!(f, "{event}/{place}/{shape}")
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

struct Prepared {
    snapshot: String,
    update: String,
}

fn prepare(case: Case) -> Prepared {
    let snapshot = snapshot_frame(case.shape);
    let update = stamp_checksum(&snapshot, update_frame(case));
    let mut mirror = OrderBookMirror::new(DEPTH, PRICE_SCALE, QTY_SCALE);
    mirror
        .on_message(&snapshot)
        .unwrap_or_else(|err| panic!("{case} snapshot: {err}"));
    mirror
        .on_message(&update)
        .unwrap_or_else(|err| panic!("{case} stamped update: {err}"));
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

fn snapshot_frame(shape: Shape) -> String {
    let per_level = orders_per_level(shape);
    let mut out = String::new();
    // Bids high to low, asks low to high. Slots at a price are oldest first.
    out.push_str(
        r#"{"channel":"level3","type":"snapshot","data":[{"checksum":0,"symbol":"BTC/USD","timestamp":"2026-01-01T00:00:00Z","bids":"#,
    );
    push_side(&mut out, 'b', per_level, false);
    out.push_str(r#","asks":"#);
    push_side(&mut out, 'a', per_level, true);
    out.push_str("}]}");
    out
}

fn update_frame(case: Case) -> String {
    let level = level_index(case.place);
    let per_level = orders_per_level(case.shape);
    // Last order at the level, so modify and delete scan the whole price.
    // Modify keeps that timestamp and only changes quantity, which writes in place.
    // Add uses a new id and a later timestamp, so it appends at this price.
    let (event, slot, qty) = match case.event {
        Event::Add => ("add", per_level, "1.00000000"),
        Event::Modify => ("modify", per_level - 1, "0.50000000"),
        Event::Delete => ("delete", per_level - 1, "1.00000000"),
    };
    let mut out = String::new();
    out.push_str(
        r#"{"channel":"level3","type":"update","data":[{"checksum":0,"symbol":"BTC/USD","timestamp":"2026-01-01T01:00:00Z","bids":[],"asks":["#,
    );
    push_order(
        &mut out,
        'a',
        level,
        slot,
        &ask_price(level),
        qty,
        Some(event),
    );
    out.push_str("]}]}");
    out
}

fn push_side(out: &mut String, side: char, per_level: usize, ask: bool) {
    out.push('[');
    for level in 0..DEPTH {
        for slot in 0..per_level {
            if level > 0 || slot > 0 {
                out.push(',');
            }
            let price = if ask {
                ask_price(level)
            } else {
                bid_price(level)
            };
            push_order(out, side, level, slot, &price, "1.00000000", None);
        }
    }
    out.push(']');
}

fn push_order(
    out: &mut String,
    side: char,
    level: usize,
    slot: usize,
    price: &str,
    qty: &str,
    event: Option<&str>,
) {
    out.push('{');
    if let Some(event) = event {
        let _ = write!(out, "\"event\":\"{event}\",");
    }
    let _ = write!(
        out,
        "\"order_id\":\"{side}-{level}-{slot}\",\"limit_price\":{price},\"order_qty\":{qty},\"timestamp\":\"{}\"}}",
        timestamp(level * LEVEL_STRIDE + slot),
    );
}

fn orders_per_level(shape: Shape) -> usize {
    match shape {
        Shape::Single => ORDERS_SINGLE,
        Shape::Multi => ORDERS_MULTI,
    }
}

fn level_index(place: Place) -> usize {
    match place {
        Place::Begin => 0,
        Place::Middle => 50,
        Place::End => DEPTH - 1,
    }
}

fn ask_price(level: usize) -> String {
    format_ticks(1_000_000 + level)
}

fn bid_price(level: usize) -> String {
    format_ticks(999_999 - level)
}

fn format_ticks(ticks: usize) -> String {
    format!("{}.{}", ticks / 10, ticks % 10)
}

fn timestamp(total_secs: usize) -> String {
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("2026-01-01T00:{mins:02}:{secs:02}Z")
}
