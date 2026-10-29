use super::sign;
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    error: Vec<String>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
pub struct TokenResult {
    // expires: u64,
    pub token: String,
}

// https://docs.kraken.com/exchange/guides/websockets/authentication
pub async fn get_websocket_auth_token(
    api_key: &str,
    api_secret: &str,
) -> Result<TokenResult, Box<dyn Error>> {
    let path = "/0/private/GetWebSocketsToken";
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let body = format!("nonce={nonce}");
    let signature = sign::get_signature(path, nonce, &body, api_secret)?;

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

pub struct PairInfo {
    pub base_precision: u8,
    pub quote_precision: u8,
}

#[derive(Debug, Deserialize)]
struct AssetPair {
    pair_decimals: u8,
    lot_decimals: u8,
}

pub async fn get_pair_info(pair: &str) -> Result<PairInfo, Box<dyn Error>> {
    // https://docs.kraken.com/api-reference/market-data/get-tradable-asset-pairs
    let path = "/0/public/AssetPairs";
    let url = format!("https://api.kraken.com{path}");
    let params = [("pair", pair)];
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        // .header("Content-Type", "application/x-www-form-urlencoded")
        .query(&params)
        .send()
        .await?
        .error_for_status()?;
    let body = response.text().await?;
    let parsed: ApiResponse<HashMap<String, AssetPair>> = serde_json::from_str(&body)?;
    let Some(asset_pairs) = parsed.result else {
        return Err(parsed.error.join("; ").into());
    };
    let Some((_, asset_pair)) = asset_pairs.iter().next() else {
        return Err("No pairs returned".into());
    };

    Ok(PairInfo {
        base_precision: asset_pair.lot_decimals,
        quote_precision: asset_pair.pair_decimals,
    })
}

// pub async fn get_pair_info(pair: &str) -> Result<PairInfo, Box<dyn Error>> {
//     let ret = match pair {
//         "BTC/USD" => PairInfo {
//             base_precision: 8,
//             quote_precision: 1,
//         },
//         "ETH/USD" => PairInfo {
//             base_precision: 8,
//             quote_precision: 2,
//         },
//         _ => {
//             assert!(false);
//             PairInfo {
//                 base_precision: 8,
//                 quote_precision: 1,
//             }
//         }
//     };

//     Ok(ret)
// }
