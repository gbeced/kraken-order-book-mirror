use crate::config;
use crate::kraken::level3::{mirror, order_book, websockets};
use crate::kraken::rest_client;
use crate::tui::{self, UiEvent};
use std::error::Error;
use std::future::pending;
use tokio::sync::mpsc;
use tokio::task::JoinError;

async fn obook_reader(
    api_key: String,
    api_secret: String,
    channel: String,
    depth: usize,
    sender: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    websockets::connect_and_dispatch_level3(&api_key, &api_secret, &channel, depth, sender)
        .await
        .map_err(|e| e.to_string())
}

async fn obook_updater(
    channel: String,
    depth: usize,
    price_scale: u8,
    qty_scale: u8,
    mut receiver: mpsc::UnboundedReceiver<String>,
) -> Result<(), String> {
    let mut mirror = mirror::OrderBookMirror::new(depth, price_scale, qty_scale);
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
                        if !screen.note_update(&channel) {
                            continue;
                        }
                        let ob = mirror.get_order_book().expect("No order book");
                        let asks = to_decimal_orders(ob, &ob.asks);
                        let bids = to_decimal_orders(ob, &ob.bids);
                        screen.draw(&channel, &asks, &bids, ob.timestamp)?;
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
pub async fn run(pair: String, depth: usize) -> Result<(), Box<dyn Error>> {
    let (api_key, api_secret) = config::get_credentials()?;
    let pair_info = rest_client::get_pair_info(&pair).await?;

    let (sender, receiver) = mpsc::unbounded_channel::<String>();
    let mut consumer = tokio::spawn(obook_reader(
        api_key,
        api_secret,
        pair.clone(),
        depth,
        sender,
    ));
    let mut producer = tokio::spawn(obook_updater(
        pair,
        depth,
        pair_info.quote_precision,
        pair_info.base_precision,
        receiver,
    ));

    // `Screen` restores the terminal when the updater task returns. Abort the reader
    // so quit and apply errors exit instead of leaving the socket open.
    let (consumer_res, producer_res) = tokio::select! {
        res = &mut consumer => {
            let updater_res = producer.await;
            (res, updater_res)
        }
        res = &mut producer => {
            consumer.abort();
            let reader_res = consumer.await;
            (reader_res, res)
        }
    };

    if let Err(e) = task_result(consumer_res)? {
        eprintln!("Error in reader: {e}");
    }
    if let Err(e) = task_result(producer_res)? {
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
