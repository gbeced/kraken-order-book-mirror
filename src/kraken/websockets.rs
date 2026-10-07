use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tungstenite::client::IntoClientRequest;

use super::utils;

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    error: Vec<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct TokenResult {
    expires: u64,
    token: String,
}

// https://docs.kraken.com/exchange/guides/websockets/authentication
async fn get_websocket_auth_token(
    api_key: &str,
    api_secret: &str,
) -> Result<TokenResult, Box<dyn Error>> {
    let path = "/0/private/GetWebSocketsToken";
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let body = format!("nonce={nonce}");
    let signature = utils::get_signature(path, nonce, &body, api_secret)?;

    let url = format!("https://api.kraken.com{path}");
    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("API-Key", api_key)
        .header("API-Sign", signature)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    let body = response.text().await?;
    let parsed: ApiResponse<TokenResult> = serde_json::from_str(&body)?;
    let Some(token_result) = parsed.result else {
        return Err(parsed.error.join("; ").into());
    };

    Ok(token_result)
}

fn is_channel_level3(msg: &str) -> bool {
    // TODO: Fix this.
    // Error prone, but at this level we just wan't to forward messages ASAP.
    const PREFIX: &str = r#"{"channel":"level3","type":""#;
    msg.starts_with(PREFIX)
}

pub async fn connect_and_dispatch_level3(
    api_key: &str, api_secret: &str, channels: &[&str], depth: usize,
    sender: mpsc::UnboundedSender<String>
) -> Result<(), Box<dyn Error>> {
    let token = get_websocket_auth_token(api_key, api_secret).await?;

    // Connect.
    let request = "wss://ws-l3.kraken.com/v2".into_client_request()?;
    let (stream, _response) = connect_async(request).await?;
    let (mut write, mut read) = stream.split();

    // Subscribe.
    let subscribe_msg = json!({
        "method": "subscribe",
        "params": {
            "channel": "level3",
            "depth": depth,
            "symbol": channels,
            "snapshot": true,
            "token": token.token
        }
    });
    write
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await?;

    // Read loop.
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // println!("{text}");
                if is_channel_level3(text.as_str()) {
                    sender.send(text.to_string())?;
                }
            }
            Ok(Message::Close(frame)) => {
                return Err(format!("Connection closed: {frame:?}").into());
            }
            Ok(_) => {}
            Err(error) => {
                return Err(error.into());
            }
        }
    }

    Ok(())
}
