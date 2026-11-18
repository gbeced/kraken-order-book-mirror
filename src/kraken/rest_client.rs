use super::error::Error;
use super::sign;
use serde::Deserialize;
use std::collections::HashMap;
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
) -> Result<TokenResult, Error> {
    let path = "/0/private/GetWebSocketsToken";
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Other(e.to_string()))?
        .as_millis();
    let body = format!("nonce={nonce}");
    let signature = sign::get_signature(path, nonce, &body, api_secret)
        .map_err(|e| Error::Sign(e.to_string()))?;

    let url = format!("https://api.kraken.com{path}");
    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("API-Key", api_key)
        .header("API-Sign", signature)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| -> Error { Error::Network(e.to_string()) })?
        .error_for_status()
        .map_err(|e| -> Error { Error::Other(e.to_string()) })?;
    let body = response
        .text()
        .await
        .map_err(|e| -> Error { Error::Network(e.to_string()) })?;
    let parsed: ApiResponse<TokenResult> =
        serde_json::from_str(&body).map_err(|e| -> Error { Error::Parse(e.to_string()) })?;
    let Some(token_result) = parsed.result else {
        return Err(Error::Other(parsed.error.join("; ")));
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

pub async fn get_pair_info(pair: &str) -> Result<PairInfo, Error> {
    // https://docs.kraken.com/api-reference/market-data/get-tradable-asset-pairs
    let path = "/0/public/AssetPairs";
    let url = format!("https://api.kraken.com{path}");
    let params = [("pair", pair)];
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .query(&params)
        .send()
        .await
        .map_err(|e| -> Error { Error::Network(e.to_string()) })?
        .error_for_status()
        .map_err(|e| -> Error { Error::Other(e.to_string()) })?;

    let body = response
        .text()
        .await
        .map_err(|e| -> Error { Error::Network(e.to_string()) })?;

    let parsed: ApiResponse<HashMap<String, AssetPair>> =
        serde_json::from_str(&body).map_err(|e| -> Error { Error::Parse(e.to_string()) })?;

    let Some(asset_pairs) = parsed.result else {
        let error = match parsed
            .error
            .iter()
            .find(|v| *v == "EQuery:Unknown asset pair")
        {
            Some(_) => Error::InvalidParameter(parsed.error.join("; ")),
            _ => Error::Other(parsed.error.join("; ")),
        };
        return Err(error);
    };
    let Some((_, asset_pair)) = asset_pairs.iter().next() else {
        return Err(Error::Other("No pairs returned".into()));
    };

    Ok(PairInfo {
        base_precision: asset_pair.lot_decimals,
        quote_precision: asset_pair.pair_decimals,
    })
}
