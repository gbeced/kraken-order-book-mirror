# Order Book Dashboard: User Guide

This guide explains the terminal UI of `mirror`: what each panel shows, how every number is computed, and how to read it.

```bash
export KRAKEN_API_KEY=...
export KRAKEN_API_SECRET=...
cargo run --release -- mirror BTC/USD [DEPTH]
```

`DEPTH` is the number of price levels per side you subscribe to (default `1000`). Everything on screen is computed from that locally mirrored book. Nothing outside the subscribed depth is visible to the UI.

## How it works

1. A reader task receives Kraken `level3` frames (full order-by-order book: one entry per resting order).
2. An updater task applies each frame to the local book and verifies Kraken's CRC32 checksum. If anything fails the program exits with the error instead of showing a wrong book.
3. The screen is repainted **at most every 100 ms** (10 Hz), however fast frames arrive. Frames that are skipped still count towards `upd/s`.
4. On each paint the UI recomputes all metrics from the full book. It also records lag samples and order flow imbalance.

Because Kraken sends individual orders, the UI can show things an L2 feed can't: how many orders sit at a price, and how big the largest single order is.

Nothing is drawn until the first valid snapshot arrives, or while the book has no bid, no ask, or zero size at the touch.

## Layout modes

The layout depends on the terminal size.

| Terminal size | View |
| --- | --- |
| 120 columns × 32 rows or larger | **Full**: all panels |
| 80 × 20 or larger | **Medium**: header, ladder, imbalance, depth, and impact if there is room. No L3 panel |
| Smaller in either dimension | **Compact**: a six-row card per symbol, stacked |

Resizing the window repaints immediately.

## Keys

| Key | Action |
| --- | --- |
| `q`, Ctrl-C | Quit |
| `g` | Cycle ladder price grouping: 1×, 5×, 10×, 50× tick. No visible effect in compact view, which has no ladder |
| `p` | Pause / resume the display. The book keeps updating underneath; on resume you see the current state |

## Screen anatomy

```
┌ BTC/USD ─────────────────────────────────────────────────────────┐
│ ● LIVE  61 upd/s  lag p50 81ms · p99 246ms · age 152ms  orders … │   Header
│ MID …  SPREAD …  MICRO …  OFI 1s … 10s … 60s …                   │
├──────────────────────┬──────────────────┬────────────────────────┤
│ LADDER               │ IMBALANCE        │ IMPACT                 │
│                      ├──────────────────┤                        │
│                      │ DEPTH            ├────────────────────────┤
│                      │                  │ L3 ORDERS              │
└──────────────────────┴──────────────────┴────────────────────────┘
 q quit  g group 1x  p pause
```

### Header, line 1: feed health

| Field | Meaning |
| --- | --- |
| **Status** | `● LIVE` (green): data is fresh. `● STALE` (red): last frame is more than 2 s old. `✖ CROSSED`: best bid is above best ask, which should never happen and signals bad data. `LOCKED` is appended when best bid equals best ask. `❚❚ PAUSED`: display frozen |
| **upd/s** | Frames applied to this book in the last second |
| **lag p50 / p99** | Median and 99th percentile of (local clock − exchange timestamp of the frame), over the last 10 s of paints. It includes any clock skew between your machine and Kraken, so treat it as a trend rather than an exact latency |
| **age** | Local clock minus the timestamp of the latest frame. Green under 100 ms, yellow up to 300 ms, red above |
| **orders bid / ask** | Number of individual resting orders in the local book on each side |

### Header, line 2: price summary

| Field | Meaning |
| --- | --- |
| **MID** | `(best bid + best ask) / 2` |
| **SPREAD** | `best ask − best bid`, then in ticks (the minimum price increment), then in basis points of mid |
| **MICRO** | Microprice: `(ask × bid_size_at_touch + bid × ask_size_at_touch) / (bid_size_at_touch + ask_size_at_touch)`. It weights each side's price by the *opposite* side's size at the touch, so it leans toward the side that is likely to move next. The value in parentheses is `micro − mid` in ticks: **positive (green) means more size on the bid, so pressure is up; negative (red) means pressure is down** |
| **OFI 1s / 10s / 60s** | Order Flow Imbalance (Cont, Kukanov, Stoikov): the running sum of changes in size at the best bid and best ask. Added bid size, a higher bid, removed ask size, or a lifted ask all push it positive; the opposite pushes it negative. Units are base-asset quantity. Positive means buyers are net more aggressive over that window |

OFI is sampled at the paint rate (up to 10 times a second), not on every frame, so it is an approximation of the exact per-event figure.

### Ladder

The book itself, centered on the mid price: **asks above** (best ask nearest the center), **bids below** (best bid nearest the center). The row in the middle shows mid and spread.

| Column | Meaning |
| --- | --- |
| `#ord` | Number of individual orders at that price (queue depth) |
| `size` | Total quantity at that price |
| `cum` | Cumulative quantity from the touch out to that price |
| `price` | Price (red for asks, green for bids) |
| `depth` | Bar proportional to `cum` |

- **Flash:** when the size at a price changes, the size cell gets a green background (grew) or red background (shrank) for about 400 ms. This makes cancellations and new liquidity visible at a glance.
- **Grouping (`g`):** buckets prices by 1, 5, 10 or 50 ticks. Asks round up and bids round down, so a bucket never improves on the real price. Order counts and sizes are summed per bucket.
- **Rows shown:** as many as fit the terminal height.
- **Bar scale:** both sides share one scale, based on the largest `cum` among the visible rows. If one side is much thicker than the other within the visible range, the thinner side's bars become very short. The numbers are still correct. Use the Depth panel to see further out.

### Imbalance

One row per horizon, each a bid/ask split bar (green = bid share, red = ask share) and a signed value:

```
imbalance = (bid − ask) / (bid + ask)      range −1 … +1
```

| Row | Horizon |
| --- | --- |
| `L1` | Quantity at the best price on each side |
| `L5`, `L10` | First 5 / 10 **price levels** on each side (all orders at those prices) |
| `0.5%`, `1.0%` | All quantity within ±0.5% / ±1.0% of mid |

Positive means more bids, so buying pressure; negative means more asks. Look at the horizons together: a book can be heavy on the bid at L1 but lopsided the other way at 1%. Short horizons react quickly, but they can be spoofed or may flicker. Long horizons are more stable.

### Depth

Quantity resting within a distance of mid, in basis points (1 bp = 0.01%):

| Column | Meaning |
| --- | --- |
| `±bps` | Distance from mid: 5, 10, 25, 50, 100 bps |
| `bid`, `ask` | Base-asset quantity on each side within that distance |
| `ratio` | `bid / ask`. Green above 1, red below 1 |

If the subscribed `DEPTH` doesn't reach as far as a band, that band is truncated and its values are understated.

### Impact

Estimated cost of a **market order** of a given size, walking the visible book:

- Rows are 10k, 100k and 1M of quote currency (USD for BTC/USD).
- `buy` walks the asks upward, `sell` walks the bids downward.
- Value = volume-weighted average fill price versus mid, in bps. 0.0 means the order fits within the touch at about mid.
- Colors: bold white under 3 bps, yellow from 3 bps, red from 10 bps.
- `—` means the visible book is not deep enough to fill the order.

This ignores fees and any liquidity that appears or disappears while you trade.

### L3 orders

| Line | Meaning |
| --- | --- |
| `avg order` | Average order size per side (total quantity ÷ number of orders) |
| `max bid` / `max ask` | Largest single order on that side: quantity, price, and distance from mid in bps |
| `◆ N×` | Shown when that order is at least 10× the side's average. It is a likely wall or large resting participant |

A large distance from mid usually means a passive wall far from the touch, not an imminent move.

## Compact view

Below either size threshold, each symbol gets a six-row card, stacked from the top. It shows the most important figures from the dashboard, computed the same way, so the numbers don't change when you resize.

```
┌ BTC/USD ● LIVE ────────────────────────────── 61/s  age 152ms ┐
│ BID 77,430.3 × 0.0786   ASK 77,430.4 × 2.3139   1 tick 0.013bp│
│ MID 77,430.35   MICRO -0.47t   OFI 1s +0.00   10s -2.23       │
│ IMB L1 -0.93   L10 -0.97   1.0% +0.07                         │
│ DEPTH 10bp 75.553/91.608 0.82   100bp 262.09/225.52 1.16      │
└───────────────────────────────────────────────────────────────┘
 q quit  p pause
```

| Where | Meaning |
| --- | --- |
| **Border** | Symbol and status (same states as the dashboard header), then `upd/s` and age on the right |
| **Row 1** | Best bid and best ask, each `price × total size at that price`, then the spread in ticks and bps |
| **Row 2** | Mid, microprice offset from mid in ticks (`t`), OFI over 1 s and 10 s |
| **Row 3** | Imbalance at L1, L10 and ±1.0% |
| **Row 4** | Bid/ask quantity and bid/ask ratio within ±10 and ±100 bps of mid |

The ladder, impact and L3 orders appear only in the larger views.

On a narrow terminal, a field that doesn't fit is dropped whole, because a clipped number could read as a different number. Fields go from the right of each row. If both bid and ask can't fit with their sizes, the sizes go and both prices stay. On a short terminal the card loses rows from the bottom.

## Limitations

- Only the subscribed depth is visible. There is no reconnect or token refresh, so the session ends after an error or when the token expires.
- Lag includes clock skew, and OFI is sampled at 10 Hz.
- `pair_decimals` and `lot_decimals` from Kraken's REST `AssetPairs` endpoint define the tick and the size step.
- The dashboard shows one symbol at a time.
