fn main() {
    divan::main();
}

#[divan::bench]
fn on_message_replay(bencher: divan::Bencher) {
    let messages =
        rust_order_book_mirror::bench::load_messages(std::path::Path::new("tmp/1k_messages.log"))
            .expect("failed to read tmp/1k_messages.log");
    if messages.is_empty() {
        panic!("no messages in tmp/1k_messages.log");
    }

    bencher.bench_local(move || {
        rust_order_book_mirror::bench::replay(&messages, 100).expect("replay failed")
    });
}
