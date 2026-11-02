//! Compact view for small terminals: one six-row card per symbol, stacked from the top.
//! It reads the same `Analysis` as the dashboard, so numbers don't change across a resize.

use super::BookCard;
use super::format::{
    age_ms, format_age, format_fixed, format_qty_auto, format_signed, format_spread_bps, text_width,
};
use super::metrics::{Analysis, imbalance};
use super::render::{Ctx, age_style, label, ratio_cell, sign_style, status_spans, value};
use super::style::{bright, cyan, gray, green, red};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};
use rust_decimal::Decimal;
use std::time::Duration;

/// Borders plus four content rows.
const CARD_HEIGHT: u16 = 6;
const CARD_GAP: u16 = 1;
const FIELD_GAP: &str = "   ";
/// Width lost to the borders and the horizontal padding.
const CARD_CHROME: u16 = 4;
const SIZE_DP: u32 = 4;
const DEPTH_BANDS_BPS: [i64; 2] = [10, 100];
const IMBALANCE_ROWS: [usize; 3] = [0, 2, 4];
const OFI_WINDOWS: [(&str, u64); 2] = [("OFI 1s", 1), ("10s", 10)];

/// A group of spans that is either shown whole or not at all.
type Field = Vec<Span<'static>>;

pub(super) fn render(frame: &mut Frame, cards: &[&BookCard], ctx: &Ctx) {
    let area = frame.area();
    // The cards come first: the footer only takes a row that one card doesn't need.
    let has_footer = area.height > CARD_HEIGHT;
    let cards_bottom = area.bottom() - u16::from(has_footer);

    let mut y = area.y;
    for card in cards {
        if y >= cards_bottom {
            break;
        }
        let height = CARD_HEIGHT.min(cards_bottom - y);
        render_card(frame, Rect { y, height, ..area }, card, ctx);
        y = y.saturating_add(height + CARD_GAP);
    }

    if has_footer {
        let footer = Rect {
            y: cards_bottom,
            height: 1,
            ..area
        };
        frame.render_widget(Paragraph::new(footer_line(ctx.paused)), footer);
    }
}

fn footer_line(paused: bool) -> Line<'static> {
    Line::from(vec![
        Span::styled(" q", bright()),
        label(" quit  "),
        Span::styled("p", bright()),
        label(if paused { " resume" } else { " pause" }),
    ])
}

fn render_card(frame: &mut Frame, area: Rect, card: &BookCard, ctx: &Ctx) {
    let (Some(a), Some(timestamp)) = (card.analysis.as_ref(), card.timestamp) else {
        return;
    };
    let age = age_ms(ctx.now_utc - timestamp);

    let mut title = vec![Span::styled(
        format!(" {} ", card.symbol),
        cyan().patch(bright()),
    )];
    title.extend(status_spans(a, age, ctx.paused));
    title.push(Span::raw(" "));

    let rate = Line::from(vec![
        Span::raw(" "),
        value(card.per_sec.to_string()),
        label("/s  age "),
        Span::styled(format_age(age), age_style(age)),
        Span::raw(" "),
    ]);

    let block = Block::bordered()
        .border_style(gray())
        .padding(Padding::horizontal(1))
        .title_top(Line::from(title))
        .title_top(rate.right_aligned());

    let width = usize::from(area.width.saturating_sub(CARD_CHROME));
    let lines = vec![
        touch_line(a, width),
        fit_fields(price_fields(a, card, ctx), width),
        fit_fields(imbalance_fields(a), width),
        fit_fields(depth_fields(a), width),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Keep adding fields while they fit. A clipped number reads as a different number,
/// so a field that doesn't fit is dropped whole, along with everything after it.
fn fit_fields(fields: Vec<Field>, width: usize) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for field in fields {
        let gap = if spans.is_empty() { 0 } else { FIELD_GAP.len() };
        let needed = field_width(&field);
        if used + gap + needed > width {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw(FIELD_GAP));
        }
        spans.extend(field);
        used += gap + needed;
    }
    Line::from(spans)
}

/// Best bid and ask, then the spread. Both prices matter more than their sizes,
/// so on a very narrow card the sizes go before either price does.
fn touch_line(a: &Analysis, width: usize) -> Line<'static> {
    let spread = vec![label(&format!(
        "{} tick {}bp",
        a.spread_ticks.round_dp(1).normalize(),
        format_spread_bps(a.panel.spread_bps)
    ))];
    let full = vec![
        quote_field("BID", a.touch.bid_price, Some(a.touch.bid_qty), a, true),
        quote_field("ASK", a.touch.ask_price, Some(a.touch.ask_qty), a, false),
        spread,
    ];
    let sized_width = field_width(&full[0]) + FIELD_GAP.len() + field_width(&full[1]);
    if sized_width <= width {
        return fit_fields(full, width);
    }
    fit_fields(
        vec![
            quote_field("BID", a.touch.bid_price, None, a, true),
            quote_field("ASK", a.touch.ask_price, None, a, false),
        ],
        width,
    )
}

fn field_width(field: &Field) -> usize {
    field.iter().map(|s| text_width(&s.content)).sum()
}

fn quote_field(
    name: &str,
    price: Decimal,
    qty: Option<Decimal>,
    a: &Analysis,
    is_bid: bool,
) -> Field {
    let side = if is_bid { green() } else { red() };
    let mut field = vec![
        label(&format!("{name} ")),
        Span::styled(format_fixed(price, a.price_dp), side.patch(bright())),
    ];
    if let Some(qty) = qty {
        field.push(label(" × "));
        field.push(value(format_fixed(qty, SIZE_DP)));
    }
    field
}

fn price_fields(a: &Analysis, card: &BookCard, ctx: &Ctx) -> Vec<Field> {
    let mut fields = vec![
        vec![
            label("MID "),
            value(format_fixed(a.panel.mid, a.price_dp + 1)),
        ],
        vec![
            label("MICRO "),
            Span::styled(
                format!("{}t", format_signed(a.micro_offset_ticks, 2)),
                sign_style(a.micro_offset_ticks),
            ),
        ],
    ];
    for (name, seconds) in OFI_WINDOWS {
        let ofi = card.ofi_over(Duration::from_secs(seconds), ctx.now);
        fields.push(vec![
            label(&format!("{name} ")),
            Span::styled(format_signed(ofi, 2), sign_style(ofi)),
        ]);
    }
    fields
}

fn imbalance_fields(a: &Analysis) -> Vec<Field> {
    IMBALANCE_ROWS
        .iter()
        .enumerate()
        .map(|(position, &index)| {
            let row = &a.imbalances[index];
            let imb = imbalance(row.bid, row.ask);
            let prefix = if position == 0 { "IMB " } else { "" };
            vec![
                label(&format!("{prefix}{} ", row.label)),
                Span::styled(format_signed(imb, 2), sign_style(imb).patch(bright())),
            ]
        })
        .collect()
}

fn depth_fields(a: &Analysis) -> Vec<Field> {
    DEPTH_BANDS_BPS
        .iter()
        .enumerate()
        .filter_map(|(position, &bps)| {
            let row = a.bands.iter().find(|row| row.bps == bps)?;
            let prefix = if position == 0 { "DEPTH " } else { "" };
            let (ratio, ratio_style) = ratio_cell(row.bid, row.ask);
            Some(vec![
                label(&format!("{prefix}{bps}bp ")),
                Span::styled(format_qty_auto(row.bid), green()),
                label("/"),
                Span::styled(format_qty_auto(row.ask), red()),
                Span::raw(" "),
                Span::styled(ratio, ratio_style.patch(bright())),
            ])
        })
        .collect()
}
