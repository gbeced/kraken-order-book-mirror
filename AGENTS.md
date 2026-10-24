# rust-order-book-mirror

Tokio binary that authenticates to Kraken, subscribes to the WebSocket v2 `level3` channel, and maintains a local order-by-order book. `mirror::obook_updater` prints one summary line per applied frame (best bid/ask, order-book imbalance, age). The book stays in memory for the process lifetime. The binary does not send orders.

One package, edition 2024. Requires rustc 1.85+. The binary is the product. A library target exists so the CodSpeed benches can call the crate. Not a monorepo.

## Layout

- `src/lib.rs` — declares `bench`, `kraken`, and `mirror`. `kraken` stays private to the crate.
- `src/main.rs` — dispatch only. A `bench` argument runs `bench::run()`. Otherwise `mirror::run()`.
- `benches/mirror.rs` — CodSpeed/Divan harness. Eighteen `on_update` cases. Each applies one ask event to a book built from a 100-level snapshot. The snapshot apply is outside the timer. Cases cross `add` / `modify` / `delete`, ask level 0 / 50 / 99, and 1 or 4 orders per level.
- `src/bench.rs` — offline replay of a captured `level3` log through `OrderBookMirror::on_message`. Default file `tmp/raw_messages.log`, default depth `100`. `get_message_type` only labels snapshot vs update timings. A debug build warns. The first apply error prints a partial report and exits 1.
- `src/mirror.rs` — reads `KRAKEN_API_KEY` / `KRAKEN_API_SECRET`, spawns the reader and the updater on an unbounded `mpsc`
- `src/kraken/mod.rs` — `pub(crate) mod json_utils; pub mod level3; mod utils; pub mod websockets;`
- `src/kraken/websockets.rs` — REST token, connect, subscribe, forward `level3` text frames
- `src/kraken/json_utils.rs` — byte scans: `is_channel_level3` and `get_message_type`. `pub(crate)` so `bench` can call the type scan. The read loop uses the channel scan.
- `src/kraken/level3.rs` — book types, mirror state machine, apply, checksum
- `src/kraken/utils.rs` — REST HMAC (`get_signature`). Private module
- `tests/data/` — offline fixtures: `level3_snapshot.json`, `level3_update_1.json`, `level3_update_2.json`, `btc_usd_stream.log`
- `Cargo.toml` / `Cargo.lock` — crate `rust-order-book-mirror`

There is no `tests/*.rs` integration crate and no `rustfmt.toml` / `clippy.toml` / `rust-toolchain.toml`. Unit tests live in `#[cfg(test)]` modules in `json_utils`, `utils`, `level3`, and `bench`. CI is `.github/workflows/codspeed.yml` only: CPU simulation of the Divan benches, triggered on `workflow_dispatch`, push to `master`, and pull requests. `README.md` documents how to run the live client, the local bench, and that CodSpeed workflow. It also notes that the live client passes BTC/USD price scale 1 and quantity scale 8, and that the book stores counts of those increments.

`json_utils` and `utils` are declared only in `kraken/mod.rs`. `utils` is private. `json_utils` is `pub(crate)`. Modules inside `kraken` call `super::json_utils` and `super::utils`. `bench` calls `crate::kraken::json_utils`. A compiler hint to add either module in `main.rs` is wrong.

## Setup

```bash
export KRAKEN_API_KEY=...
export KRAKEN_API_SECRET=...
cargo build
```

The binary does not load `.env`. Export the variables yourself, or `set -a; source .env; set +a` when a local `.env` exists.

Never commit `.env`, or secrets.

## Commands

| Command | Use |
| --- | --- |
| `cargo check` | Typecheck |
| `cargo build` | Debug build |
| `cargo test` | Unit tests in `json_utils`, `utils`, `level3`, and `bench`. No network |
| `cargo fmt` | rustfmt, default config |
| `cargo clippy` | Lints, no project config |
| `cargo run` | Long-running **live** client |
| `cargo run --release -- bench` | Offline replay. Default file `tmp/raw_messages.log`, default depth `100` |
| `cargo bench` | Release Divan benches: one-update mirror cases, and the unit benches |

`cargo run` with no arguments calls Kraken REST and `wss://ws-l3.kraken.com/v2`. `cargo run -- bench` stays offline. Use `cargo check` and `cargo test` to verify a change. Run the live client only when the task needs a real session.

## Runtime

1. `mirror::run` reads the two env vars and joins two tasks. The reader sends raw `level3` text frames. The updater owns `OrderBookMirror`.
2. Token: POST `https://api.kraken.com/0/private/GetWebSocketsToken`. Body is the hand-built form `nonce={nonce}`. `utils::get_signature` is SHA256 of `nonce` concatenated with that body, then HMAC-SHA512 of `url_path || sha256` using the base64-decoded API secret. Headers are `API-Key` and `API-Sign` (base64 HMAC). `TokenResult` stores `token`. The `expires` field is commented out and is not deserialized.
3. Connect to `wss://ws-l3.kraken.com/v2` (native-tls via tokio-tungstenite). Tungstenite answers WebSocket ping itself. The read loop ignores non-text messages.
4. Subscribe. Symbol `BTC/USD` and depth `100` are hardcoded in `mirror.rs`. The reader and the updater both receive that depth. `snapshot: true` is hardcoded in `src/kraken/websockets.rs`:

   ```json
   {
     "method": "subscribe",
     "params": {
       "channel": "level3",
       "depth": 100,
       "symbol": ["BTC/USD"],
       "snapshot": true,
       "token": "<token>"
     }
   }
   ```

   `OrderBookMirror::new` is called with that depth (`100`), price scale `1`, and quantity scale `8`. The subscribe depth and the `new` depth must stay equal. The live client passes those BTC/USD scales; do not hardcode them inside `level3`.
5. `json_utils::is_channel_level3` scans the top-level JSON object for a plain `channel` string equal to `level3`. When that key is first, the scan returns without walking `data`. Heartbeat, status, and subscribe/unsubscribe acks are dropped. Acks nest `"channel":"level3"` under `result` or `params`, and a nested value does not match.
6. The updater calls `OrderBookMirror::on_message` and `expect`s a book before it prints the summary with `println!`. Age is `Utc::now()` minus `OrderBook.timestamp` (the last accepted frame). The first apply error returns from the updater task. A successful message that left no book panics on that `expect`. Close or a read error returns from the reader. No reconnect and no token refresh.

Kraken WebSocket tokens last about 15 minutes. `level3` requires a token and uses `wss://ws-l3.kraken.com/v2`. The public L2 `book` channel on `wss://ws.kraken.com/v2` is a different feed.

References:

- [level3 channel](https://docs.kraken.com/api/docs/websocket-v2/level3/)
- [WebSocket auth](https://docs.kraken.com/api/docs/guides/spot-ws-auth)
- [L3 checksum / book maintenance](https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2)

## Local book

`OrderBookMirror` is `WaitingSnapshot`, then `WaitingUpdate`, or `Invalid`. A snapshot is required first. A second snapshot, an update before the snapshot, or any frame after `Invalid` is an error. `Invalid` is terminal: drop the book, and do not copy it to roll an update back. Resubscribe only when asked. `on_snapshot` / `on_update` take `&mut self` plus `&MirrorContext`. Borrow `state` and `context` separately so a mutable borrow of `state` can sit next to a shared borrow of `context`. `MirrorContext` holds `max_depth`, `price_scale`, and `qty_scale`. `OrderBookUpdater::new` copies `max_depth` and passes both scales through to `OrderBook`. `on_update` does not read `context`.

`on_message` deserializes the frame as `UpdateMsg` first, then as `SnapshotMsg`. `Order` has no `event` field, so an update payload also deserializes as a snapshot; the update attempt has to stay first. `channel` and `type` are not struct fields. A frame that matches neither returns `"Unexpected message"` and leaves the state unchanged. `data` is a `Vec` of one book per subscribed symbol. Fixtures are full frames (`channel` / `type` / `data`), including compact one-line frames in `btc_usd_stream.log`.

`Order.limit_price` and `Order.order_qty` are public `u64` base-unit counts. `Order` does not convert to `Decimal`. `OrderBook::price_to_decimal` and `OrderBook::qty_to_decimal` do, using the `u8` scales stored on that book (`Decimal::from_i128_with_scale`). Wire JSON still parses with `rust_decimal` arbitrary precision into private `WireOrder` / `WireOrderEvent` values. `process_snapshot` and `process_update` convert those to units with `context.price_scale` and `context.qty_scale` before the state machine sees them. A negative value, a value that is not a multiple of the increment, or a count that does not fit in `u64` fails that conversion and leaves the state unchanged. Timestamps stay `chrono::DateTime<Utc>`. Comparisons, `partition_point`, and `find_price_level` use the `u64` fields. Do not put `Decimal` back on `Order`, and do not hardcode the scales in `level3`. The live client and the bench pass `1` and `8` because the feed is BTC/USD (tick `0.1`, step `1e-8`).

Side order, which the CRC queue depends on:

- Asks: `limit_price` ascending, then `timestamp` ascending.
- Bids: `limit_price` descending, then `timestamp` ascending.
- `insert_order` uses `partition_point` with a strict `<` on timestamp, so a later timestamp appends after earlier orders at that price.
- `delete` and `modify` find the price level with `partition_point`, then scan that level for `order_id`. `modify` with the same timestamp writes `order_qty` in place. A later timestamp removes and reinserts inside that price level. An earlier timestamp hits `assert!`. On the wire, a price change is `delete` then `add`.

After each side's events, truncate that side to `max_depth` distinct price levels (asks, then bids). Every order at the boundary price stays. Kraken omits `delete` for levels that fall out of the top N. Skipping the truncate duplicates `order_id`s when those levels return. Depth `0` drops every order.

`OrderBook.timestamp` is copied from the snapshot, then replaced with `update.timestamp` after a checksum match. There is no stale-update check. A checksum or apply failure returns before that assignment.

The checksum hashes every order on the top 10 price levels per side (asks, then bids), ignoring levels past the 10th price and ignoring the subscribed depth. Each stored count is written as its decimal digits, a zero count writes nothing, then CRC32 (`crc32fast`). On this feed the wire scale matches the tick and the step, so those digits are the Kraken checksum string (decimal point and leading zeros removed, trailing zeros kept). Subscribed depth must stay at least 10 so the local book can contain the levels Kraken hashes. Building the book copies asks, bids, and timestamp and drops `Payload.checksum`. Updates compare a fresh CRC against `update.checksum`. A checksum or apply failure sets `Invalid`.

`Snapshot` is a type alias for `Payload<Order>`. An impl on that alias does not apply to `Payload<OrderEvent>` (`Update`). JSON frames deserialize as `Envelope<Payload<WireOrder>>` and `Envelope<Payload<WireOrderEvent>>`, then convert into those unit payloads. `Envelope<T>` is the `{ data: [...] }` frame. The maintained book is `OrderBook` (asks, bids, timestamp, price scale, quantity scale).

## Conventions

- Match surrounding style (4-space indent). `eprintln!` for errors, `println!` for the summary line.
- Keep `main.rs` as the dispatch entry. The live session stays in `mirror.rs`. Kraken protocol code stays under `src/kraken/`.
- New tests: `#[cfg(test)]` next to the code. Fixtures go in `tests/data/` and are read via `CARGO_MANIFEST_DIR`. No live network. No real API keys in fixtures.
- `get_websocket_auth_token` builds the form body by hand. Leave that as it is unless the task is the auth request.

## Security

- Never log, print, or commit `KRAKEN_API_KEY`, `KRAKEN_API_SECRET`, or WebSocket tokens.
- Do not weaken nonce/HMAC handling.
- Do not add order-entry or trading methods unless explicitly asked.

## Boundaries

**Always**

- Keep diffs scoped to the requested change.
- After code edits, run `cargo check` and `cargo test`.

**Ask first**

- Changing the subscribed symbol, depth, snapshot flag, or the BTC/USD price and quantity scales. The subscribe `depth` and the depth passed to `OrderBookMirror::new` must stay the same value.
- Adding reconnect or token refresh.
- Adding dependencies, CI, a README, or reformatting the whole crate.

**Never**

- Commit secrets, `.env`, `tmp/*`, or `benchmark.md`.
- Put live Kraken calls in unit tests.
- Run the live client as a substitute for compiling.
