fn main() {
    divan::main();
}

fn decimal(value: &str) -> rust_decimal::Decimal {
    value.parse().expect("decimal")
}

#[divan::bench]
fn price_exact_scale(bencher: divan::Bencher) {
    // Wire scale matches the BTC/USD tick (0.1).
    let value = decimal("75644.7");
    bencher.bench_local(|| {
        rust_order_book_mirror::level3::units::to_units(
            divan::black_box(value),
            divan::black_box(1u8),
        )
        .expect("price units")
    });
}

#[divan::bench]
fn qty_exact_scale(bencher: divan::Bencher) {
    // Wire scale matches the BTC/USD step (1e-8).
    let value = decimal("1.32196829");
    bencher.bench_local(|| {
        rust_order_book_mirror::level3::units::to_units(
            divan::black_box(value),
            divan::black_box(8u8),
        )
        .expect("qty units")
    });
}

#[divan::bench]
fn qty_coarser_than_scale(bencher: divan::Bencher) {
    // Fewer fractional digits than the step: multiply by 10^(scale - value_scale).
    let value = decimal("1.5");
    bencher.bench_local(|| {
        rust_order_book_mirror::level3::units::to_units(
            divan::black_box(value),
            divan::black_box(8u8),
        )
        .expect("qty units")
    });
}

#[divan::bench]
fn price_finer_than_scale(bencher: divan::Bencher) {
    // Extra fractional digit that divides evenly into the tick.
    let value = decimal("75644.70");
    bencher.bench_local(|| {
        rust_order_book_mirror::level3::units::to_units(
            divan::black_box(value),
            divan::black_box(1u8),
        )
        .expect("price units")
    });
}
