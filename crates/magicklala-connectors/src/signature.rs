use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn sign_hmac_sha256(secret: &[u8], message: &[u8]) -> Option<String> {
    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(message);
    Some(hex::encode(mac.finalize().into_bytes()))
}

pub fn verify_hmac_sha256(secret: &[u8], message: &[u8], provided_hex: &str) -> bool {
    let Ok(provided) = hex::decode(provided_hex.trim()) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(message);
    mac.verify_slice(&provided).is_ok()
}
