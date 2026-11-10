use crate::config;
use crate::kraken::level3::websockets;
use std::error::Error;
use tokio::sync::mpsc;

#[tokio::main]
pub async fn run(pair: String, depth: usize) -> Result<(), Box<dyn Error>> {
    let (api_key, api_secret) = config::get_credentials()?;

    let (sender, mut receiver) = mpsc::unbounded_channel::<websockets::Utf8Bytes>();
    let producer = tokio::spawn(async move {
        websockets::connect_and_dispatch_level3(&api_key, &api_secret, &pair, depth, sender)
            .await
            .map_err(|e| e.to_string())
    });
    let consumer = tokio::spawn(async move {
        while let Some(msg) = receiver.recv().await {
            println!("{}", msg);
        }
    });

    let res = producer.await?;
    let _ = consumer.await;
    res.map_err(|e| e.into())
}
