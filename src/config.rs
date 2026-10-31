use std::env;
use std::error::Error;

pub fn get_credentials() -> Result<(String, String), Box<dyn Error>> {
    let api_key =
        env::var("KRAKEN_API_KEY").map_err(|_| "KRAKEN_API_KEY must be set".to_string())?;
    let api_secret =
        env::var("KRAKEN_API_SECRET").map_err(|_| "KRAKEN_API_SECRET must be set".to_string())?;
    Ok((api_key, api_secret))
}
