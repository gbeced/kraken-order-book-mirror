# rust-order-book-mirror

## Live client

Export `KRAKEN_API_KEY` and `KRAKEN_API_SECRET`. The binary does not load `.env`. With a local `.env`:

```bash
set -a
source .env
set +a
cargo run
```

`cargo run` connects to Kraken and prints one summary line per applied level3 frame. It does not send orders.

## Benchmark

```bash
cargo run --release -- bench [FILE] [--depth N]
```

`FILE` defaults to `raw_messages.log`. `--depth` defaults to `100` and must match the depth that produced the file. The command times `OrderBookMirror::on_message` for each message and prints per-update latency at p50, p90, p95, p99, and p99.9. Use `--release` for comparable numbers.
