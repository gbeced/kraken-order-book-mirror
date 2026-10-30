use crate::common::json_utils;
use crate::kraken::rest_client;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::error::Error;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tungstenite::client::IntoClientRequest;

// Faster, but error prone.
// pub fn is_channel_level3(msg: &str) -> bool {
//     const PREFIX: &str = r#"{"channel":"level3","type":""#;
//     msg.starts_with(PREFIX)
// }

// Top-level "channel" must be the plain string "level3".
// A byte scan stops at that key, so a depth-100 snapshot is not parsed twice.
// Nested "channel" (subscribe acks) does not match. Key order and whitespace do.
fn is_channel_level3(msg: &str) -> bool {
    json_utils::JsonScanner::new(msg.as_bytes()).top_level_plain_string(b"channel")
        == Some("level3")
}

fn extract_error<'a>(msg: &'a str) -> Option<&'a str> {
    json_utils::JsonScanner::new(msg.as_bytes()).top_level_plain_string(b"error")
}

pub async fn connect_and_dispatch_level3(
    api_key: &str,
    api_secret: &str,
    channel: &str,
    depth: usize,
    sender: mpsc::UnboundedSender<String>,
) -> Result<(), Box<dyn Error>> {
    let token = rest_client::get_websocket_auth_token(api_key, api_secret).await?;

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
            "symbol": [channel],
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
                if is_channel_level3(text.as_str()) {
                    sender.send(text.to_string())?;
                }
                else if let Some(error) = extract_error(text.as_str()) {
                    return Err(error.into());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_level3_frames() {
        let samples = [
            r#"{"channel":"level3","type":"snapshot","data":[]}"#,
            r#"{"channel":"level3","type":"update","data":[]}"#,
            "{\n  \"channel\" : \"level3\"\n}",
            r#"{"type":"update","channel":"level3","data":[]}"#,
            r#"{"data":[{"note":"}"}],"channel":"level3"}"#,
            r#"{"note":"\"channel\":\"level3\"","channel":"level3"}"#,
            r#"{"result":{"channel":"heartbeat"},"channel":"level3"}"#,
            // Stop at the channel key. The rest of the frame is not scanned.
            r#"{"channel":"level3","data":[}"#,
        ];
        for sample in samples {
            assert!(is_channel_level3(sample), "{sample}");
        }
    }

    #[test]
    fn rejects_other_frames() {
        let samples = [
            "",
            "[]",
            "{",
            "{}",
            r#"{"channel":"#,
            r#"{"channel":"heartbeat"}"#,
            r#"{"channel":"status","type":"update","data":[]}"#,
            r#"{"channel":"level3 "}"#,
            r#"{"channel":"\u006cevel3"}"#,
            r#"{"data":[{"channel":"level3"}]}"#,
            r#"{"note":"\"channel\":\"level3\""}"#,
            r#"{"method":"subscribe","result":{"channel":"level3","symbol":"BTC/USD"},"success":true}"#,
            r#"{"method":"unsubscribe","result":{"channel":"level3","symbol":"BTC/USD"},"success":true}"#,
            r#"{"method":"subscribe","params":{"channel":"level3","symbol":["BTC/USD"]},"success":true}"#,
        ];
        for sample in samples {
            assert!(!is_channel_level3(sample), "{sample}");
        }
    }

    #[test]
    fn error_frames() {
        let positive_samples = [
            r#"{"error":"Subscription depth not supported","method":"subscribe"}"#,
        ];
        for sample in positive_samples {
            assert_eq!(extract_error(sample).unwrap(), "Subscription depth not supported");
        }

        let negative_samples = [
            r#"{"channel":"status","type":"update","data":[]}"#,
        ];
        for sample in negative_samples {
            assert!(extract_error(sample).is_none());
        }

    }


}
