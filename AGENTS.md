# rust-order-book-mirror

Tokio binary that authenticates to Kraken, subscribes to the WebSocket v2 `level3` channel, and maintains a local order-by-order book. `mirror::obook_updater` prints one summary line per applied frame (best bid/ask, order-book imbalance, age). The book stays in memory for the process lifetime. The binary does not send orders.

Single binary crate, edition 2024. Requires rustc 1.85+. Not a library and not a monorepo.

## Layout

- `src/main.rs` — declares `bench`, `kraken`, and `mirror`. A `bench` argument runs `bench::run()`. Otherwise `mirror::run()`.
- `src/mirror.rs` — reads `KRAKEN_API_KEY` / `KRAKEN_API_SECRET`, spawns the reader and the updater on an unbounded `mpsc`
- `src/kraken/mod.rs` — `mod json_utils; mod utils; pub mod level3; pub mod websockets;`
- `src/kraken/websockets.rs` — REST token, connect, subscribe, forward `level3` text frames
- `src/kraken/json_utils.rs` — byte scans: `is_channel_level3` and `get_message_type`. Private module. The read loop uses the channel scan. `on_message` uses the type scan.
- `src/kraken/level3.rs` — book types, mirror state machine, apply, checksum
- `src/kraken/utils.rs` — REST HMAC (`get_signature`). Private module
- `tests/data/` — offline fixtures: `level3_snapshot.json`, `level3_update_1.json`, `level3_update_2.json`, `btc_usd_stream.log`
- `Cargo.toml` / `Cargo.lock` — crate `rust-order-book-mirror`

There is no `tests/*.rs` integration crate, no CI, and no `rustfmt.toml` / `clippy.toml` / `rust-toolchain.toml`. Unit tests live in `#[cfg(test)]` modules. `README.md` only documents how to run the live client and the bench.

`json_utils` and `utils` are declared only in `kraken/mod.rs`. Sibling modules call `super::json_utils` and `super::utils`. A compiler hint to add either module in `main.rs` is wrong.

## Setup

```bash
export KRAKEN_API_KEY=...
export KRAKEN_API_SECRET=...
cargo build
```

The binary does not load `.env`. Export the variables yourself, or `set -a; source .env; set +a` when a local `.env` exists.

`.gitignore` lists `.DS_Store`, `.env`, `.vscode`, and `/target`. `mirror.log` (a captured live stream) is untracked. Never commit it, `.env`, or secrets.

## Commands

| Command | Use |
| --- | --- |
| `cargo check` | Typecheck |
| `cargo build` | Debug build |
| `cargo test` | Unit tests in `json_utils`, `utils`, and `level3`. No network |
| `cargo fmt` | rustfmt, default config |
| `cargo clippy` | Lints, no project config |
| `cargo run` | Long-running **live** client |

`cargo run` calls Kraken REST and `wss://ws-l3.kraken.com/v2`. Use `cargo check` and `cargo test` to verify a change. Run the live client only when the task needs a real session.

## Runtime

1. `mirror::run` reads the two env vars and joins two tasks. The reader sends raw `level3` text frames. The updater owns `OrderBookMirror`.
2. Token: POST `https://api.kraken.com/0/private/GetWebSocketsToken`. Body is the hand-built form `nonce={nonce}`. `utils::get_signature` is SHA256 of `nonce` concatenated with that body, then HMAC-SHA512 of `url_path || sha256` using the base64-decoded API secret. Headers are `API-Key` and `API-Sign` (base64 HMAC). `TokenResult.expires` is deserialized and unused.
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

   `OrderBookMirror::new` is called with the same depth (`100`). Those two depths must stay equal.
5. `json_utils::is_channel_level3` scans the top-level JSON object for a plain `channel` string equal to `level3`. When that key is first, the scan returns without walking `data`. Heartbeat, status, and subscribe/unsubscribe acks are dropped. Acks nest `"channel":"level3"` under `result` or `params`, and a nested value does not match.
6. The updater calls `OrderBookMirror::on_message` and, when a book is present, prints the summary with `println!`. The first apply error returns from the updater task. Close or a read error returns from the reader. No reconnect and no token refresh.

Kraken WebSocket tokens last about 15 minutes. `level3` requires a token and uses `wss://ws-l3.kraken.com/v2`. The public L2 `book` channel on `wss://ws.kraken.com/v2` is a different feed.

References:

- [level3 channel](https://docs.kraken.com/api/docs/websocket-v2/level3/)
- [WebSocket auth](https://docs.kraken.com/api/docs/guides/spot-ws-auth)
- [L3 checksum / book maintenance](https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2)

## Local book

`OrderBookMirror` is `WaitingSnapshot`, then `WaitingUpdate`, or `Invalid`. A snapshot is required first. A second snapshot, an update before the snapshot, or any frame after `Invalid` is an error. `Invalid` is terminal: drop the book, and do not copy it to roll an update back. Resubscribe only when asked. `on_snapshot` / `on_update` take `&mut self` plus `&MirrorContext` (the depth sibling). Borrow `state` and `context` separately so a mutable borrow of `state` can sit next to a shared borrow of `context`.

`on_message` peeks `type`, then deserializes the full frame. `data` is a `Vec` of one book per subscribed symbol. Fixtures are full frames (`channel` / `type` / `data`), including compact one-line frames in `btc_usd_stream.log`.

Prices and quantities are `rust_decimal::Decimal` via `serde` arbitrary precision. Timestamps are `chrono::DateTime<Utc>`. Keep `limit_price` and `order_qty` at full decimal precision.

Side order, which the CRC queue depends on:

- Asks: `limit_price` ascending, then `timestamp` ascending.
- Bids: `limit_price` descending, then `timestamp` ascending.
- `insert_order` uses `partition_point` with a strict `<` on timestamp, so a later timestamp appends after earlier orders at that price.
- `delete` and `modify` find the price level with `partition_point`, then scan that level for `order_id`. `modify` removes and reinserts. On the wire, a price change is `delete` then `add`.

After each update, truncate each side to `max_depth` distinct price levels. Every order at the boundary price stays. Kraken omits `delete` for levels that fall out of the top N. Skipping the truncate duplicates `order_id`s when those levels return. Depth `0` drops every order.

An update older than the book (`snapshot.timestamp > update.timestamp`) is an error. Equal timestamps are distinct messages and are applied.

The checksum hashes every order on the top 10 price levels per side (asks, then bids), ignoring levels past the 10th price and ignoring the subscribed depth. Decimals are stringified with the point and leading zeros removed, then CRC32 (`crc32fast`). Subscribed depth must stay at least 10 so the local book can contain the levels Kraken hashes. A checksum or apply failure sets `Invalid`.

`OrderBookSnapshot` is a type alias for `OrderBook<Order>`. An impl on that alias does not apply to `OrderBook<OrderEvent>`.

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

- Changing the subscribed symbol, depth, or snapshot flag. The subscribe `depth` and `OrderBookMirror::new` argument must stay the same value.
- Adding reconnect or token refresh.
- Adding dependencies, CI, a README, or reformatting the whole crate.

**Never**

- Commit secrets, `.env`, or `mirror.log`.
- Put live Kraken calls in unit tests.
- Run the live client as a substitute for compiling.
