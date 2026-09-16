# rust-order-book-mirror

Rust binary that authenticates to Kraken and streams the WebSocket v2 `level3` (order-by-order) book. It currently prints raw JSON frames to stdout. It does not reconstruct, checksum, or persist a local book.

Single binary crate, edition 2024. Requires rustc 1.85+. Not a library and not a monorepo.

## Layout

- `src/main.rs` — reads `KRAKEN_API_KEY` / `KRAKEN_API_SECRET` from the process environment and starts the client
- `src/kraken.rs` — REST token fetch, WebSocket connect, subscribe, read loop
- `Cargo.toml` / `Cargo.lock` — crate `rust-order-book-mirror`

There is no `tests/` directory, no CI, no README, and no `rustfmt.toml` / `clippy.toml` / `rust-toolchain.toml`.

## Setup

```bash
export KRAKEN_API_KEY=...
export KRAKEN_API_SECRET=...
cargo build
```

The binary does not load `.env`. Export the variables yourself, or `set -a; source .env; set +a` when a local `.env` exists.

`.gitignore` currently contains only `/target`. A local `.env` (live secrets) and `mirror.log` (live stream dump) are untracked — never commit them.

## Commands

| Command | Use |
| --- | --- |
| `cargo check` | Typecheck |
| `cargo build` | Debug build |
| `cargo test` | Test suite (none yet) |
| `cargo fmt` | rustfmt, default config |
| `cargo clippy` | Lints, no project config |
| `cargo run` | Long-running **live** client |

`cargo run` calls Kraken REST and `wss://ws-l3.kraken.com/v2`. Do not use it to verify a compile. Prefer `cargo check` / `cargo test`. Only run the live client when the task needs a real session.

## Runtime flow

1. POST `https://api.kraken.com/0/private/GetWebSocketsToken` with Kraken REST HMAC auth:
   - body is `nonce={nonce}`
   - SHA256 of `nonce` concatenated with that body
   - HMAC-SHA512 of `url_path || sha256` using the base64-decoded API secret
   - send headers `API-Key` and `API-Sign` (base64 HMAC)
2. Connect to `wss://ws-l3.kraken.com/v2` (native-tls via tokio-tungstenite).
3. Subscribe (hardcoded today):

   ```json
   {
     "method": "subscribe",
     "params": {
       "channel": "level3",
       "depth": 10,
       "symbol": ["BTC/USD"],
       "snapshot": true,
       "token": "<token>"
     }
   }
   ```

4. Print every text frame. Reply to WebSocket ping with pong. Exit on close or error. No reconnect and no token refresh.

Kraken WebSocket tokens last about 15 minutes. `level3` **requires** a token and uses `wss://ws-l3.kraken.com/v2`. It is not the public L2 `book` channel on `wss://ws.kraken.com/v2`.

References:

- [level3 channel](https://docs.kraken.com/api/docs/websocket-v2/level3/)
- [WebSocket auth](https://docs.kraken.com/api/docs/guides/spot-ws-auth)
- [L3 checksum / book maintenance](https://docs.kraken.com/exchange/guides/websockets/l3-checksum-v2)

## Conventions

- Match surrounding style (4-space indent). `eprintln!` for errors, `println!` for stream data.
- Keep `main.rs` as the env/entry wrapper. Kraken protocol code stays in `src/kraken.rs` or new modules next to it.
- If you start maintaining a local book, do not parse L3 `limit_price` / `order_qty` as `f64`. Checksums need full decimal precision (strings or a decimal type). After each update, truncate to the subscribed depth; Kraken does not send `delete` for levels that fall out of the top N.
- `get_websocket_auth_token` has a TODO to replace the hand-built form body with `serde_qs`. Do that only when touching that path.
- New tests: `#[cfg(test)]` next to the code, or `tests/`. No live network. No real API keys in fixtures.

## Security

- Never log, print, or commit `KRAKEN_API_KEY`, `KRAKEN_API_SECRET`, or WebSocket tokens.
- Do not weaken nonce/HMAC handling.
- Do not add order-entry or trading methods unless explicitly asked.

## Boundaries

**Always**

- Keep diffs scoped to the requested change.
- After code edits, run `cargo check` (and `cargo test` once tests exist).

**Ask first**

- Changing the subscribed symbol, depth, or snapshot flag.
- Adding reconnect, token refresh, or an in-memory order book.
- Adding dependencies, CI, or reformatting the whole crate.

**Never**

- Commit secrets, `.env`, or `mirror.log`.
- Put live Kraken calls in unit tests.
- Run the live client as a substitute for compiling.
