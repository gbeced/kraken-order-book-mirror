use chrono::{DateTime, Duration as ChronoDuration, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const PAINT_INTERVAL: Duration = Duration::from_millis(100);
const WIDE_WIDTH: u16 = 80;
const FIELD_GAP: &str = "   ";

pub(crate) enum UiEvent {
    Quit,
    Resize,
}

pub struct Screen {
    last_paint: Option<Instant>,
    raw_mode: bool,
    terminal: Option<DefaultTerminal>,
    cards: Vec<BookCard>,
    events: Option<mpsc::UnboundedReceiver<UiEvent>>,
    stop_input: Arc<AtomicBool>,
    input: Option<JoinHandle<()>>,
}

struct BookCard {
    symbol: String,
    updates: UpdateRate,
    per_sec: u64,
    metrics: Option<Panel>,
    timestamp: Option<DateTime<Utc>>,
}

impl BookCard {
    fn new(symbol: &str) -> BookCard {
        BookCard {
            symbol: symbol.to_string(),
            updates: UpdateRate::default(),
            per_sec: 0,
            metrics: None,
            timestamp: None,
        }
    }

    fn record(&mut self, now: Instant) {
        self.per_sec = self.updates.record(now);
    }
}

#[derive(Clone)]
struct CardView {
    symbol: String,
    per_sec: u64,
    metrics: Panel,
    timestamp: DateTime<Utc>,
}

impl Screen {
    pub fn new() -> Result<Screen, String> {
        let stop_input = Arc::new(AtomicBool::new(false));
        let terminal = match ratatui::try_init() {
            Ok(terminal) => terminal,
            Err(err) => {
                ratatui::restore();
                return Err(err.to_string());
            }
        };
        let (sender, receiver) = mpsc::unbounded_channel();
        Ok(Screen {
            last_paint: None,
            raw_mode: true,
            terminal: Some(terminal),
            cards: Vec::new(),
            events: Some(receiver),
            stop_input: Arc::clone(&stop_input),
            input: Some(spawn_input_thread(stop_input, sender)),
        })
    }

    pub(crate) fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<UiEvent>> {
        self.events.take()
    }

    /// Count this applied frame in the symbol's one-second rate window.
    ///
    /// Returns whether the throttle allows a paint. The first attempt is immediate.
    /// Later attempts wait 100ms. A paint that [`draw`] skips does not move the stamp,
    /// so the next frame is due again.
    pub fn note_update(&mut self, symbol: &str) -> bool {
        self.card_mut(symbol).record(Instant::now());
        self.paint_due()
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
        let Some(panel) = panel_from_book(asks, bids) else {
            return Ok(());
        };
        let card = self.card_mut(symbol);
        card.metrics = Some(panel);
        card.timestamp = Some(timestamp);
        self.paint()?;
        self.last_paint = Some(Instant::now());
        Ok(())
    }

    pub(crate) fn redraw(&mut self) -> Result<(), String> {
        if self.cards.iter().all(|card| card.metrics.is_none()) {
            return Ok(());
        }
        self.paint()
    }

    fn card_mut(&mut self, symbol: &str) -> &mut BookCard {
        if let Some(index) = self.cards.iter().position(|card| card.symbol == symbol) {
            return &mut self.cards[index];
        }
        self.cards.push(BookCard::new(symbol));
        self.cards.last_mut().expect("card just pushed")
    }

    fn paint_due(&self) -> bool {
        match self.last_paint {
            None => true,
            Some(stamp) => stamp.elapsed() >= PAINT_INTERVAL,
        }
    }

    fn paint(&mut self) -> Result<(), String> {
        let now = Utc::now();
        let cards = self.visible_cards();
        self.terminal
            .as_mut()
            .expect("ratatui terminal")
            .draw(|frame| render(frame, &cards, now))
            .map_err(|err| err.to_string())?;
        Ok(())
    }

    fn visible_cards(&self) -> Vec<CardView> {
        self.cards
            .iter()
            .filter_map(|card| {
                Some(CardView {
                    symbol: card.symbol.clone(),
                    per_sec: card.per_sec,
                    metrics: card.metrics?,
                    timestamp: card.timestamp?,
                })
            })
            .collect()
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.stop_input.store(true, Ordering::Relaxed);
        if let Some(handle) = self.input.take() {
            let _ = handle.join();
        }
        self.terminal.take();
        if self.raw_mode {
            ratatui::restore();
            self.raw_mode = false;
        }
    }
}

fn spawn_input_thread(
    stop: Arc<AtomicBool>,
    sender: mpsc::UnboundedSender<UiEvent>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let ready = match event::poll(Duration::from_millis(100)) {
                Ok(ready) => ready,
                Err(_) => {
                    let _ = sender.send(UiEvent::Quit);
                    break;
                }
            };
            if !ready {
                continue;
            }
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press && is_quit(&key) => {
                    let _ = sender.send(UiEvent::Quit);
                    break;
                }
                Ok(Event::Resize(_, _)) => {
                    let _ = sender.send(UiEvent::Resize);
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = sender.send(UiEvent::Quit);
                    break;
                }
            }
        }
    })
}

fn is_quit(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
        _ => false,
    }
}

fn render(frame: &mut Frame, cards: &[CardView], now: DateTime<Utc>) {
    if cards.is_empty() {
        return;
    }
    let area = frame.area();
    let fields: Vec<DepthFields> = cards.iter().map(DepthFields::from_card).collect();
    let inner = usize::from(area.width.saturating_sub(4));
    let show_bps = area.width >= WIDE_WIDTH;
    let layout = column_layout(cards, &fields, inner, show_bps, now);

    let mut constraints = Vec::with_capacity(cards.len() * 2 + 1);
    for _card in cards {
        if !constraints.is_empty() {
            constraints.push(Constraint::Length(1));
        }
        constraints.push(Constraint::Length(card_height(layout.wrap)));
    }
    // The trailing Min absorbs leftover rows so the last card stays its natural height.
    constraints.push(Constraint::Min(0));
    let areas = Layout::vertical(constraints).split(area);
    let mut slot = 0;
    for (index, card) in cards.iter().enumerate() {
        if index > 0 {
            slot += 1;
        }
        render_card(frame, areas[slot], card, &fields[index], &layout, now);
        slot += 1;
    }
}

fn card_height(wrap_header: bool) -> u16 {
    // Borders plus header, two depth rows, and the footer. A wrapped header adds one row.
    if wrap_header { 7 } else { 6 }
}

struct RowLayout {
    columns: [usize; 4],
    cols: Cols,
    short_labels: bool,
    show_bps: bool,
    wrap: bool,
}

fn render_card(
    frame: &mut Frame,
    area: Rect,
    card: &CardView,
    fields: &DepthFields,
    layout: &RowLayout,
    now: DateTime<Utc>,
) {
    let lines = card_lines(card, fields, layout, now);
    let block = Block::bordered()
        .padding(Padding::horizontal(1))
        .title(format!(" {} ", card.symbol));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn column_layout(
    cards: &[CardView],
    fields: &[DepthFields],
    inner: usize,
    show_bps: bool,
    now: DateTime<Utc>,
) -> RowLayout {
    let cols = Cols::from_fields(fields);
    let full = column_widths(cards, fields, &cols, show_bps, false, now);
    if row_width(&full) <= inner {
        return RowLayout {
            columns: full,
            cols,
            short_labels: false,
            show_bps,
            wrap: false,
        };
    }
    let short = column_widths(cards, fields, &cols, show_bps, true, now);
    RowLayout {
        columns: short,
        cols,
        short_labels: true,
        show_bps,
        wrap: row_width(&short) > inner,
    }
}

fn column_widths(
    cards: &[CardView],
    fields: &[DepthFields],
    cols: &Cols,
    show_bps: bool,
    short_labels: bool,
    now: DateTime<Utc>,
) -> [usize; 4] {
    let mut columns = [0usize; 4];
    for (card, field) in cards.iter().zip(fields) {
        for row in card_rows(card, field, cols, show_bps, short_labels, now) {
            for (column, cell) in columns.iter_mut().zip(row) {
                *column = (*column).max(cell.width);
            }
        }
    }
    columns
}

fn row_width(columns: &[usize]) -> usize {
    let gaps = FIELD_GAP.len() * columns.len().saturating_sub(1);
    columns.iter().sum::<usize>() + gaps
}

fn card_lines(
    card: &CardView,
    fields: &DepthFields,
    layout: &RowLayout,
    now: DateTime<Utc>,
) -> Vec<Line<'static>> {
    let (half_label, one_label) = depth_labels(layout.short_labels);
    let mut lines = Vec::new();
    if layout.wrap {
        lines.push(join_fields(
            vec![
                mid_field(&card.metrics),
                spread_field(&card.metrics, layout.show_bps),
            ],
            &layout.columns,
        ));
        let indent = layout.columns[0] + FIELD_GAP.len() + layout.columns[1] + FIELD_GAP.len();
        let mut second = vec![Span::raw(" ".repeat(indent))];
        second.extend(join_spans(
            vec![micro_field(&card.metrics), imb_field(&card.metrics)],
            &layout.columns[2..],
        ));
        lines.push(Line::from(second));
    } else {
        lines.push(join_fields(
            vec![
                mid_field(&card.metrics),
                spread_field(&card.metrics, layout.show_bps),
                micro_field(&card.metrics),
                imb_field(&card.metrics),
            ],
            &layout.columns,
        ));
    }
    lines.push(depth_row_line(
        half_label,
        &fields.half_bid,
        &fields.half_ask,
        &fields.half_ratio,
        fields.half_ratio_style,
        &fields.asset,
        layout,
    ));
    lines.push(depth_row_line(
        one_label,
        &fields.one_bid,
        &fields.one_ask,
        &fields.one_ratio,
        fields.one_ratio_style,
        &fields.asset,
        layout,
    ));
    let age_ms = age_ms(now - card.timestamp);
    lines.push(join_fields(
        vec![
            updates_field(card.per_sec),
            last_field(format_age_ago(age_ms), age_style(age_ms)),
        ],
        &layout.columns,
    ));
    lines
}

fn card_rows(
    card: &CardView,
    fields: &DepthFields,
    cols: &Cols,
    show_bps: bool,
    short_labels: bool,
    now: DateTime<Utc>,
) -> [Vec<Field>; 4] {
    let (half_label, one_label) = depth_labels(short_labels);
    let age = format_age_ago(age_ms(now - card.timestamp));
    [
        vec![
            mid_field(&card.metrics),
            spread_field(&card.metrics, show_bps),
            micro_field(&card.metrics),
            imb_field(&card.metrics),
        ],
        depth_fields(
            half_label,
            &fields.half_bid,
            &fields.half_ask,
            &fields.half_ratio,
            fields.half_ratio_style,
            &fields.asset,
            cols,
        ),
        depth_fields(
            one_label,
            &fields.one_bid,
            &fields.one_ask,
            &fields.one_ratio,
            fields.one_ratio_style,
            &fields.asset,
            cols,
        ),
        vec![
            updates_field(card.per_sec),
            last_field(age, age_style(age_ms(now - card.timestamp))),
            Field::empty(),
            Field::empty(),
        ],
    ]
}

fn depth_labels(short: bool) -> (&'static str, &'static str) {
    if short {
        ("0.5%", "1.0%")
    } else {
        ("0.5% Depth", "1.0% Depth")
    }
}

fn depth_row_line(
    label: &str,
    bid: &str,
    ask: &str,
    ratio: &str,
    ratio_style: Style,
    asset: &str,
    layout: &RowLayout,
) -> Line<'static> {
    join_fields(
        depth_fields(label, bid, ask, ratio, ratio_style, asset, &layout.cols),
        &layout.columns,
    )
}

fn depth_fields(
    label: &str,
    bid: &str,
    ask: &str,
    ratio: &str,
    ratio_style: Style,
    asset: &str,
    cols: &Cols,
) -> Vec<Field> {
    vec![
        Field::plain(label),
        Field::plain(format!(
            "Bid: {} {}",
            pad_left(bid, cols.bid),
            pad_right(asset, cols.asset),
        )),
        Field::plain(format!(
            "Ask: {} {}",
            pad_left(ask, cols.ask),
            pad_right(asset, cols.asset),
        )),
        ratio_field(ratio, ratio_style, cols.ratio),
    ]
}

fn mid_field(panel: &Panel) -> Field {
    Field::from_spans(vec![
        Span::raw("Mid: "),
        Span::styled(format_price(panel.mid), style_bright()),
    ])
}

fn spread_field(panel: &Panel, show_bps: bool) -> Field {
    let mut spans = vec![
        Span::raw("Spread: "),
        Span::styled(format_price(panel.spread), style_bright()),
    ];
    if show_bps {
        spans.push(Span::raw(format!(
            " ({} bps)",
            format_bps(panel.spread_bps)
        )));
    }
    Field::from_spans(spans)
}

fn micro_field(panel: &Panel) -> Field {
    Field::from_spans(vec![
        Span::raw("Micro: "),
        Span::styled(format_price(panel.micro), style_bright()),
    ])
}

fn imb_field(panel: &Panel) -> Field {
    Field::from_spans(vec![
        Span::raw("Imb: "),
        Span::styled(
            format_imbalance(panel.imbalance),
            imbalance_style(panel.imbalance),
        ),
    ])
}

fn updates_field(per_sec: u64) -> Field {
    Field::plain(format!("Updates: {per_sec}/s"))
}

fn last_field(age: String, style: Style) -> Field {
    Field::from_spans(vec![Span::raw("Last: "), Span::styled(age, style)])
}

fn ratio_field(ratio: &str, style: Style, width: usize) -> Field {
    Field::from_spans(vec![
        Span::raw("Ratio: "),
        Span::styled(pad_left(ratio, width), style),
    ])
}

fn join_fields(fields: Vec<Field>, columns: &[usize]) -> Line<'static> {
    Line::from(join_spans(fields, columns))
}

fn join_spans(fields: Vec<Field>, columns: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, field) in fields.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(FIELD_GAP));
        }
        let width = columns.get(index).copied().unwrap_or(field.width);
        spans.extend(field.pad_to(width).spans);
    }
    spans
}

struct Field {
    spans: Vec<Span<'static>>,
    width: usize,
}

impl Field {
    fn plain(text: impl Into<String>) -> Field {
        let text = text.into();
        let width = text_width(&text);
        Field {
            spans: vec![Span::raw(text)],
            width,
        }
    }

    fn from_spans(spans: Vec<Span<'static>>) -> Field {
        let width = spans.iter().map(|span| span.content.chars().count()).sum();
        Field { spans, width }
    }

    fn empty() -> Field {
        Field {
            spans: Vec::new(),
            width: 0,
        }
    }

    fn pad_to(mut self, column: usize) -> Field {
        let pad = column.saturating_sub(self.width);
        if pad > 0 {
            self.spans.push(Span::raw(" ".repeat(pad)));
            self.width += pad;
        }
        self
    }
}

struct DepthFields {
    half_bid: String,
    half_ask: String,
    half_ratio: String,
    half_ratio_style: Style,
    one_bid: String,
    one_ask: String,
    one_ratio: String,
    one_ratio_style: Style,
    asset: String,
}

impl DepthFields {
    fn from_card(card: &CardView) -> DepthFields {
        let (half_ratio, half_ratio_style) =
            ratio_parts(card.metrics.half_pct.bid, card.metrics.half_pct.ask);
        let (one_ratio, one_ratio_style) =
            ratio_parts(card.metrics.one_pct.bid, card.metrics.one_pct.ask);
        DepthFields {
            half_bid: format_qty(card.metrics.half_pct.bid),
            half_ask: format_qty(card.metrics.half_pct.ask),
            half_ratio,
            half_ratio_style,
            one_bid: format_qty(card.metrics.one_pct.bid),
            one_ask: format_qty(card.metrics.one_pct.ask),
            one_ratio,
            one_ratio_style,
            asset: base_asset(&card.symbol).to_string(),
        }
    }
}

struct Cols {
    bid: usize,
    ask: usize,
    asset: usize,
    ratio: usize,
}

impl Cols {
    fn from_fields(fields: &[DepthFields]) -> Cols {
        let mut cols = Cols {
            bid: 0,
            ask: 0,
            asset: 0,
            ratio: 0,
        };
        for field in fields {
            cols.bid = cols
                .bid
                .max(text_width(&field.half_bid))
                .max(text_width(&field.one_bid));
            cols.ask = cols
                .ask
                .max(text_width(&field.half_ask))
                .max(text_width(&field.one_ask));
            cols.asset = cols.asset.max(text_width(&field.asset));
            cols.ratio = cols
                .ratio
                .max(text_width(&field.half_ratio))
                .max(text_width(&field.one_ratio));
        }
        cols
    }
}

#[derive(Clone, Copy)]
struct DepthBand {
    bid: Decimal,
    ask: Decimal,
}

#[derive(Clone, Copy)]
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

fn style_green() -> Style {
    Style::default().fg(Color::Green)
}

fn style_red() -> Style {
    Style::default().fg(Color::Red)
}

fn style_yellow() -> Style {
    Style::default().fg(Color::Yellow)
}

fn style_gray() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn style_bright() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn imbalance_style(value: Decimal) -> Style {
    let near_zero = Decimal::new(5, 3);
    if value.abs() < near_zero {
        style_gray()
    } else if value.is_sign_positive() {
        style_green()
    } else {
        style_red()
    }
}

fn ratio_parts(bid: Decimal, ask: Decimal) -> (String, Style) {
    if ask.is_zero() {
        return ("—".to_string(), Style::default());
    }
    let ratio = bid / ask;
    (format!("{:.2}", ratio.round_dp(2)), ratio_style(ratio))
}

fn ratio_style(ratio: Decimal) -> Style {
    let shown = ratio.round_dp(2);
    if shown > Decimal::ONE {
        style_green()
    } else if shown < Decimal::ONE {
        style_red()
    } else {
        Style::default()
    }
}

fn age_style(ms: i64) -> Style {
    if ms < 100 {
        style_green()
    } else if ms <= 300 {
        style_yellow()
    } else {
        style_red()
    }
}

fn age_ms(age: ChronoDuration) -> i64 {
    age.num_milliseconds().max(0)
}

fn format_age_ago(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms}ms ago")
    } else if ms < 60_000 {
        let tenths = ms / 100;
        format!("{}.{}s ago", tenths / 10, tenths % 10)
    } else {
        format!("{}m ago", ms / 60_000)
    }
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
    format!("{:.1}", value.round_dp(1))
}

fn format_imbalance(value: Decimal) -> String {
    format!("{:+.2}", value.round_dp(2))
}

fn format_qty(value: Decimal) -> String {
    format!("{:.4}", value.round_dp(4))
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

fn pad_left(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text_width(text));
    format!("{}{text}", " ".repeat(pad))
}

fn pad_right(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text_width(text));
    format!("{text}{}", " ".repeat(pad))
}

fn text_width(text: &str) -> usize {
    text.chars().count()
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
