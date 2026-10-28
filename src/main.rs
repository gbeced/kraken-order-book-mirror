use std::error::Error;

pub fn main() -> Result<(), Box<dyn Error>> {
    match std::env::args().nth(1).as_deref() {
        Some("bench") => rust_order_book_mirror::bench::run(),
        Some("mirror") => rust_order_book_mirror::mirror::run(),
        Some(cmd) => Err(format!("Unsupported command '{cmd}'").into()),
        None => rust_order_book_mirror::mirror::run(),
    }
}
