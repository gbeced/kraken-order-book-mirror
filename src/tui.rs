use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

const PANEL_INNER_WIDTH: usize = 84;

pub struct Screen {
    updates: UpdateRate,
    terminal: Terminal,
}

impl Screen {
    pub fn new() -> Screen {
        Screen {
            updates: UpdateRate::default(),
            terminal: Terminal::new(),
        }
    }

    /// Paint when both sides have a best price and the touch and totals are non-zero.
    /// Otherwise return Ok without writing. Frames that skip the paint still count in the rate window.
    pub fn draw(
        &mut self,
        symbol: &str,
        asks: &[(Decimal, Decimal)],
        bids: &[(Decimal, Decimal)],
        timestamp: DateTime<Utc>,
    ) -> Result<(), String> {
        let per_sec = self.updates.record(Instant::now());
        let Some(panel) = panel_from_book(asks, bids) else {
            return Ok(());
        };
        let age = Utc::now() - timestamp;
        let text = render_panel(symbol, &panel, per_sec, age);
        self.terminal.paint(&text)
    }
}

struct DepthBand {
    bid: Decimal,
    ask: Decimal,
}

struct Panel {
    mid: Decimal,
    spread: Decimal,
    spread_bps: Decimal,
    micro: Decimal,
    imbalance: Decimal,
    half_pct: DepthBand,
    one_pct: DepthBand,
}

fn panel_from_book(asks: &[(Decimal, Decimal)], bids: &[(Decimal, Decimal)]) -> Option<Panel> {
    let (best_bid, _) = *bids.first()?;
    let (best_ask, _) = *asks.first()?;
    let mid = (best_bid + best_ask) / Decimal::from(2);
    if mid.is_zero() {
        return None;
    }

    let bid_touch = sum_touch(bids, best_bid);
    let ask_touch = sum_touch(asks, best_ask);
    let touch = bid_touch + ask_touch;
    if touch.is_zero() {
        return None;
    }
    let micro = (best_ask * bid_touch + best_bid * ask_touch) / touch;
    let spread = best_ask - best_bid;
    let spread_bps = spread / mid * Decimal::from(10_000);

    let bid_total: Decimal = bids.iter().map(|(_, qty)| *qty).sum();
    let ask_total: Decimal = asks.iter().map(|(_, qty)| *qty).sum();
    let total = bid_total + ask_total;
    if total.is_zero() {
        return None;
    }
    let imbalance = (bid_total - ask_total) / total;

    // Bids are descending and asks ascending, so each walk stops at the first price outside the band.
    let bid_floor_half = mid * Decimal::new(995, 3);
    let ask_ceil_half = mid * Decimal::new(1005, 3);
    let bid_floor_one = mid * Decimal::new(99, 2);
    let ask_ceil_one = mid * Decimal::new(101, 2);
    let half_pct = DepthBand {
        bid: sum_while(bids, |price| price >= bid_floor_half),
        ask: sum_while(asks, |price| price <= ask_ceil_half),
    };
    let one_pct = DepthBand {
        bid: sum_while(bids, |price| price >= bid_floor_one),
        ask: sum_while(asks, |price| price <= ask_ceil_one),
    };

    Some(Panel {
        mid,
        spread,
        spread_bps,
        micro,
        imbalance,
        half_pct,
        one_pct,
    })
}

fn sum_touch(orders: &[(Decimal, Decimal)], price: Decimal) -> Decimal {
    let mut qty = Decimal::ZERO;
    for (level_price, level_qty) in orders {
        if *level_price != price {
            break;
        }
        qty += *level_qty;
    }
    qty
}

fn sum_while(orders: &[(Decimal, Decimal)], mut keep: impl FnMut(Decimal) -> bool) -> Decimal {
    let mut qty = Decimal::ZERO;
    for (price, level_qty) in orders {
        if !keep(*price) {
            break;
        }
        qty += *level_qty;
    }
    qty
}

fn render_panel(symbol: &str, panel: &Panel, updates_per_sec: u64, age: ChronoDuration) -> String {
    let base = base_asset(symbol);
    let rows = [
        format!(
            "Mid: {}   Spread: {} ({} bps)   Micro: {}   Imbalance: {}",
            format_price(panel.mid),
            format_price(panel.spread),
            format_bps(panel.spread_bps),
            format_price(panel.micro),
            format_imbalance(panel.imbalance),
        ),
        format!(
            "0.5% Depth  Bid: {} {}   Ask: {} {}   Ratio: {}",
            format_qty(panel.half_pct.bid),
            base,
            format_qty(panel.half_pct.ask),
            base,
            format_ratio(panel.half_pct.bid, panel.half_pct.ask),
        ),
        format!(
            "1.0% Depth  Bid: {} {}   Ask: {} {}   Ratio: {}",
            format_qty(panel.one_pct.bid),
            base,
            format_qty(panel.one_pct.ask),
            base,
            format_ratio(panel.one_pct.bid, panel.one_pct.ask),
        ),
        format!(
            "Checksum: OK   Updates: {updates_per_sec}/s   Last: {}",
            format_age(age),
        ),
    ];

    let title = format!("─ {symbol} ");
    let inner = rows
        .iter()
        .map(|row| row.chars().count() + 2)
        .max()
        .unwrap_or(0)
        .max(PANEL_INNER_WIDTH)
        .max(title.chars().count());

    let mut out = String::new();
    out.push('┌');
    out.push_str(&title);
    out.extend(std::iter::repeat_n('─', inner - title.chars().count()));
    out.push('┐');
    out.push('\n');
    for row in &rows {
        out.push('│');
        out.push(' ');
        out.push_str(row);
        let pad = inner - 2 - row.chars().count();
        out.extend(std::iter::repeat_n(' ', pad));
        out.push(' ');
        out.push('│');
        out.push('\n');
    }
    out.push('└');
    out.extend(std::iter::repeat_n('─', inner));
    out.push('┘');
    out.push('\n');
    out
}

fn base_asset(symbol: &str) -> &str {
    symbol
        .split_once('/')
        .map(|(base, _)| base)
        .unwrap_or(symbol)
}

fn format_price(value: Decimal) -> String {
    group_formatted(&format!("{:.2}", value.round_dp(2)))
}

fn format_bps(value: Decimal) -> String {
    group_formatted(&format!("{:.1}", value.round_dp(1)))
}

fn format_imbalance(value: Decimal) -> String {
    format!("{:+.2}", value.round_dp(2))
}

fn format_qty(value: Decimal) -> String {
    let text = format!("{:.4}", value.round_dp(4));
    let (int_part, frac) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{int_part}.0")
    } else {
        format!("{int_part}.{frac}")
    }
}

fn format_ratio(bid: Decimal, ask: Decimal) -> String {
    if ask.is_zero() {
        "—".to_string()
    } else {
        format!("{:.2}", (bid / ask).round_dp(2))
    }
}

fn format_age(age: ChronoDuration) -> String {
    let ms = age.num_milliseconds().max(0);
    if ms < 1_000 {
        format!("{ms}ms ago")
    } else if ms < 60_000 {
        let tenths = ms / 100;
        format!("{}.{}s ago", tenths / 10, tenths % 10)
    } else {
        format!("{}m ago", ms / 60_000)
    }
}

fn group_formatted(text: &str) -> String {
    let (sign, number) = if let Some(rest) = text.strip_prefix('-') {
        ("-", rest)
    } else {
        ("", text)
    };
    let Some((int_part, frac)) = number.split_once('.') else {
        return format!("{sign}{}", group_digits(number));
    };
    format!("{sign}{}.{}", group_digits(int_part), frac)
}

fn group_digits(digits: &str) -> String {
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

#[derive(Default)]
struct UpdateRate {
    stamps: VecDeque<Instant>,
}

impl UpdateRate {
    fn record(&mut self, now: Instant) -> u64 {
        self.stamps.push_back(now);
        while self
            .stamps
            .front()
            .is_some_and(|stamp| now.saturating_duration_since(*stamp) >= Duration::from_secs(1))
        {
            self.stamps.pop_front();
        }
        self.stamps.len() as u64
    }
}

struct Terminal {
    lines: usize,
    tty: bool,
}

impl Terminal {
    fn new() -> Terminal {
        Terminal {
            lines: 0,
            tty: std::io::stdout().is_terminal(),
        }
    }

    fn paint(&mut self, panel: &str) -> Result<(), String> {
        let mut out = std::io::stdout().lock();
        if self.tty && self.lines > 0 {
            write!(out, "\x1b[{}A", self.lines).map_err(|e| e.to_string())?;
        }
        if self.tty {
            for line in panel.lines() {
                writeln!(out, "{line}\x1b[K").map_err(|e| e.to_string())?;
            }
        } else {
            write!(out, "{panel}").map_err(|e| e.to_string())?;
        }
        out.flush().map_err(|e| e.to_string())?;
        self.lines = panel.lines().count();
        Ok(())
    }
}
