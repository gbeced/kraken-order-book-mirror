use rust_order_book_mirror::common::units;

fn main() {
    divan::main();
}

#[divan::bench]
fn price_exact_scale(bencher: divan::Bencher) {
    // Wire scale matches the BTC/USD tick (0.1).
    let value = "75644.7";
    bencher.bench_local(|| {
        units::parse_units(divan::black_box(value), divan::black_box(1u8)).expect("price units")
    });
}

#[divan::bench]
fn qty_exact_scale(bencher: divan::Bencher) {
    // Wire scale matches the BTC/USD step (1e-8).
    let value = "1.32196829";
    bencher.bench_local(|| {
        units::parse_units(divan::black_box(value), divan::black_box(8u8)).expect("qty units")
    });
}

#[divan::bench]
fn qty_coarser_than_scale(bencher: divan::Bencher) {
    // Fewer fractional digits than the step: multiply by 10^(scale - value_scale).
    let value = "1.5";
    bencher.bench_local(|| {
        units::parse_units(divan::black_box(value), divan::black_box(8u8)).expect("qty units")
    });
}

#[divan::bench]
fn price_finer_than_scale(bencher: divan::Bencher) {
    // Extra fractional digit that divides evenly into the tick.
    let value = "75644.70";
    bencher.bench_local(|| {
        units::parse_units(divan::black_box(value), divan::black_box(1u8)).expect("price units")
    });
}
