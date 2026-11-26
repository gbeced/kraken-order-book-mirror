# Kraken Order Book Mirror

A Rust (Tokio) client that keeps a local, order-by-order copy of a Level 3 Kraken spot order book. It authenticates to Kraken, subscribes to the WebSocket v2 [`level3`](https://docs.kraken.com/api/docs/websocket-v2/level3/) channel for one trading pair, applies every snapshot and update to an in-memory book, and verifies each update against Kraken's CRC32 checksum. If the local book ever disagrees with Kraken, the program stops with an error instead of showing a wrong book.

On top of the mirrored book there is a terminal dashboard, a raw frame recorder, and an offline replay benchmark.

![Order book dashboard](docs/dashboard.png)

See the [dashboard user guide](docs/ui-guide.md) for what each panel shows and how every number is computed.

## Requirements

- Rust 1.99 or newer (`rust-version` in `Cargo.toml`).
- A Kraken API key and secret for `dashboard` and `dump`. `level3` is an authenticated feed, so the key must be allowed to request a WebSocket token. It never needs trading permissions.

The binary does not load `.env`. Export the variables yourself:

```bash
export KRAKEN_API_KEY=...
export KRAKEN_API_SECRET=...
```

Or, from a local `.env` (see `env.example`):

```bash
set -a; source .env; set +a
```

## Build and test

```bash
cargo build --release   # build
cargo test              # unit and integration tests, offline
cargo bench --bench '*' # gungraun benchmarks (Linux + Valgrind)
```

`cargo test` needs no network and no API keys. The integration tests in `tests/mirror.rs` replay the fixtures in `tests/data/` through the mirror.

The benches (`benches/`) cover single-update applies on a 1000-level book, a replay of `tests/data/btc_usd_stream.log`, the checksum, and the unit conversion helpers. They use [gungraun](https://github.com/gungraun/gungraun), which runs each benchmark once under Valgrind's Callgrind and reports instruction counts, simulated cache hits, and estimated cycles. The cache geometry is fixed in the bench config, so the numbers don't depend on the host CPU's caches. Setup (building the book) is not measured.

gungraun needs Linux, Valgrind, and a `gungraun-runner` binary of the same version as the `gungraun` dev-dependency (`cargo install gungraun-runner --version 0.20.0`). `--bench '*'` selects only the bench targets; without it the library's test harness rejects gungraun flags. Save a baseline and compare against it:

```bash
cargo bench --bench '*' -- --save-baseline=master
cargo bench --bench '*' -- --baseline=master
```

On macOS, `scripts/bench.sh` runs the same thing in an Ubuntu container (`docker/bench.Dockerfile`), passing its arguments to `cargo bench`. Its build and baselines live in `target/linux/`. Compare local runs only with other local runs.

```bash
scripts/bench.sh -- --save-baseline=master
scripts/bench.sh --bench mirror -- --baseline=master
```

In GitHub Actions (`.github/workflows/benchmarks.yml`), pull requests bench the base commit and then the head in the same job, and the log shows the per-metric difference. Pushes to `master` just run the benches.

## Commands

```text
kraken-order-book-mirror dashboard PAIR [DEPTH]
kraken-order-book-mirror dump      PAIR [DEPTH]
kraken-order-book-mirror bench     FILE PAIR [DEPTH]
```

`PAIR` is a Kraken pair name such as `BTC/USD`. `DEPTH` is the number of price levels per side to subscribe to and defaults to `1000`. It must be at least 10, because Kraken's checksum covers the top 10 levels.

### `dashboard`

```bash
cargo run --release -- dashboard BTC/USD
cargo run --release -- dashboard BTC/USD 100
```

Opens a live full-screen order-book dashboard: price ladder, book imbalance, depth bands, market impact, L3 order statistics, and feed health. On small terminals it switches to compact cards. The screen repaints at most every 100 ms.

Keys: `q` or Ctrl-C quit, `g` changes ladder grouping, `p` pauses the display, `Tab` moves to the next symbol.

The program exits on a checksum mismatch, a Kraken error, a closed connection, or 5 seconds without any message. It does not reconnect or refresh its WebSocket token (Kraken tokens last about 15 minutes for new connections).

### `dump`

```bash
cargo run --release -- dump BTC/USD > btc_usd.log
```

Prints every raw `level3` frame, starting with the snapshot, to stdout, one JSON frame per line. It does not build a book or open the dashboard. The output is the input format for `bench`.

### `bench`

```bash
cargo run --release -- bench btc_usd.log BTC/USD
```

Replays a captured log through the mirror and reports per-update latency of `OrderBookMirror::on_message`. The replay is offline and needs no API keys, but it calls Kraken's public `AssetPairs` endpoint once to look up the pair's price and quantity precision.

- `DEPTH` must match the depth used to capture the file.
- Use `--release`. A debug build prints a warning, and its numbers are not meaningful.
- The first apply or checksum error prints a partial report and exits with status 1.

#### Results

1,000,000 frames of BTC/USD at depth 1000, on a 2.3 GHz 8-Core Intel Core i9:

```text
% cargo run --release bench tmp/btc_usd_1000_1M.log BTC/USD
file        tmp/btc_usd_1000_1M.log
depth       1000
updates     999999
total       5406166.825 µs
throughput  184973.8 msg/s
min         3.803 µs
mean        5.406 µs
p50         5.096 µs
p90         5.765 µs
p95         6.062 µs
p99         14.211 µs
p99.9       38.873 µs
max         173.496 µs
book        present
```

That is about 185,000 updates per second on one core, with a median of about 5 µs per update, including JSON parsing, applying the events, and computing the checksum.

## How it works

1. **Pair info.** `GET /0/public/AssetPairs` gives the pair's price and lot decimals. These become the price and quantity scales of the local book.
2. **Token.** `POST /0/private/GetWebSocketsToken`, signed with HMAC-SHA512, returns a WebSocket token.
3. **Subscribe.** Connect to `wss://ws-l3.kraken.com/v2` and subscribe to `level3` with the pair, the depth, and `snapshot: true`.
4. **Mirror.** A reader task forwards `level3` text frames over a channel without copying them. An updater task applies each frame to `OrderBookMirror`:
   - Prices and quantities are parsed straight from the JSON text into integer counts of the tick and lot size. No floating point and no `Decimal` on the hot path.
   - Each side is a sorted vector of orders: asks by price ascending, bids by price descending, then by time within each price level.
   - After each update the book is cut back to the subscribed depth, and a CRC32 over the top 10 levels per side is compared with Kraken's checksum.
5. **Display.** When a paint is due, the book is converted to decimals and handed to the dashboard.

See Kraken's [L3 checksum guide](https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2) for the book maintenance and checksum rules.

## Project layout

```text
src/
  main.rs              CLI (clap) and subcommand dispatch
  lib.rs               library crate used by main, tests, and benches
  config.rs            reads KRAKEN_API_KEY / KRAKEN_API_SECRET
  cmds/
    dashboard.rs       live session: reader + updater tasks, dashboard
    dump.rs            live raw frame capture to stdout
    bench.rs           offline replay and latency report
  kraken/
    rest_client.rs     AssetPairs lookup and WebSocket token request
    sign.rs            REST request signing (HMAC-SHA512)
    level3/
      websockets.rs    connect, subscribe, forward level3 frames
      wire.rs          serde types for the JSON frames
      order_book.rs    orders, sorted sides, insert/modify/remove
      mirror.rs        snapshot/update state machine and checksum
  common/
    error.rs           crate error type (thiserror)
    units.rs           decimal text <-> integer unit conversions
    ts.rs              timestamps (ns since epoch)
    json_utils.rs      small JSON scanner
  tui/                 AI Generated ratatui dashboard: metrics, rendering, layout modes
benches/               gungraun (Callgrind) benchmarks
docker/                Linux image for running the benches on macOS
scripts/bench.sh       runs cargo bench in that image
tests/                 integration tests and fixtures (tests/data/)
docs/                  dashboard user guide and screenshot
```

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
