use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::Rng;
use std::sync::Mutex;
use once_cell::sync::Lazy;

use crate::error::{AppError, AppResult};

// Global encryption key (in production, this would be derived from master password)
static MASTER_KEY: Lazy<Mutex<Option<[u8; 32]>>> = Lazy::new(|| Mutex::new(None));

const NONCE_SIZE: usize = 12;

fn get_key() -> [u8; 32] {
    let guard = MASTER_KEY.lock().unwrap();
    guard.unwrap_or_else(|| {
        // Default key for development - in production, derive from user password
        [0u8; 32]
    })
}

pub fn set_master_key(key: &[u8; 32]) {
    let mut guard = MASTER_KEY.lock().unwrap();
    *guard = Some(*key);
}

pub fn encrypt(plaintext: &str) -> AppResult<String> {
    let key = get_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Encryption(e.to_string()))?;
    
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| AppError::Encryption(e.to_string()))?;
    
    // Prepend nonce to ciphertext
    let mut result = nonce_bytes.to_vec();
    result.extend(ciphertext);
    
    Ok(base64_encode(&result))
}

pub fn decrypt(encrypted: &str) -> AppResult<String> {
    let key = get_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| AppError::Encryption(e.to_string()))?;
    
    let data = base64_decode(encrypted)?;
    
    if data.len() < NONCE_SIZE {
        return Err(AppError::Encryption("Invalid encrypted data".to_string()));
    }
    
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);
    
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| AppError::Encryption(e.to_string()))?;
    
    String::from_utf8(plaintext)
        .map_err(|e| AppError::Encryption(e.to_string()))
}

fn base64_encode(data: &[u8]) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.encode(data)
}

fn base64_decode(data: &str) -> AppResult<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.decode(data)
        .map_err(|e| AppError::Encryption(e.to_string()))
}
