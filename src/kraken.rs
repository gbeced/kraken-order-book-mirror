use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac, KeyInit};
use serde::{Deserialize};
use serde_json::json;
use sha2::{Sha256, Sha512, Digest};
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_tungstenite::{connect_async};
use tungstenite::client::{IntoClientRequest};
use tungstenite::{Message};

type HmacSha512 = Hmac<Sha512>;


#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    error: Vec<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct TokenResult {
    pub expires: u64,
    pub token: String,
}

fn get_signature(url_path: &str, nonce: u128, api_secret: &str) -> Result<String, Box<dyn Error>> {
    let encoded_data = format!("{nonce}nonce={nonce}");
    let digest = Sha256::digest(&encoded_data);
    let mut message: Vec<u8> = Vec::with_capacity(encoded_data.bytes().len() + digest.len());
    message.extend_from_slice(url_path.as_bytes());
    message.extend_from_slice(&digest);

    let secret_bytes = STANDARD.decode(api_secret)?;
    let mut mac = HmacSha512::new_from_slice(&secret_bytes)?;
    mac.update(&message);

    let signature= STANDARD.encode(mac.finalize().into_bytes());
    Ok(signature)
}

async fn get_websocket_auth_token(api_key: &str, api_secret: &str) -> Result<TokenResult, Box<dyn Error>> {
    let path = "/0/private/GetWebSocketsToken";
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let signature = get_signature(path, nonce, api_secret)?;

    let url = format!("https://api.kraken.com{path}");
    // TODO: Use https://docs.rs/serde_qs/latest/serde_qs/ for encoding.
    let data = format!("nonce={nonce}");
    let client = reqwest::Client::new();
    let response = client.post(url)
        .header("API-Key", api_key)
        .header("API-Sign", signature)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(data)
        .send().await?
        .error_for_status()?
        ;
    let body = response.text().await?;
    let parsed: ApiResponse<TokenResult> = serde_json::from_str(&body)?;
    let Some(token_result) = parsed.result else {
        return Err(parsed.error.join("; ").into())
    };

    Ok(token_result)
}


pub async fn connect_and_dispatch(api_key: &str, api_secret: &str) -> Result<(), Box<dyn std::error::Error>> {
    let token = get_websocket_auth_token(&api_key, &api_secret).await?;

    // Connect.
    let request = "wss://ws-l3.kraken.com/v2".into_client_request()?;
    let (stream, _response) = connect_async(request).await?;
    let (mut write, mut read) = stream.split();

    // Subscribe.
    let subscribe_msg = json!({
        "method": "subscribe",
        "params": {
            "channel": "level3",
            "depth": 10,
            "symbol": ["BTC/USD"],
            "snapshot": true,
            "token": token.token
        }
    });
    write.send(Message::Text(subscribe_msg.to_string().into())).await?;

    // Read loop.
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                println!("{text}");
            }
            Ok(Message::Ping(data)) => {
                if let Err(error) = write.send(Message::Pong(data)).await {
                    eprintln!("Failed to pong: {error}");
                }
            }
            Ok(Message::Close(frame)) => {
                eprintln!("Connection closed: {frame:?}");
                break;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("WebSocket error: {error}");
                break;
            }
        }
    }

    Ok(())
}