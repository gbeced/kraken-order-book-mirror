use std::hint::black_box;

use gungraun::{
    Callgrind, LibraryBenchmarkConfig, library_benchmark, library_benchmark_group, main,
};
use rust_order_book_mirror::common::units;

// Each benchmark is a single cold run, so first touches of code and data
// would show up as cache misses that depend on the binary's layout, not on
// `parse_units`. One call outside the measured function warms those lines.
fn warm_up(value: &'static str, scale: u8) -> (&'static str, u8) {
    black_box(units::parse_units(black_box(value), black_box(scale)).expect("units"));
    (value, scale)
}

#[library_benchmark(setup = warm_up)]
// Wire scale matches the BTC/USD tick (0.1).
#[bench::price_exact_scale("75644.7", 1)]
// Wire scale matches the BTC/USD step (1e-8).
#[bench::qty_exact_scale("1.32196829", 8)]
// Fewer fractional digits than the step: multiply by 10^(scale - value_scale).
#[bench::qty_coarser_than_scale("1.5", 8)]
// Extra fractional digit that divides evenly into the tick.
#[bench::price_finer_than_scale("75644.70", 1)]
fn parse_units(input: (&'static str, u8)) -> u64 {
    let (value, scale) = input;
    black_box(units::parse_units(black_box(value), black_box(scale)).expect("units"))
}

library_benchmark_group!(name = parse_units_group, benchmarks = parse_units);

// Fixed cache geometry so cache metrics don't depend on the host CPU.
main!(
    config = LibraryBenchmarkConfig::default().tool(Callgrind::with_args([
        "--I1=32768,8,64",
        "--D1=32768,8,64",
        "--LL=8388608,16,64",
    ])),
    library_benchmark_groups = parse_units_group
);
