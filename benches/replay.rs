fn main() {
    divan::main();
}

// Depth matches the tmp/raw_messages.log capture. One sample: the replay is the whole log.
#[divan::bench(sample_count = 1, sample_size = 1)]
fn on_message_replay(bencher: divan::Bencher) {
    let messages =
        rust_order_book_mirror::bench::load_messages(std::path::Path::new("tmp/raw_messages.log"))
            .expect("failed to read tmp/raw_messages.log");
    if messages.is_empty() {
        panic!("no messages in tmp/raw_messages.log");
    }

    bencher.bench_local(move || {
        rust_order_book_mirror::bench::replay(&messages, 100).expect("replay failed")
    });
}
