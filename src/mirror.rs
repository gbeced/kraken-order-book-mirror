use chrono::Utc;
use rust_decimal::Decimal;
use std::env;
use std::error::Error;
use tokio::sync::mpsc;

use crate::kraken;

async fn obook_reader(
    api_key: String,
    api_secret: String,
    channel: &str,
    max_depth: usize,
    sender: mpsc::UnboundedSender<String>,
) -> Result<(), String> {
    kraken::websockets::connect_and_dispatch_level3(
        &api_key,
        &api_secret,
        channel,
        max_depth,
        sender,
    )
    .await
    .map_err(|e| e.to_string())
}

async fn obook_updater(
    channel: &str,
    max_depth: usize,
    mut receiver: mpsc::UnboundedReceiver<String>,
) -> Result<(), String> {
    let mut mirror = kraken::level3::OrderBookMirror::new(max_depth);

    while let Some(msg) = receiver.recv().await {
        match mirror.on_message(&msg) {
            Ok(_) => {
                let ob = mirror.get_order_book().expect("No order book");
                let v_asks: Decimal = ob.asks.iter().map(|o| o.order_qty).sum();
                let v_bids: Decimal = ob.bids.iter().map(|o| o.order_qty).sum();
                let obi = (v_bids - v_asks) / (v_bids + v_asks);
                let age = Utc::now() - ob.timestamp;

                println!(
                    "{} {}: ask {} bid {} obi {} age {}",
                    ob.timestamp,
                    channel,
                    ob.asks[0].limit_price,
                    ob.bids[0].limit_price,
                    obi.round_dp(4),
                    age.as_seconds_f32()
                );
            }
            Err(e) => {
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}

#[tokio::main]
pub async fn run() -> Result<(), Box<dyn Error>> {
    let api_key =
        env::var("KRAKEN_API_KEY").map_err(|_| "KRAKEN_API_KEY must be set".to_string())?;
    let api_secret =
        env::var("KRAKEN_API_SECRET").map_err(|_| "KRAKEN_API_SECRET must be set".to_string())?;
    let channel = "BTC/USD";
    let max_depth = 100;

    let (sender, receiver) = mpsc::unbounded_channel::<String>();
    let (reader_res, updater_res) = tokio::join!(
        tokio::spawn(obook_reader(
            api_key, api_secret, channel, max_depth, sender
        )),
        tokio::spawn(obook_updater(channel, max_depth, receiver)),
    );

    if let Err(e) = reader_res? {
        eprintln!("Error in reader: {e}");
    }
    if let Err(e) = updater_res? {
        eprintln!("Error in updater: {e}");
    }

    Ok(())
}
