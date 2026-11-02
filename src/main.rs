use clap::{Parser, Subcommand};
use rust_order_book_mirror::cmds;
use std::error::Error;

#[derive(Parser)]
#[command(
    name = "kraken-order-book-mirror",
    version,
    about = "Kraken order book mirror"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the live order book dashboard for a pair
    Dashboard {
        /// Trading pair, e.g. BTC/USD
        pair: String,
        /// Book depth
        #[arg(default_value_t = 1000)]
        depth: usize,
    },
    /// Dump the order book for a pair
    Dump {
        /// Trading pair, e.g. BTC/USD
        pair: String,
        /// Book depth
        #[arg(default_value_t = 1000)]
        depth: usize,
    },
    /// Run a benchmark from a file
    Bench {
        /// Input file
        file: String,
        /// Trading pair, e.g. BTC/USD
        pair: String,
        /// Book depth
        #[arg(default_value_t = 1000)]
        depth: usize,
    },
}

pub fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Dashboard { pair, depth } => cmds::dashboard::run(pair, depth),
        Command::Dump { pair, depth } => cmds::dump::run(pair, depth),
        Command::Bench { file, pair, depth } => cmds::bench::run(file, pair, depth),
    }
}
