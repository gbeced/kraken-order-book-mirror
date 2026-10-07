mod kraken;

use std::env;
use std::error::Error;
use rust_decimal::Decimal;
use tokio::sync::mpsc;

async fn obook_reader(sender: mpsc::UnboundedSender<String>) -> Result<(), String> {
    let api_key = env::var("KRAKEN_API_KEY").map_err(|_| "KRAKEN_API_KEY must be set".to_string())?;
    let api_secret = env::var("KRAKEN_API_SECRET").map_err(|_| "KRAKEN_API_SECRET must be set".to_string())?;
    let channels = ["BTC/USD"];

    kraken::websockets::connect_and_dispatch_level3(&api_key, &api_secret, &channels, 100, sender)
    .await
    .map_err(|e| e.to_string())
}

async fn obook_updater(mut receiver: mpsc::UnboundedReceiver<String>) -> Result<(), String> {
    let mut mirror = kraken::level3::OrderBookMirror::new(100);

    while let Some(msg) = receiver.recv().await {
        match mirror.on_message(&msg) {
            Ok(_) => {
                if let Some(snapshot) = mirror.get_order_book() {
                    let v_asks: Decimal = snapshot.asks.iter().map(|o| o.order_qty).sum();
                    let v_bids: Decimal = snapshot.bids.iter().map(|o| o.order_qty).sum();
                    let obi = (v_bids - v_asks) / (v_bids + v_asks);

                    println!(
                        "{} {}: ask {} bid {} obi {}",
                        snapshot.timestamp,
                        snapshot.symbol,
                        snapshot.asks[0].limit_price,
                        snapshot.bids[0].limit_price,
                        obi,
                    );
                }
            },
            Err(e) => {
                return Err(e.to_string());
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {

    let (sender, receiver) = mpsc::unbounded_channel::<String>();
    let (reader_res, updater_res) = tokio::join!(
        tokio::spawn(obook_reader(sender)),
        tokio::spawn(obook_updater(receiver)),
    );
    if let Err(e) = reader_res? {
        eprintln!("Error in reader: {e}");
    }
    if let Err(e) = updater_res? {
        eprintln!("Error in updater: {e}");
    }

    Ok(())
}
