fn main() {
    divan::main();
}

fn bench_serialize(bencher: divan::Bencher, units: u64) {
    let mut out = String::with_capacity(20);
    bencher.bench_local(|| {
        out.clear();
        rust_order_book_mirror::level3::units::serialize_units(divan::black_box(units), &mut out);
        out.len()
    });
}

#[divan::bench]
fn price(bencher: divan::Bencher) {
    // 75644.7 at the BTC/USD tick: six digits.
    bench_serialize(bencher, 756_447);
}

#[divan::bench]
fn qty(bencher: divan::Bencher) {
    // 1.32196829 at the BTC/USD step: nine digits.
    bench_serialize(bencher, 132_196_829);
}

#[divan::bench]
fn small_qty(bencher: divan::Bencher) {
    // 0.00005100 at the step: four digits.
    bench_serialize(bencher, 5_100);
}
