mod bench;
mod kraken;
mod mirror;

use std::error::Error;

pub fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("bench") {
        return bench::run();
    }
    return mirror::run();
}
