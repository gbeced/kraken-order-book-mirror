mod kraken;

use std::env;
use std::error::Error;
use tokio::sync::mpsc;

async fn obook_reader(sender: mpsc::UnboundedSender<String>) -> Result<(), String> {
    let api_key = env::var("KRAKEN_API_KEY").map_err(|_| "KRAKEN_API_KEY must be set".to_string())?;
    let api_secret = env::var("KRAKEN_API_SECRET").map_err(|_| "KRAKEN_API_SECRET must be set".to_string())?;
    let channels = ["BTC/USD"];

    kraken::websockets::connect_and_dispatch_level3(&api_key, &api_secret, &channels, 10, sender)
    .await
    .map_err(|e| e.to_string())
}

async fn obook_updater(mut receiver: mpsc::UnboundedReceiver<String>) -> Result<(), String> {
    while let Some(msg) = receiver.recv().await {
        println!("{msg}")
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
