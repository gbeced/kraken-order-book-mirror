mod kraken;

use std::env;
use std::error::Error;


#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let api_key = env::var("KRAKEN_API_KEY").expect("KRAKEN_API_KEY must be set");
    let api_secret = env::var("KRAKEN_API_SECRET").expect("KRAKEN_API_SECRET must be set");

    let handle = tokio::spawn(async move {
        let channels = ["BTC/USD"];
        kraken::websockets::connect_and_dispatch(&api_key, &api_secret, &channels, 10)
        .await
        .map_err(|e| e.to_string())
    });

    handle.await??;
    Ok(())
}
