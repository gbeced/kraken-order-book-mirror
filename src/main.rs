mod kraken;

use std::env;
use tokio;


#[tokio::main]
async fn main() {
    let api_key = env::var("KRAKEN_API_KEY").expect("KRAKEN_API_KEY must be set");
    let api_secret = env::var("KRAKEN_API_SECRET").expect("KRAKEN_API_SECRET must be set");

    kraken::connect_and_dispatch(&api_key, &api_secret).await;
}
