//! Dashboard rendering: header, ladder, imbalance, depth, impact, L3 stats.

use super::compact;
use super::format::{
    age_ms, base_asset, format_age, format_bps, format_fixed, format_qty_auto, format_signed,
    format_spread_bps, pad_left, pad_right, quote_asset, text_width,
};
use super::metrics::{self, Analysis, BigOrder, Level};
use super::style::{
    ask_bar, bid_bar, bright, cyan, flash_down, flash_up, gray, green, red, yellow,
};
use super::{BookCard, FLASH_FOR};
use crate::common::ts::Ts;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};

const FULL_WIDTH: u16 = 120;
const FULL_HEIGHT: u16 = 32;
const MEDIUM_WIDTH: u16 = 80;
const MEDIUM_HEIGHT: u16 = 20;
const STALE_MS: i64 = 2_000;

pub(super) struct Ctx {
    pub now_utc: Ts,
    pub now: Instant,
    pub group: u32,
    pub paused: bool,
    pub focus: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Full,
    Medium,
    Compact,
}

fn mode_for(area: Rect) -> Mode {
    if area.width >= FULL_WIDTH && area.height >= FULL_HEIGHT {
        Mode::Full
    } else if area.width >= MEDIUM_WIDTH && area.height >= MEDIUM_HEIGHT {
        Mode::Medium
    } else {
        Mode::Compact
    }
}

pub(super) fn render(frame: &mut Frame, cards: &[BookCard], ctx: &Ctx) {
    let ready: Vec<&BookCard> = cards
        .iter()
        .filter(|card| card.analysis.is_some() && card.timestamp.is_some())
        .collect();
    if ready.is_empty() {
        return;
    }
    let area = frame.area();
    let mode = mode_for(area);
    if mode == Mode::Compact {
        compact::render(frame, &ready, ctx);
        return;
    }
    let index = ctx.focus % ready.len();
    dashboard(frame, area, ready[index], ctx, mode, (index, ready.len()));
}

fn dashboard(
    frame: &mut Frame,
    area: Rect,
    card: &BookCard,
    ctx: &Ctx,
    mode: Mode,
    symbols: (usize, usize),
) {
    let (Some(analysis), Some(timestamp)) = (card.analysis.as_ref(), card.timestamp) else {
        return;
    };
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    render_header(frame, header_area, card, analysis, timestamp, ctx);
    render_footer(frame, footer_area, ctx, symbols);

    let quote = quote_asset(&card.symbol);
    let base = base_asset(&card.symbol);

    if mode == Mode::Full {
        let [ladder, middle, right] = Layout::horizontal([
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ])
        .areas(body_area);
        render_ladder(frame, ladder, card, analysis, ctx);
        let [imbalance, bands] =
            Layout::vertical([Constraint::Length(7), Constraint::Min(0)]).areas(middle);
        render_imbalance(frame, imbalance, analysis);
        render_bands(frame, bands, analysis);
        let [impact, l3] =
            Layout::vertical([Constraint::Length(6), Constraint::Min(0)]).areas(right);
        render_impact(frame, impact, analysis, quote);
        render_l3(frame, l3, analysis, base);
    } else {
        let [ladder, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(body_area);
        render_ladder(frame, ladder, card, analysis, ctx);
        let [imbalance, bands, rest] = Layout::vertical([
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(0),
        ])
        .areas(right);
        render_imbalance(frame, imbalance, analysis);
        render_bands(frame, bands, analysis);
        if rest.height >= 6 {
            render_impact(frame, rest, analysis, quote);
        }
    }
}

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::bordered()
        .border_style(gray())
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {} ", title.into()), bright()))
}

pub(super) fn sign_style(value: Decimal) -> Style {
    if value.is_zero() {
        gray()
    } else if value.is_sign_positive() {
        green()
    } else {
        red()
    }
}

pub(super) fn label(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), gray())
}

pub(super) fn value(text: String) -> Span<'static> {
    Span::styled(text, bright())
}

/// Feed and book condition, most urgent first. `LOCKED` is added to any of them.
pub(super) fn status_spans(a: &Analysis, age: i64, paused: bool) -> Vec<Span<'static>> {
    let mut status = Vec::with_capacity(2);
    if paused {
        status.push(Span::styled("❚❚ PAUSED", yellow().patch(bright())));
    } else if a.is_crossed() {
        status.push(Span::styled("✖ CROSSED", red().patch(bright())));
    } else if age > STALE_MS {
        status.push(Span::styled("● STALE", red().patch(bright())));
    } else {
        status.push(Span::styled("● LIVE", green().patch(bright())));
    }
    if a.is_locked() {
        status.push(Span::styled(" LOCKED", yellow().patch(bright())));
    }
    status
}

pub(super) fn age_style(age: i64) -> Style {
    if age < 100 {
        green()
    } else if age <= 300 {
        yellow()
    } else {
        red()
    }
}

/// Bid/ask ratio text and color: green above one, red below.
pub(super) fn ratio_cell(bid: Decimal, ask: Decimal) -> (String, Style) {
    if ask.is_zero() {
        return ("—".to_string(), Style::default());
    }
    let shown = (bid / ask).round_dp(2);
    let style = if shown > Decimal::ONE {
        green()
    } else if shown < Decimal::ONE {
        red()
    } else {
        Style::default()
    };
    (format!("{shown:.2}"), style)
}

// ----- header / footer ----------------------------------------------------------------

fn render_header(
    frame: &mut Frame,
    area: Rect,
    card: &BookCard,
    a: &Analysis,
    timestamp: Ts,
    ctx: &Ctx,
) {
    let dp = a.price_dp;
    let age = age_ms(ctx.now_utc, timestamp);

    let mut first = status_spans(a, age, ctx.paused);
    first.push(Span::raw("   "));
    first.push(value(format!("{}", card.per_sec)));
    first.push(label(" upd/s   lag "));
    match card.lag_percentiles() {
        Some((p50, p99)) => {
            first.push(label("p50 "));
            first.push(value(format_age(p50)));
            first.push(label(" · p99 "));
            first.push(value(format_age(p99)));
        }
        None => first.push(label("—")),
    }
    first.push(label(" · age "));
    first.push(Span::styled(format_age(age), age_style(age)));
    first.push(Span::raw("     "));
    first.push(label("orders  bid "));
    first.push(value(format!("{}", a.bid_orders)));
    first.push(label(" · ask "));
    first.push(value(format!("{}", a.ask_orders)));

    let panel_ref = &a.panel;
    let mut second = vec![
        label("MID "),
        value(format_fixed(panel_ref.mid, dp + 1)),
        label("   SPREAD "),
        value(format_fixed(panel_ref.spread, dp)),
        label(format!(" = {} tick", a.spread_ticks.round_dp(1).normalize()).as_str()),
        label(format!(" = {} bps", format_spread_bps(panel_ref.spread_bps)).as_str()),
        label("   MICRO "),
        value(format_fixed(panel_ref.micro, dp + 1)),
        Span::styled(
            format!(" ({} tick)", format_signed(a.micro_offset_ticks, 2)),
            sign_style(a.micro_offset_ticks),
        ),
        label("   OFI "),
    ];
    for (name, window) in [("1s", 1), ("10s", 10), ("60s", 60)] {
        let ofi = card.ofi_over(Duration::from_secs(window), ctx.now);
        second.push(label(name));
        second.push(Span::raw(" "));
        second.push(Span::styled(format_signed(ofi, 2), sign_style(ofi)));
        second.push(Span::raw("  "));
    }

    let block = Block::bordered()
        .border_style(gray())
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {} ", card.symbol),
            cyan().patch(bright()),
        ));
    frame.render_widget(
        Paragraph::new(vec![Line::from(first), Line::from(second)]).block(block),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, ctx: &Ctx, symbols: (usize, usize)) {
    let mut spans = vec![
        Span::styled(" q", bright()),
        label(" quit  "),
        Span::styled("g", bright()),
        label(format!(" group {}x  ", ctx.group).as_str()),
        Span::styled("p", bright()),
        label(if ctx.paused { " resume  " } else { " pause  " }),
    ];
    if symbols.1 > 1 {
        spans.push(Span::styled("Tab", bright()));
        spans.push(label(
            format!(" symbol {}/{}  ", symbols.0 + 1, symbols.1).as_str(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ----- ladder -------------------------------------------------------------------------

const ORD_W: usize = 5;
const SIZE_W: usize = 11;
const CUM_W: usize = 11;

fn render_ladder(frame: &mut Frame, area: Rect, card: &BookCard, a: &Analysis, ctx: &Ctx) {
    let block = Block::bordered().border_style(gray()).title(Span::styled(
        format!(" LADDER  group {}x ", ctx.group),
        bright(),
    ));
    let inner = block.inner(area);
    let rows = usize::from(inner.height).saturating_sub(2) / 2;
    let width = usize::from(inner.width);
    let dp = a.price_dp;

    let asks: Vec<&Level> = card.ask_ladder.iter().take(rows).collect();
    let bids: Vec<&Level> = card.bid_ladder.iter().take(rows).collect();
    let price_w = asks
        .iter()
        .chain(&bids)
        .map(|level| text_width(&format_fixed(level.price, dp)))
        .max()
        .unwrap_or(0)
        .max(5);
    let fixed = ORD_W + SIZE_W + CUM_W + price_w + 4;
    let bar_w = width.saturating_sub(fixed + 1);
    let max_cum = asks
        .last()
        .map(|level| level.cum)
        .into_iter()
        .chain(bids.last().map(|level| level.cum))
        .max()
        .unwrap_or(Decimal::ZERO);

    let mut lines: Vec<Line> = Vec::with_capacity(usize::from(inner.height));
    lines.push(Line::from(Span::styled(
        format!(
            "{} {} {} {} {}",
            pad_left("#ord", ORD_W),
            pad_left("size", SIZE_W),
            pad_left("cum", CUM_W),
            pad_left("price", price_w),
            "depth"
        ),
        gray(),
    )));
    for _ in asks.len()..rows {
        lines.push(Line::raw(""));
    }
    for level in asks.iter().rev() {
        lines.push(level_line(
            level, true, card, ctx, dp, price_w, bar_w, max_cum,
        ));
    }
    lines.push(mid_line(a, width));
    for level in &bids {
        lines.push(level_line(
            level, false, card, ctx, dp, price_w, bar_w, max_cum,
        ));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[allow(clippy::too_many_arguments)]
fn level_line(
    level: &Level,
    is_ask: bool,
    card: &BookCard,
    ctx: &Ctx,
    dp: u32,
    price_w: usize,
    bar_w: usize,
    max_cum: Decimal,
) -> Line<'static> {
    let flash = card
        .flashes
        .get(&level.price)
        .filter(|flash| ctx.now.duration_since(flash.at) < FLASH_FOR);
    let size_style = match flash {
        Some(flash) if flash.up => flash_up(),
        Some(_) => flash_down(),
        None => Style::default(),
    };
    let side = if is_ask { red() } else { green() };
    let bar_len = if max_cum.is_zero() {
        0
    } else {
        (metrics::to_f64(level.cum / max_cum) * bar_w as f64).round() as usize
    }
    .min(bar_w);
    let bar_style = if is_ask { ask_bar() } else { bid_bar() };
    Line::from(vec![
        Span::styled(pad_left(&level.orders.to_string(), ORD_W), gray()),
        Span::raw(" "),
        Span::styled(pad_left(&format_fixed(level.qty, 4), SIZE_W), size_style),
        Span::raw(" "),
        Span::styled(pad_left(&format_fixed(level.cum, 2), CUM_W), gray()),
        Span::raw(" "),
        Span::styled(
            pad_left(&format_fixed(level.price, dp), price_w),
            side.patch(bright()),
        ),
        Span::raw(" "),
        Span::styled("█".repeat(bar_len), bar_style),
    ])
}

fn mid_line(a: &Analysis, width: usize) -> Line<'static> {
    let style = if a.is_crossed() || a.is_locked() {
        red().patch(bright())
    } else {
        cyan().patch(bright())
    };
    let text = format!(
        " {}  spread {} ",
        format_fixed(a.panel.mid, a.price_dp + 1),
        format_fixed(a.panel.spread, a.price_dp)
    );
    let fill = width.saturating_sub(text_width(&text));
    let left = fill / 2;
    Line::from(vec![
        Span::styled("─".repeat(left), gray()),
        Span::styled(text, style),
        Span::styled("─".repeat(fill - left), gray()),
    ])
}

// ----- side panels --------------------------------------------------------------------

fn render_imbalance(frame: &mut Frame, area: Rect, a: &Analysis) {
    let block = panel("IMBALANCE  bid / ask");
    let inner = block.inner(area);
    let bar_w = usize::from(inner.width)
        .saturating_sub(5 + 1 + 6 + 1)
        .max(4);
    let lines: Vec<Line> = a
        .imbalances
        .iter()
        .map(|row| {
            let total = row.bid + row.ask;
            let share = if total.is_zero() {
                0.5
            } else {
                metrics::to_f64(row.bid / total)
            };
            let bid_len = ((share * bar_w as f64).round() as usize).min(bar_w);
            let imb = metrics::imbalance(row.bid, row.ask);
            Line::from(vec![
                label(&pad_right(row.label, 5)),
                Span::raw(" "),
                Span::styled("█".repeat(bid_len), green()),
                Span::styled("█".repeat(bar_w - bid_len), red()),
                Span::raw(" "),
                Span::styled(format_signed(imb, 2), sign_style(imb).patch(bright())),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_bands(frame: &mut Frame, area: Rect, a: &Analysis) {
    let bids: Vec<String> = a.bands.iter().map(|row| format_qty_auto(row.bid)).collect();
    let asks: Vec<String> = a.bands.iter().map(|row| format_qty_auto(row.ask)).collect();
    let bid_w = bids.iter().map(|s| text_width(s)).max().unwrap_or(0).max(3);
    let ask_w = asks.iter().map(|s| text_width(s)).max().unwrap_or(0).max(3);
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} {} {} {}",
            pad_right("±bps", 5),
            pad_left("bid", bid_w),
            pad_left("ask", ask_w),
            pad_left("ratio", 6)
        ),
        gray(),
    ))];
    for (index, row) in a.bands.iter().enumerate() {
        let (ratio, ratio_style) = ratio_cell(row.bid, row.ask);
        lines.push(Line::from(vec![
            Span::styled(pad_right(&row.bps.to_string(), 5), gray()),
            Span::raw(" "),
            Span::styled(pad_left(&bids[index], bid_w), green()),
            Span::raw(" "),
            Span::styled(pad_left(&asks[index], ask_w), red()),
            Span::raw(" "),
            Span::styled(pad_left(&ratio, 6), ratio_style.patch(bright())),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).block(panel("DEPTH")), area);
}

fn render_impact(frame: &mut Frame, area: Rect, a: &Analysis, quote: &str) {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "{} {} {}",
            pad_right("size", 10),
            pad_left("buy", 7),
            pad_left("sell", 7)
        ),
        gray(),
    ))];
    for row in &a.impact {
        lines.push(Line::from(vec![
            Span::styled(pad_right(&notional_label(row.notional, quote), 10), gray()),
            Span::raw(" "),
            impact_cell(row.buy_bps),
            Span::raw(" "),
            impact_cell(row.sell_bps),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).block(panel("IMPACT bps")), area);
}

fn notional_label(notional: i64, quote: &str) -> String {
    let short = if notional >= 1_000_000 {
        format!("{}M", notional / 1_000_000)
    } else {
        format!("{}k", notional / 1_000)
    };
    format!("{short} {quote}").trim_end().to_string()
}

fn impact_cell(bps: Option<Decimal>) -> Span<'static> {
    match bps {
        None => Span::styled(pad_left("—", 7), gray()),
        Some(bps) => {
            let style = if bps >= Decimal::from(10) {
                red()
            } else if bps >= Decimal::from(3) {
                yellow()
            } else {
                Style::default()
            };
            Span::styled(pad_left(&format_bps(bps), 7), style.patch(bright()))
        }
    }
}

fn render_l3(frame: &mut Frame, area: Rect, a: &Analysis, base: &str) {
    let dp = a.price_dp;
    let mut lines = vec![
        Line::from(vec![
            label("avg order  bid "),
            value(format_qty_auto(a.avg_bid_order)),
            label(" · ask "),
            value(format_qty_auto(a.avg_ask_order)),
            label(format!(" {base}").as_str()),
        ]),
        big_line("max bid", a.big_bid.as_ref(), dp, green()),
        big_line("max ask", a.big_ask.as_ref(), dp, red()),
    ];
    lines.push(Line::from(label("◆ = ≥10× the side's average order")));
    frame.render_widget(Paragraph::new(lines).block(panel("L3 ORDERS")), area);
}

fn big_line(name: &str, big: Option<&BigOrder>, dp: u32, side: Style) -> Line<'static> {
    let Some(big) = big else {
        return Line::from(label(name));
    };
    let mut spans = vec![
        label(&format!("{name} ")),
        Span::styled(format_qty_auto(big.qty), side.patch(bright())),
        label(" @ "),
        value(format_fixed(big.price, dp)),
        label(&format!(" ({} bps) ", format_signed(big.from_mid_bps, 1))),
    ];
    if big.is_wall() {
        spans.push(Span::styled(
            format!("◆ {}×", big.multiple.round_dp(0)),
            yellow().patch(bright()),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{GROUPS, Screen};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn d(value: &str) -> Decimal {
        value.parse().unwrap()
    }

    fn sample_book() -> (Vec<(Decimal, Decimal)>, Vec<(Decimal, Decimal)>) {
        let asks = (0..40)
            .map(|i| (d("100.1") + Decimal::new(i, 1), d("1.5")))
            .collect();
        let bids = (0..40)
            .map(|i| (d("100.0") - Decimal::new(i, 1), d("2.5")))
            .collect();
        (asks, bids)
    }

    fn render_to_string(width: u16, height: u16) -> String {
        let (asks, bids) = sample_book();
        render_book(&asks, &bids, 0, false, width, height)
    }

    fn render_book(
        asks: &[(Decimal, Decimal)],
        bids: &[(Decimal, Decimal)],
        age_ms: i64,
        paused: bool,
        width: u16,
        height: u16,
    ) -> String {
        let mut screen = Screen {
            last_paint: None,
            raw_mode: false,
            terminal: None,
            cards: Vec::new(),
            events: None,
            stop_input: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            input: None,
            group: 0,
            focus: 0,
            frozen: None,
        };
        let card = screen.card_mut("BTC/USD");
        let analysis = Analysis::from_book(asks, bids).unwrap();
        let now = Ts::now();
        let timestamp = Ts(now.0 - age_ms * 1_000_000);
        card.ingest(asks, bids, analysis, timestamp, 1, Instant::now(), now);
        let ctx = Ctx {
            now_utc: now,
            now: Instant::now(),
            group: GROUPS[0],
            paused,
            focus: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render(frame, &screen.cards, &ctx))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn full_dashboard_shows_every_panel() {
        let out = render_to_string(140, 40);
        for needle in [
            "BTC/USD",
            "LIVE",
            "LADDER",
            "IMBALANCE",
            "DEPTH",
            "IMPACT",
            "L3 ORDERS",
            "MID",
            "MICRO",
            "OFI",
        ] {
            assert!(out.contains(needle), "missing {needle}\n{out}");
        }
    }

    #[test]
    fn medium_dashboard_renders() {
        let out = render_to_string(100, 30);
        assert!(out.contains("LADDER") && out.contains("IMBALANCE"));
    }

    #[test]
    fn small_terminal_falls_back_to_compact_card() {
        let out = render_to_string(70, 12);
        assert!(!out.contains("LADDER"));
        for needle in [
            "BTC/USD",
            "LIVE",
            "age 0ms",
            "BID 100.0 × 2.5000",
            "ASK 100.1 × 1.5000",
            "1 tick 10.0bp",
            "MID 100.05",
            "MICRO +0.12t",
            "OFI 1s +0.00",
            "IMB L1 +0.25",
            "L10 +0.25",
            "1.0% +0.25",
            "DEPTH 10bp",
            "100bp",
            "q quit",
        ] {
            assert!(out.contains(needle), "missing {needle}\n{out}");
        }
    }

    #[test]
    fn compact_uses_the_pair_tick_for_prices() {
        let asks = vec![(d("0.52342"), d("10"))];
        let bids = vec![(d("0.52341"), d("10"))];
        let out = render_book(&asks, &bids, 0, false, 70, 12);
        assert!(out.contains("BID 0.52341"), "{out}");
        assert!(out.contains("ASK 0.52342"), "{out}");
        assert!(out.contains("MID 0.523415"), "{out}");
    }

    #[test]
    fn compact_reports_feed_and_book_problems() {
        let (asks, bids) = sample_book();
        let out = render_book(&asks, &bids, 5_000, false, 70, 12);
        assert!(out.contains("STALE"), "{out}");

        let out = render_book(&asks, &bids, 0, true, 70, 12);
        assert!(out.contains("PAUSED") && out.contains("p resume"), "{out}");

        let crossed = vec![(d("100.5"), d("1"))];
        let out = render_book(&asks, &crossed, 0, false, 70, 12);
        assert!(out.contains("CROSSED"), "{out}");
    }

    #[test]
    fn compact_never_clips_a_number_on_narrow_terminals() {
        let (asks, bids) = sample_book();
        let out = render_book(&asks, &bids, 0, false, 40, 12);
        // Sizes go first, and both prices stay.
        assert!(
            out.contains("BID 100.0") && out.contains("ASK 100.1"),
            "{out}"
        );
        assert!(!out.contains("× "), "{out}");
        // The second depth band doesn't fit, so it is gone rather than cut off.
        assert!(
            out.contains("DEPTH 10bp") && !out.contains("100bp"),
            "{out}"
        );
    }

    #[test]
    fn compact_keeps_the_touch_when_very_short() {
        let (asks, bids) = sample_book();
        let out = render_book(&asks, &bids, 0, false, 70, 4);
        assert!(out.contains("BID 100.0"), "{out}");
        assert!(!out.contains("q quit"), "{out}");
    }
}
