# rust-order-book-mirror

## Live client

Export `KRAKEN_API_KEY` and `KRAKEN_API_SECRET`. The binary does not load `.env`. With a local `.env`:

```bash
set -a
source .env
set +a
cargo run
```

`cargo run` connects to Kraken and fills the terminal with ratatui order-book cards. It does not send orders. For BTC/USD it subscribes at depth `1000`, and passes a price scale of 1 (tick `0.1`) and a quantity scale of 8 (step `1e-8`). The book stores each price and quantity as a count of that increment.

## Benchmark

```bash
cargo run --release -- bench FILE [--depth N]
```

`FILE` is the captured level3 log. `--depth` defaults to `100` and must match the depth that produced the file. The command times `OrderBookMirror::on_message` for each message and prints per-update latency at p50, p90, p95, p99, and p99.9. Use `--release` for comparable numbers.

CodSpeed runs the Divan benches under CPU simulation. In GitHub Actions, run the "CodSpeed Benchmarks" workflow. A push to `master` and a pull request run it as well.
