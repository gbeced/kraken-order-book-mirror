# rust-order-book-mirror

Tokio binary that authenticates to Kraken, subscribes to the WebSocket v2 `level3` channel for a pair given on the command line, and maintains a local order-by-order book. `cmds::dashboard` applies each frame. `tui::Screen` paints an order-book dashboard (ladder, imbalance, depth, impact, L3 order stats, and feed health), with compact cards on small terminals. The book stays in memory for the process lifetime. The binary does not send orders.

One package, edition 2024. `Cargo.toml` sets no `rust-version`. Edition 2024 needs rustc 1.85+, but `units.rs` uses `std::fmt::NumBuffer` / `format_into`, which needs a much newer stable toolchain. Verified with rustc 1.99.1. The binary is the product. A library target exists so the benches, the integration tests, and `main.rs` can call the crate. Not a monorepo.

## Layout

- `src/lib.rs` — `pub mod cmds; mod common; mod config; pub mod kraken; mod tui;`. `common`, `config`, and `tui` stay crate-private.
- `src/main.rs` — clap CLI (`derive`) and dispatch only. Subcommands: `dashboard PAIR [DEPTH]`, `dump PAIR [DEPTH]`, and `bench FILE PAIR [DEPTH]`. `DEPTH` is a positional argument and defaults to `1000`. There is no `--depth` flag.
- `src/cmds/mod.rs` — `pub mod bench; pub mod dashboard; pub mod dump;`
- `src/cmds/dashboard.rs` — live session. `run` reads credentials, fetches pair info, spawns the reader and the updater on an unbounded `mpsc`. The updater owns `OrderBookMirror` and `tui::Screen`.
- `src/cmds/bench.rs` — replay of a captured `level3` log through `OrderBookMirror::on_message`. The file and the pair are required. The pair is only used for the REST precision lookup. A debug build warns. The first apply error prints a partial report and exits 1. `replay` is the sum-of-nanos helper for callers.
- `src/cmds/dump.rs` — live raw capture. `run` reads credentials, then prints each `level3` text frame to stdout, one per line, through the same `connect_and_dispatch_level3`. It does not fetch pair info, build a book, or open the TUI. Redirect stdout to produce a file for `bench`. `run` awaits the reader, then lets the printer drain the channel.
- `src/config.rs` — `get_credentials` reads `KRAKEN_API_KEY` / `KRAKEN_API_SECRET`. `dashboard` and `dump` call it.
- `src/common/mod.rs` declares `json_utils` and `ts`. `src/common/ts.rs` — `Ts(i64)`, nanoseconds since the Unix epoch (UTC). It converts from `DateTime<Utc>` (fallible outside about 1677–2262) and from an RFC 3339 `&str`, and back into `DateTime<Utc>`. `src/common/json_utils.rs` — `JsonScanner`, a byte scanner. `top_level_plain_string(key)` returns the value of a top-level plain-string key without walking past it.
- `src/tui/` — crate-private. `Screen` is the only public item. It receives decimal price and quantity pairs and does not import `level3`. `mod.rs` holds `Screen`, the input thread, the paint throttle, and per-symbol `BookCard` state (ladders, flashes, lag window, OFI window). `metrics.rs` is pure analytics (`Analysis`, `ladder`, `slippage_bps`, `ofi_step`, `percentile`). `render.rs` draws the dashboard for the focused symbol: header, ladder, imbalance, depth bands, impact, L3 stats. It has three modes by terminal size: full (≥120x32), medium (≥80x20), and `compact.rs` below that, which stacks one six-row card per symbol (best bid and ask, mid, micro offset, OFI, imbalance, depth) from the same `Analysis`. `format.rs` and `style.rs` hold helpers. Keys: `q`/Ctrl-C quit, `g` ladder grouping, `p` pause display (`note_update` reports no paint due while paused), `Tab` next symbol. `Screen` supports several symbols, but `dashboard` subscribes to one pair, so `Tab` and stacked compact cards only matter if that changes. History is not kept: the only time-based series are the 10s lag window and the 60s OFI window, both fed in `BookCard::ingest`, which runs only when a paint happens. Unit tests live in `metrics`, `format`, `mod`, and `render` (the latter via ratatui `TestBackend`, which renders to a string so tests can assert on panel titles and text).
- `src/kraken/mod.rs` — `pub mod level3; pub mod rest_client; mod sign;`
- `src/kraken/rest_client.rs` — `get_websocket_auth_token` (private, signed) and `get_pair_info` (public `AssetPairs`).
- `src/kraken/sign.rs` — REST HMAC (`get_signature`). Private module.
- `src/kraken/level3/mod.rs` — `pub mod mirror; pub mod order_book; pub mod units; pub mod websockets; mod wire;`
- `src/kraken/level3/websockets.rs` — connect, subscribe, forward `level3` text frames. Holds the private `is_channel_level3` and `extract_error`, both built on `JsonScanner`.
- `src/kraken/level3/wire.rs` — private serde types: `Envelope`, `Payload<T>`, `WireOrder`, `WireOrderEvent`, `EventType`.
- `src/kraken/level3/units.rs` — `to_units`, `to_decimal`, `serialize_units`.
- `src/kraken/level3/order_book.rs` — `Order`, `Orders` (sorted vec), `OrderBook`.
- `src/kraken/level3/mirror.rs` — `OrderBookMirror` state machine, apply, checksum.
- `benches/mirror.rs` — CodSpeed/Divan harness. Eighteen `on_update` cases. Each applies one ask event to a book built from a 100-level snapshot. The snapshot apply is outside the timer. Cases cross `add` / `modify` / `delete`, ask level 0 / 50 / 99, and 1 or 4 orders per level.
- `benches/to_units.rs`, `benches/serialize_units.rs` — Divan micro-benches for `units::to_units` and `units::serialize_units`. All three are `[[bench]]` entries with `harness = false`.
- `tests/mirror.rs` — integration tests over the fixtures, using depth `10` and scales `1` / `8`.
- `tests/data/` — offline fixtures: `level3_snapshot.json`, `level3_update_1.json`, `level3_update_2.json`, `btc_usd_stream.log`
- `docs/ui-guide.md` — user guide for the dashboard: every panel, how each number is computed, layout modes, keys. Keep it in sync with `src/tui/` changes (panels, thresholds, keys, colors).
- `env.example` — empty `KRAKEN_API_KEY` / `KRAKEN_API_SECRET` template.
- `Cargo.toml` / `Cargo.lock` — crate `rust-order-book-mirror`

There is no `rustfmt.toml` / `clippy.toml` / `rust-toolchain.toml`. Unit tests live in `#[cfg(test)]` modules in `cmds::bench`, `kraken::sign`, `kraken::level3::{units, order_book, mirror, websockets}`, and `tui::{metrics, format, mod, render}`. CI is `.github/workflows/codspeed.yml` only: `cargo codspeed build` then CPU simulation of the Divan benches, triggered on `workflow_dispatch`, push to `master`, and pull requests. `README.md` documents the live client and the bench command, but it is stale (see Known drift).

`sign` is private to `kraken`; `rest_client` calls it as `super::sign`. `wire` is private to `level3`. `is_channel_level3` and `extract_error` are private to `level3/websockets.rs`. `common` and `config` are declared only in `lib.rs`. A compiler hint to add a module in `main.rs` is wrong, since `main.rs` uses the library crate.

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
| `cargo test` | Unit tests plus `tests/mirror.rs`. No network |
| `rustfmt --edition 2024 FILE...` | Format only the files you touched. `cargo fmt` would also reformat `src/cmds/dump.rs` and `src/kraken/level3/websockets.rs`, which are not rustfmt-clean |
| `cargo clippy --all-targets` | Lints, no project config. The baseline already has warnings (mostly `mirror.rs`, `units.rs`, `tests/mirror.rs`). Add no new ones; don't fix existing ones unless asked |
| `cargo run -- dashboard BTC/USD [DEPTH]` | Long-running **live** client. Default depth `1000` |
| `cargo run -- dump BTC/USD [DEPTH] > FILE` | Long-running **live** raw frame capture. Default depth `1000` |
| `cargo run --release -- bench FILE PAIR [DEPTH]` | Replay of `FILE`. Default depth `1000`. **Needs network** for the pair lookup |
| `cargo bench` | Release Divan benches: `mirror`, `to_units`, `serialize_units` |

`dashboard` and `dump` need API keys. `dashboard` calls Kraken REST (`AssetPairs`, then `GetWebSocketsToken`) and `wss://ws-l3.kraken.com/v2`. `dump` skips `AssetPairs`. `bench` needs no API keys, but it still calls the public `AssetPairs` endpoint. The replay itself is offline. `DEPTH` must match the depth that produced the replayed file. Use `cargo check` and `cargo test` to verify a change. Run the live client only when the task needs a real session.

## Runtime

1. `cmds::dashboard::run` reads the two env vars, calls `rest_client::get_pair_info(pair)`, then runs two tasks. The reader sends raw `level3` text frames. The updater owns `OrderBookMirror` and the `Screen`. Whichever task returns first, `run` aborts and awaits the other, then returns the finished task's result.
2. Pair info: GET `https://api.kraken.com/0/public/AssetPairs?pair=PAIR`. It takes the first entry in the result map. `pair_decimals` becomes the price scale and `lot_decimals` the quantity scale. `PairInfo` calls them `quote_precision` and `base_precision`. A Kraken error array or an empty result is an error. The scales are not hardcoded.
3. Token: POST `https://api.kraken.com/0/private/GetWebSocketsToken`. Body is the hand-built form `nonce={nonce}`. `sign::get_signature` is SHA256 of `nonce` concatenated with that body, then HMAC-SHA512 of `url_path || sha256` using the base64-decoded API secret. Headers are `API-Key` and `API-Sign` (base64 HMAC). `TokenResult` stores `token`. The `expires` field is commented out and is not deserialized.
4. Connect to `wss://ws-l3.kraken.com/v2` (native-tls via tokio-tungstenite). Tungstenite answers WebSocket ping itself. The read loop ignores non-text messages.
5. Subscribe. Symbol and depth come from the CLI and pass through `connect_and_dispatch_level3`. `snapshot: true` is hardcoded in `src/kraken/level3/websockets.rs`:

   ```json
   {
     "method": "subscribe",
     "params": {
       "channel": "level3",
       "depth": 1000,
       "symbol": ["BTC/USD"],
       "snapshot": true,
       "token": "<token>"
     }
   }
   ```

   `OrderBookMirror::new` is called with the same depth plus the two scales from step 2. The subscribe depth and the `new` depth must stay equal. Do not hardcode scales inside `level3`.
6. `is_channel_level3` scans the top-level JSON object for a plain `channel` string equal to `level3`. When that key is first, the scan returns without walking `data`. Heartbeat, status, and subscribe/unsubscribe acks are dropped. Acks nest `"channel":"level3"` under `result` or `params`, and a nested value does not match. A frame that is not `level3` but has a top-level plain `error` string (`extract_error`, e.g. `Subscription depth not supported`) makes the read loop return that message as an error.
7. The updater `select!`s (biased) on UI events and on the frame receiver. `Quit` (`q` or Ctrl-C) breaks the loop. `Resize` calls `Screen::redraw`. `Key` calls `Screen::on_key`. For each frame it calls `OrderBookMirror::on_message`. On success it calls `Screen::note_update` with the pair, which records that book's one-second update rate and reports whether a paint is due (immediately on the first attempt, then at most every 100ms). When a paint is not due, the updater skips decimal conversion and `draw`. When it is due, the updater `expect`s the book, converts each order with `OrderBook::price_to_decimal` and `OrderBook::qty_to_decimal`, and passes those pairs to `Screen::draw`. `draw` paints a ratatui fullscreen alternate screen: the dashboard for the focused symbol, or stacked compact cards on a small terminal. Age is `Ts::now()` minus the frame timestamp, computed in `tui` at paint time. A book with no best price, a zero mid, a zero touch, or zero totals skips the paint and does not move the throttle stamp. That frame still counts in the rate window. `Screen`'s `Drop` stops the input thread and restores the terminal. The first apply error returns from the updater task, which also aborts the reader. A successful message that left no book panics on that `expect`. A WebSocket Close frame, a read error, or a Kraken `error` frame returns `Err` from the reader. If the stream ends without a Close frame, the reader returns `Ok(())` and the program exits silently. A closed frame channel ends the updater. `run` does not print task errors itself: the finished task's `String` error is returned through `main`'s `Result`, so it prints after the updater is dropped and the terminal is restored. A panic or join error also propagates. No reconnect and no token refresh.

Kraken WebSocket tokens last about 15 minutes. `level3` requires a token and uses `wss://ws-l3.kraken.com/v2`. The public L2 `book` channel on `wss://ws.kraken.com/v2` is a different feed.

References:

- [level3 channel](https://docs.kraken.com/api/docs/websocket-v2/level3/)
- [WebSocket auth](https://docs.kraken.com/api/docs/guides/spot-ws-auth)
- [L3 checksum / book maintenance](https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2)

## Local book

`OrderBookMirror` is `WaitingSnapshot`, then `WaitingUpdate`, or `Invalid`. A snapshot is required first. A second snapshot or an update before the snapshot is an error and sets `Invalid`. Any frame after `Invalid` is an error. One mirror holds one book: a frame whose `data` has two entries applies them in turn, so a second snapshot entry fails as a second snapshot. `Invalid` is terminal: drop the book, and do not copy it to roll an update back. Resubscribe only when asked. `on_snapshot` / `on_update` take `&mut self` plus `&MirrorContext`. Borrow `state` and `context` separately so a mutable borrow of `state` can sit next to a shared borrow of `context`. `MirrorContext` holds `max_depth`, `price_scale`, and `qty_scale`. `OrderBookUpdater::new` copies `max_depth` and passes both scales through to `OrderBook`. `on_update` does not read `context`.

`on_message` deserializes the frame as `UpdateMsg` first, then as `SnapshotMsg`. `Order` has no `event` field, so an update payload also deserializes as a snapshot; the update attempt has to stay first. `channel` and `type` are not struct fields. A frame that matches neither returns `"Unexpected message"` and leaves the state unchanged. `data` is a `Vec` of one book per subscribed symbol. Fixtures are full frames (`channel` / `type` / `data`), including compact one-line frames in `btc_usd_stream.log`.

`Order.limit_price` and `Order.order_qty` are public `u64` base-unit counts. `Order` does not convert to `Decimal`. `OrderBook::price_to_decimal` and `OrderBook::qty_to_decimal` do, using the `u8` scales stored on that book (`units::to_decimal`). Wire JSON still parses with `rust_decimal` arbitrary precision into private `wire::WireOrder` / `wire::WireOrderEvent` values. `process_snapshot` and `process_update` convert those to units with `context.price_scale` and `context.qty_scale` (`Snapshot::try_from_wire`, `Update::try_from_wire`) before the state machine sees them. A negative value, a value that is not a multiple of the increment, or a count that does not fit in `u64` fails that conversion and leaves the state unchanged. Timestamps are `common::ts::Ts` (i64 nanoseconds; Kraken sends 9 fractional digits, so microseconds would tie distinct events). `wire` deserializes them as `DateTime<Utc>` and converts with `deserialize_ts`; an out-of-range timestamp fails deserialization. `cmds::dashboard` passes `OrderBook.timestamp` to `Screen::draw` as a `Ts`, and `tui` computes ages from `Ts::now()` without chrono. Comparisons, `partition_point`, and `find_price_level` use the `u64` fields. Do not put `Decimal` back on `Order`, and do not hardcode the scales in `level3`. The scales come from the pair lookup. The fixtures, the benches, and `tests/mirror.rs` use `1` and `8` because they are BTC/USD (tick `0.1`, step `1e-8`).

Side order, which the CRC queue depends on:

- Asks: `limit_price` ascending, then `timestamp` ascending.
- Bids: `limit_price` descending, then `timestamp` ascending.
- `insert_order` uses `partition_point` with a strict `<` on timestamp, so a later timestamp appends after earlier orders at that price.
- `remove_order` and `modify_order` go through `find_order`, which finds the price level with `partition_point`, then scans that level for `order_id`. `modify_order` with the same timestamp writes `order_qty` in place. A later timestamp removes and reinserts inside that price level. An earlier timestamp hits `assert!`. On the wire, a price change is `delete` then `add`.

After each side's events, `truncate_price_level` cuts that side to `max_depth` distinct price levels (asks, then bids). Every order at the boundary price stays. Kraken omits `delete` for levels that fall out of the top N. Skipping the truncate duplicates `order_id`s when those levels return. Depth `0` drops every order.

`OrderBook.timestamp` is copied from the snapshot, then replaced with `update.timestamp` after a checksum match. There is no stale-update check. A checksum or apply failure returns before that assignment.

The checksum hashes every order on the top 10 price levels per side (asks, then bids), ignoring levels past the 10th price and ignoring the subscribed depth. Each stored count is written as its decimal digits (`units::serialize_units`), a zero count writes nothing, then CRC32 (`crc32fast`). When the wire scale matches the tick and the step, those digits are the Kraken checksum string (decimal point and leading zeros removed, trailing zeros kept). Subscribed depth must stay at least 10 so the local book can contain the levels Kraken hashes. Building the book copies asks, bids, and timestamp and drops `Payload.checksum`: the snapshot checksum is not verified, and the snapshot is not truncated to `max_depth` (only updates truncate). Updates compare a fresh CRC against `update.checksum`. A checksum or apply failure sets `Invalid`.

`Snapshot` is a type alias for `Payload<Order>`. An impl on that alias does not apply to `Payload<OrderEvent>` (`Update`). JSON frames deserialize as `Envelope<Payload<WireOrder>>` and `Envelope<Payload<WireOrderEvent>>`, then convert into those unit payloads. `Envelope<T>` is the `{ data: [...] }` frame. The maintained book is `OrderBook` (asks, bids, timestamp, price scale, quantity scale).

## Conventions

- Match surrounding style (4-space indent). Reader and updater errors return as `Err` and surface through `main`. The panel goes through `tui`.
- Keep `main.rs` as the CLI and dispatch entry. Each subcommand lives in `src/cmds/`. Panel rendering, formatting, and redraw stay in `src/tui/`. Kraken protocol and REST code stays under `src/kraken/`. Credentials stay in `src/config.rs`.
- New tests: `#[cfg(test)]` next to the code, or `tests/*.rs` for whole-mirror behavior. Fixtures go in `tests/data/` and are read via `CARGO_MANIFEST_DIR`. No live network. No real API keys in fixtures.
- `get_websocket_auth_token` builds the form body by hand. Leave that as it is unless the task is the auth request.
- Commented-out code in `rest_client.rs`, `websockets.rs`, `wire.rs`, and `mirror.rs` is the author's scratch. Leave it alone unless asked.
- When removing a UI feature, also remove the state, metrics, and tests that only it used, and update `docs/ui-guide.md` and the `src/tui/` bullet above.

## Security

- Never log, print, or commit `KRAKEN_API_KEY`, `KRAKEN_API_SECRET`, or WebSocket tokens.
- Do not weaken nonce/HMAC handling.
- Do not add order-entry or trading methods unless explicitly asked.

## Known drift

- `README.md` still describes the old interface: bare `cargo run`, `bench FILE [--depth N]`, a default depth of `100`, a hardcoded BTC/USD subscription, and hardcoded scales. The real CLI is above. Update it only when asked.
- `docs/ui-guide.md` still calls the subcommand `mirror` (`cargo run --release -- mirror BTC/USD`); it is `dashboard`. It also omits `Tab`.
- The `connect_and_dispatch_level3` parameter named `channel` (and `channel` in `cmds::dashboard`) holds the pair, e.g. `BTC/USD`.
- `get_pair_info` takes the first entry of the `AssetPairs` result and uses `lot_decimals` as the level3 quantity scale. If a pair's feed has finer quantities, `to_units` fails with `value is not a multiple of the increment`.
- `tests/mirror.rs::mirror_checksum_mismatch_after_update` replays the whole stream and asserts the final timestamp. It does not test a mismatch.

## Boundaries

**Always**

- Keep diffs scoped to the requested change.
- After code edits, run `cargo check` and `cargo test`.

**Ask first**

- Changing the `snapshot` flag, the default depth, or how scales are derived. The subscribe `depth` and the depth passed to `OrderBookMirror::new` must stay the same value.
- Adding reconnect or token refresh.
- Adding dependencies, CI, or reformatting the whole crate.

**Never**

- Commit secrets, `.env` or `tmp/*`.
- Put live Kraken calls in unit tests.
- Run the live client as a substitute for compiling.
