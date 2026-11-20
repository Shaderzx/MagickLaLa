use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::StoreError;

const NONCE_LEN: usize = 12;
pub const MASTER_KEY_ENV: &str = "MAGICKLALA_MASTER_KEY";

pub trait CryptoVault: Send + Sync {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, StoreError>;
    fn decrypt(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError>;
}

pub struct AesGcmVault {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for AesGcmVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AesGcmVault").field("key", &"<redacted>").finish()
    }
}

impl AesGcmVault {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()) }
    }

    pub fn from_key_material(material: &str) -> Result<Self, StoreError> {
        let material = material.trim();
        if material.is_empty() {
            return Err(StoreError::Invalid("master key is empty".to_owned()));
        }
        Ok(Self::new(&derive_key(material)))
    }

    pub fn from_env_or_key(fallback: Option<&str>) -> Result<Self, StoreError> {
        match std::env::var(MASTER_KEY_ENV) {
            Ok(value) if !value.trim().is_empty() => Self::from_key_material(&value),
            _ => match fallback {
                Some(key) => Self::from_key_material(key),
                None => Err(StoreError::Invalid(format!("{MASTER_KEY_ENV} is not set"))),
            },
        }
    }
}

fn derive_key(material: &str) -> [u8; 32] {
    let decoded = [hex::decode(material).ok(), STANDARD.decode(material).ok(), URL_SAFE_NO_PAD.decode(material).ok()];
    decoded.into_iter().flatten().find_map(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok()).unwrap_or_else(|| Sha256::digest(material.as_bytes()).into())
}

impl CryptoVault for AesGcmVault {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, StoreError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let body = self.cipher.encrypt(&nonce, plaintext).map_err(|_| StoreError::Crypto)?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&body);
        Ok(sealed)
    }

    fn decrypt(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError> {
        if sealed.len() < NONCE_LEN {
            return Err(StoreError::Crypto);
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        self.cipher.decrypt(Nonce::from_slice(nonce), body).map_err(|_| StoreError::Crypto)
    }
}
