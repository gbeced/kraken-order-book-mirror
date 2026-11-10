use crate::config;
use crate::kraken::level3::{mirror, order_book, websockets};
use crate::kraken::rest_client;
use crate::tui::{self, UiEvent};
use std::error::Error;
use std::future::pending;
use tokio::sync::mpsc;

async fn obook_reader(
    api_key: String,
    api_secret: String,
    channel: String,
    depth: usize,
    sender: mpsc::UnboundedSender<websockets::Utf8Bytes>,
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
    mut receiver: mpsc::UnboundedReceiver<websockets::Utf8Bytes>,
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
                    UiEvent::Key(key) => screen.on_key(key)?,
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

    let (sender, receiver) = mpsc::unbounded_channel::<websockets::Utf8Bytes>();
    let mut producer = tokio::spawn(obook_reader(
        api_key,
        api_secret,
        pair.clone(),
        depth,
        sender,
    ));
    let mut consumer = tokio::spawn(obook_updater(
        pair,
        depth,
        pair_info.quote_precision,
        pair_info.base_precision,
        receiver,
    ));

    let res = tokio::select! {
        res = & mut consumer => {
            producer.abort();
            let _ = producer.await;
            res
        },
        res = & mut producer => {
            consumer.abort();
            let _ = consumer.await;
            res
        },
    }?;
    res.map_err(|e| e.into())
}
