use super::level3::{mirror, order_book, websockets};
use super::tui::{self, UiEvent};
use std::env;
use std::error::Error;
use std::future::pending;
use tokio::sync::mpsc;
use tokio::task::JoinError;

async fn obook_reader(
    api_key: String,
    api_secret: String,
    channel: &str,
    max_depth: usize,
    sender: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    websockets::connect_and_dispatch_level3(&api_key, &api_secret, channel, max_depth, sender)
        .await
        .map_err(|e| e.to_string())
}

async fn obook_updater(
    channel: &str,
    max_depth: usize,
    price_scale: u8,
    qty_scale: u8,
    mut receiver: mpsc::UnboundedReceiver<String>,
) -> Result<(), String> {
    let mut mirror = mirror::OrderBookMirror::new(max_depth, price_scale, qty_scale);
    let mut screen = tui::Screen::new()?;
    let mut ui_events = screen.take_events();

    loop {
        tokio::select! {
            biased;
            event = recv_ui(&mut ui_events) => {
                match event {
                    UiEvent::Quit => break,
                    UiEvent::Resize => screen.redraw()?,
                }
            }
            incoming = receiver.recv() => {
                let Some(msg) = incoming else {
                    break;
                };
                match mirror.on_message(&msg) {
                    Ok(_) => {
                        if !screen.note_update(channel) {
                            continue;
                        }
                        let ob = mirror.get_order_book().expect("No order book");
                        let asks = to_decimal_orders(ob, &ob.asks);
                        let bids = to_decimal_orders(ob, &ob.bids);
                        screen.draw(channel, &asks, &bids, ob.timestamp)?;
                    }
                    Err(e) => {
                        return Err(e.to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

async fn recv_ui(events: &mut Option<mpsc::UnboundedReceiver<UiEvent>>) -> UiEvent {
    match events {
        Some(events) => events.recv().await.unwrap_or(UiEvent::Quit),
        None => pending().await,
    }
}

fn to_decimal_orders(
    ob: &order_book::OrderBook,
    orders: &order_book::Orders,
) -> Vec<(rust_decimal::Decimal, rust_decimal::Decimal)> {
    orders
        .iter()
        .map(|order| {
            (
                ob.price_to_decimal(order.limit_price),
                ob.qty_to_decimal(order.order_qty),
            )
        })
        .collect()
}

#[tokio::main]
pub async fn run() -> Result<(), Box<dyn Error>> {
    let api_key =
        env::var("KRAKEN_API_KEY").map_err(|_| "KRAKEN_API_KEY must be set".to_string())?;
    let api_secret =
        env::var("KRAKEN_API_SECRET").map_err(|_| "KRAKEN_API_SECRET must be set".to_string())?;
    let (channel, price_scale, qty_scale) = ("BTC/USD", 1, 8);
    // let (channel, price_scale, qty_scale) = ("ETH/USD", 2, 8);
    let max_depth = 1000;

    let (sender, receiver) = mpsc::unbounded_channel::<String>();
    let mut reader = tokio::spawn(obook_reader(
        api_key, api_secret, channel, max_depth, sender,
    ));
    let mut updater = tokio::spawn(obook_updater(
        channel,
        max_depth,
        price_scale,
        qty_scale,
        receiver,
    ));

    // `Screen` restores the terminal when the updater task returns. Abort the reader
    // so quit and apply errors exit instead of leaving the socket open.
    let (reader_res, updater_res) = tokio::select! {
        res = &mut reader => {
            let updater_res = updater.await;
            (res, updater_res)
        }
        res = &mut updater => {
            reader.abort();
            let reader_res = reader.await;
            (reader_res, res)
        }
    };

    if let Err(e) = task_result(reader_res)? {
        eprintln!("Error in reader: {e}");
    }
    if let Err(e) = task_result(updater_res)? {
        eprintln!("Error in updater: {e}");
    }

    Ok(())
}

fn task_result(
    joined: Result<Result<(), String>, JoinError>,
) -> Result<Result<(), String>, Box<dyn Error>> {
    match joined {
        Ok(result) => Ok(result),
        Err(err) if err.is_cancelled() => Ok(Ok(())),
        Err(err) => Err(err.into()),
    }
}
