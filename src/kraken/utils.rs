use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256, Sha512};
use std::error::Error;

type HmacSha512 = Hmac<Sha512>;

pub fn get_signature(
    url_path: &str,
    nonce: u128,
    body: &str,
    api_secret: &str,
) -> Result<String, Box<dyn Error>> {
    let encoded_data = format!("{nonce}{body}");
    let digest = Sha256::digest(&encoded_data);
    let mut message: Vec<u8> = Vec::with_capacity(url_path.len() + digest.len());
    message.extend_from_slice(url_path.as_bytes());
    message.extend_from_slice(&digest);

    let secret_bytes = STANDARD.decode(api_secret)?;
    let mut mac = HmacSha512::new_from_slice(&secret_bytes)?;
    mac.update(&message);

    let signature = STANDARD.encode(mac.finalize().into_bytes());
    Ok(signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_ok() {
        assert_eq!(
            get_signature("/0/private/GetWebSocketsToken", 10, "nonce=10", "1234").unwrap(),
            "fM1XozrBc9YJSP9e/na5l/Vrhx+msBUlzlFa1M26XLdaN8Jww0KY2F/k+mS4q2+O81XMmovDzLOevYLuyywyYg=="
        );
    }

    #[test]
    fn sign_fails_with_invalid_key() {
        assert_eq!(
            get_signature("/0/private/GetWebSocketsToken", 10, "nonce=10", "🤓")
                .unwrap_err()
                .to_string(),
            "Invalid symbol 240, offset 0."
        );
    }
}
