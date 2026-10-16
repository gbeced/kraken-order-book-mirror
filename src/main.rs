use std::error::Error;

pub fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("bench") {
        rust_order_book_mirror::bench::run()
    } else {
        rust_order_book_mirror::mirror::run()
    }
}
