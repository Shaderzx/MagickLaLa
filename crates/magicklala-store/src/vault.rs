use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};

use crate::StoreError;

const NONCE_LEN: usize = 12;

pub trait CryptoVault: Send + Sync {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, StoreError>;
    fn decrypt(&self, sealed: &[u8]) -> Result<Vec<u8>, StoreError>;
}

pub struct AesGcmVault {
    cipher: Aes256Gcm,
}

impl AesGcmVault {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { cipher: Aes256Gcm::new(key.into()) }
    }
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
