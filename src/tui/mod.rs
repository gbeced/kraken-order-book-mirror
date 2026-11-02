mod compact;
mod format;
mod metrics;
mod render;
mod style;

use chrono::{DateTime, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use format::age_ms;
use metrics::{Analysis, Level, Touch};
use ratatui::DefaultTerminal;
use rust_decimal::Decimal;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const PAINT_INTERVAL: Duration = Duration::from_millis(100);
/// Rows kept per ladder side. The renderer shows as many as fit.
const LADDER_ROWS: usize = 64;
/// Price grouping choices, in ticks. `g` cycles through them.
const GROUPS: [u32; 4] = [1, 5, 10, 50];
const FLASH_FOR: Duration = Duration::from_millis(400);
const LAG_WINDOW: Duration = Duration::from_secs(10);
const OFI_WINDOW: Duration = Duration::from_secs(60);

pub(crate) enum UiEvent {
    Quit,
    Resize,
    Key(UiKey),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum UiKey {
    /// Cycle the ladder price grouping.
    Group,
    /// Freeze or resume the display. The book keeps updating underneath.
    Pause,
    /// Focus the next symbol.
    NextSymbol,
}

pub struct Screen {
    last_paint: Option<Instant>,
    raw_mode: bool,
    terminal: Option<DefaultTerminal>,
    cards: Vec<BookCard>,
    events: Option<mpsc::UnboundedReceiver<UiEvent>>,
    stop_input: Arc<AtomicBool>,
    input: Option<JoinHandle<()>>,
    group: usize,
    focus: usize,
    frozen: Option<(DateTime<Utc>, Instant)>,
}

/// Direction of a ladder level's last size change.
#[derive(Clone, Copy)]
struct Flash {
    at: Instant,
    up: bool,
}

struct BookCard {
    symbol: String,
    updates: UpdateRate,
    per_sec: u64,
    asks: Vec<(Decimal, Decimal)>,
    bids: Vec<(Decimal, Decimal)>,
    analysis: Option<Analysis>,
    timestamp: Option<DateTime<Utc>>,
    ask_ladder: Vec<Level>,
    bid_ladder: Vec<Level>,
    flashes: HashMap<Decimal, Flash>,
    lags: VecDeque<(Instant, i64)>,
    ofi: VecDeque<(Instant, Decimal)>,
    prev_touch: Option<Touch>,
}

impl BookCard {
    fn new(symbol: &str) -> BookCard {
        BookCard {
            symbol: symbol.to_string(),
            updates: UpdateRate::default(),
            per_sec: 0,
            asks: Vec::new(),
            bids: Vec::new(),
            analysis: None,
            timestamp: None,
            ask_ladder: Vec::new(),
            bid_ladder: Vec::new(),
            flashes: HashMap::new(),
            lags: VecDeque::new(),
            ofi: VecDeque::new(),
            prev_touch: None,
        }
    }

    fn record(&mut self, now: Instant) {
        self.per_sec = self.updates.record(now);
    }

    /// Store a freshly applied book and update every time-based series.
    #[allow(clippy::too_many_arguments)]
    fn ingest(
        &mut self,
        asks: &[(Decimal, Decimal)],
        bids: &[(Decimal, Decimal)],
        analysis: Analysis,
        timestamp: DateTime<Utc>,
        group: u32,
        now: Instant,
        now_utc: DateTime<Utc>,
    ) {
        self.asks.clear();
        self.asks.extend_from_slice(asks);
        self.bids.clear();
        self.bids.extend_from_slice(bids);

        self.lags.push_back((now, age_ms(now_utc - timestamp)));
        while self
            .lags
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > LAG_WINDOW)
        {
            self.lags.pop_front();
        }

        if let Some(prev) = &self.prev_touch {
            let step = metrics::ofi_step(prev, &analysis.touch);
            if !step.is_zero() {
                self.ofi.push_back((now, step));
            }
        }
        while self
            .ofi
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) > OFI_WINDOW)
        {
            self.ofi.pop_front();
        }
        self.prev_touch = Some(analysis.touch);

        self.timestamp = Some(timestamp);
        let tick = analysis.tick;
        self.analysis = Some(analysis);
        let old_asks = std::mem::take(&mut self.ask_ladder);
        let old_bids = std::mem::take(&mut self.bid_ladder);
        self.rebuild_ladders(group, tick);
        self.note_flashes(&old_asks, &old_bids, now);
    }

    fn rebuild_ladders(&mut self, group: u32, tick: Decimal) {
        let step = (group > 1).then(|| tick * Decimal::from(group));
        self.ask_ladder = metrics::ladder(&self.asks, true, step, LADDER_ROWS);
        self.bid_ladder = metrics::ladder(&self.bids, false, step, LADDER_ROWS);
    }

    fn note_flashes(&mut self, old_asks: &[Level], old_bids: &[Level], now: Instant) {
        self.flashes
            .retain(|_, flash| now.duration_since(flash.at) < FLASH_FOR);
        if old_asks.is_empty() && old_bids.is_empty() {
            return;
        }
        let previous: HashMap<Decimal, Decimal> = old_asks
            .iter()
            .chain(old_bids)
            .map(|level| (level.price, level.qty))
            .collect();
        for level in self.ask_ladder.iter().chain(&self.bid_ladder) {
            let before = previous.get(&level.price).copied().unwrap_or(Decimal::ZERO);
            if before != level.qty {
                self.flashes.insert(
                    level.price,
                    Flash {
                        at: now,
                        up: level.qty > before,
                    },
                );
            }
        }
    }

    fn set_group(&mut self, group: u32) {
        let Some(tick) = self.analysis.as_ref().map(|a| a.tick) else {
            return;
        };
        self.flashes.clear();
        self.rebuild_ladders(group, tick);
    }

    /// Sum of order flow imbalance over the trailing `window`.
    fn ofi_over(&self, window: Duration, now: Instant) -> Decimal {
        self.ofi
            .iter()
            .filter(|(at, _)| now.duration_since(*at) <= window)
            .map(|(_, step)| *step)
            .sum()
    }

    /// Lag percentiles over the trailing window, in ms: (p50, p99).
    fn lag_percentiles(&self) -> Option<(i64, i64)> {
        let mut values: Vec<i64> = self.lags.iter().map(|(_, ms)| *ms).collect();
        values.sort_unstable();
        Some((
            metrics::percentile(&values, 50)?,
            metrics::percentile(&values, 99)?,
        ))
    }
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
            group: 0,
            focus: 0,
            frozen: None,
        })
    }

    pub(crate) fn take_events(&mut self) -> Option<mpsc::UnboundedReceiver<UiEvent>> {
        self.events.take()
    }

    /// Count this applied frame in the symbol's one-second rate window.
    ///
    /// Returns whether the throttle allows a paint. The first attempt is immediate.
    /// Later attempts wait 100ms. A paint that [`draw`] skips does not move the stamp,
    /// so the next frame is due again. While the display is paused no paint is due.
    pub fn note_update(&mut self, symbol: &str) -> bool {
        self.card_mut(symbol).record(Instant::now());
        self.frozen.is_none() && self.paint_due()
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
        if self.frozen.is_some() {
            return Ok(());
        }
        let Some(analysis) = Analysis::from_book(asks, bids) else {
            return Ok(());
        };
        let group = GROUPS[self.group];
        self.card_mut(symbol).ingest(
            asks,
            bids,
            analysis,
            timestamp,
            group,
            Instant::now(),
            Utc::now(),
        );
        self.paint()?;
        self.last_paint = Some(Instant::now());
        Ok(())
    }

    pub(crate) fn redraw(&mut self) -> Result<(), String> {
        if self.cards.iter().all(|card| card.analysis.is_none()) {
            return Ok(());
        }
        self.paint()
    }

    pub(crate) fn on_key(&mut self, key: UiKey) -> Result<(), String> {
        match key {
            UiKey::Group => {
                self.group = (self.group + 1) % GROUPS.len();
                let group = GROUPS[self.group];
                for card in &mut self.cards {
                    card.set_group(group);
                }
            }
            UiKey::Pause => {
                self.frozen = match self.frozen {
                    Some(_) => None,
                    None => Some((Utc::now(), Instant::now())),
                };
            }
            UiKey::NextSymbol => {
                if !self.cards.is_empty() {
                    self.focus = (self.focus + 1) % self.cards.len();
                }
            }
        }
        self.redraw()
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
        let (now_utc, now) = self.frozen.unwrap_or_else(|| (Utc::now(), Instant::now()));
        let ctx = render::Ctx {
            now_utc,
            now,
            group: GROUPS[self.group],
            paused: self.frozen.is_some(),
            focus: self.focus,
        };
        let cards = &self.cards;
        self.terminal
            .as_mut()
            .expect("ratatui terminal")
            .draw(|frame| render::render(frame, cards, &ctx))
            .map_err(|err| err.to_string())?;
        Ok(())
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
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    if let Some(key) = map_key(&key) {
                        let _ = sender.send(UiEvent::Key(key));
                    }
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

fn map_key(key: &KeyEvent) -> Option<UiKey> {
    match key.code {
        KeyCode::Char('g') | KeyCode::Char('G') => Some(UiKey::Group),
        KeyCode::Char('p') | KeyCode::Char('P') => Some(UiKey::Pause),
        KeyCode::Tab => Some(UiKey::NextSymbol),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn d(value: &str) -> Decimal {
        value.parse().unwrap()
    }

    fn card_with(
        asks: &[(&str, &str)],
        bids: &[(&str, &str)],
    ) -> (BookCard, Vec<(Decimal, Decimal)>, Vec<(Decimal, Decimal)>) {
        let asks: Vec<_> = asks.iter().map(|(p, q)| (d(p), d(q))).collect();
        let bids: Vec<_> = bids.iter().map(|(p, q)| (d(p), d(q))).collect();
        (BookCard::new("BTC/USD"), asks, bids)
    }

    fn ingest(card: &mut BookCard, asks: &[(Decimal, Decimal)], bids: &[(Decimal, Decimal)]) {
        let analysis = Analysis::from_book(asks, bids).unwrap();
        let now_utc = Utc::now();
        card.ingest(asks, bids, analysis, now_utc, 1, Instant::now(), now_utc);
    }

    #[test]
    fn size_changes_flash_in_the_right_direction() {
        let (mut card, asks, bids) = card_with(&[("101.0", "2")], &[("100.0", "3")]);
        ingest(&mut card, &asks, &bids);
        assert!(card.flashes.is_empty(), "first book has nothing to compare");

        let grown = vec![(d("100.0"), d("5"))];
        let shrunk = vec![(d("101.0"), d("1"))];
        ingest(&mut card, &shrunk, &grown);
        assert!(card.flashes[&d("100.0")].up);
        assert!(!card.flashes[&d("101.0")].up);
    }

    #[test]
    fn ofi_accumulates_in_window() {
        let (mut card, asks, bids) = card_with(&[("101.0", "2")], &[("100.0", "3")]);
        ingest(&mut card, &asks, &bids);
        let bigger_bid = vec![(d("100.0"), d("7"))];
        ingest(&mut card, &asks, &bigger_bid);
        assert_eq!(
            card.ofi_over(Duration::from_secs(1), Instant::now()),
            d("4")
        );
    }

    #[test]
    fn grouping_rebuilds_ladders_without_flashes() {
        let (mut card, asks, bids) = card_with(
            &[("100.1", "1"), ("100.2", "1")],
            &[("100.0", "1"), ("99.9", "1")],
        );
        ingest(&mut card, &asks, &bids);
        assert_eq!(card.ask_ladder.len(), 2);
        card.set_group(5);
        assert_eq!(card.ask_ladder.len(), 1);
        assert_eq!(card.ask_ladder[0].price, d("100.5"));
        assert!(card.flashes.is_empty());
    }

    #[test]
    fn lag_percentiles_need_samples() {
        let card = BookCard::new("X");
        assert!(card.lag_percentiles().is_none());
    }
}
