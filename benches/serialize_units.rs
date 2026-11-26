use std::hint::black_box;

use gungraun::{
    Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main,
};
use rust_order_book_mirror::common::units;

// The output buffer is allocated outside the measured function.
fn with_buffer(units: u64) -> (u64, String) {
    (units, String::with_capacity(20))
}

#[library_benchmark(setup = with_buffer)]
// 75644.7 at the BTC/USD tick: six digits.
#[bench::price(756_447)]
// 1.32196829 at the BTC/USD step: nine digits.
#[bench::qty(132_196_829)]
// 0.00005100 at the step: four digits.
#[bench::small_qty(5_100)]
fn serialize_units(input: (u64, String)) -> String {
    let (units, mut out) = input;
    units::serialize_units(black_box(units), &mut out);
    // Returned so the buffer is dropped outside the measured function.
    black_box(out)
}

library_benchmark_group!(name = serialize_units_group, benchmarks = serialize_units);

// Fixed cache geometry so cache metrics don't depend on the host CPU.
main!(
    config = LibraryBenchmarkConfig::default().tool(Callgrind::with_args([
        "--I1=32768,8,64",
        "--D1=32768,8,64",
        "--LL=8388608,16,64",
    ])),
    library_benchmark_groups = serialize_units_group
);
