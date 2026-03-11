//! Cryptographic operations for rockbot-credentials.
//!
//! Provides password-derived key generation using Argon2id and
//! AES-256-GCM encryption/decryption for credential storage.
//!
//! # Security Design
//!
//! - Password → Argon2id → 256-bit master key
//! - Each credential encrypted with unique nonce
//! - Keys are zeroized when dropped

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CredentialError, Result};
use crate::types::Hash256;

/// AES-256-GCM nonce size (96 bits / 12 bytes).
pub const NONCE_SIZE: usize = 12;

/// AES-256 key size (256 bits / 32 bytes).
pub const KEY_SIZE: usize = 32;

/// Salt size for Argon2 (16 bytes recommended).
pub const SALT_SIZE: usize = 16;

/// Master key derived from password, zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    key: [u8; KEY_SIZE],
}

impl MasterKey {
    /// Derives a master key from a password using Argon2id.
    ///
    /// Uses OWASP-recommended Argon2id parameters for password hashing.
    pub fn derive_from_password(password: &str, salt: &[u8]) -> Result<Self> {
        if salt.len() < SALT_SIZE {
            return Err(CredentialError::Internal(format!(
                "salt must be at least {} bytes",
                SALT_SIZE
            )));
        }

        // Use Argon2id with recommended parameters
        let argon2 = Argon2::default();

        // Create a SaltString from the raw bytes
        let salt_string = SaltString::encode_b64(salt)
            .map_err(|e| CredentialError::Internal(format!("failed to encode salt: {}", e)))?;

        // Hash the password
        let hash = argon2
            .hash_password(password.as_bytes(), &salt_string)
            .map_err(|e| CredentialError::Internal(format!("failed to derive key: {}", e)))?;

        // Extract the hash output (should be 32 bytes)
        let hash_output = hash
            .hash
            .ok_or_else(|| CredentialError::Internal("no hash output".to_string()))?;

        let hash_bytes = hash_output.as_bytes();
        if hash_bytes.len() < KEY_SIZE {
            return Err(CredentialError::Internal(
                "hash output too short".to_string(),
            ));
        }

        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(&hash_bytes[..KEY_SIZE]);

        Ok(Self { key })
    }

    /// Creates a master key from raw bytes (for testing or YubiKey-derived keys).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != KEY_SIZE {
            return Err(CredentialError::Internal(format!(
                "key must be exactly {} bytes",
                KEY_SIZE
            )));
        }
        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(bytes);
        Ok(Self { key })
    }

    /// Generates a random master key.
    pub fn generate() -> Self {
        use aes_gcm::aead::rand_core::RngCore;
        let mut key = [0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key);
        Self { key }
    }

    /// Returns the key bytes (use with caution).
    pub fn as_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.key
    }
}

/// Generates a random salt for key derivation.
pub fn generate_salt() -> [u8; SALT_SIZE] {
    use aes_gcm::aead::rand_core::RngCore;
    let mut salt = [0u8; SALT_SIZE];
    OsRng.fill_bytes(&mut salt);
    salt
}

/// Generates a random nonce for AES-GCM encryption.
pub fn generate_nonce() -> [u8; NONCE_SIZE] {
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Encrypts plaintext using AES-256-GCM with the provided key and nonce.
pub fn encrypt(key: &MasterKey, nonce: &[u8; NONCE_SIZE], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CredentialError::Internal(format!("failed to create cipher: {}", e)))?;

    let nonce = Nonce::from_slice(nonce);

    cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| CredentialError::EncryptionFailed)
}

/// Decrypts ciphertext using AES-256-GCM with the provided key and nonce.
pub fn decrypt(key: &MasterKey, nonce: &[u8; NONCE_SIZE], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key.as_bytes())
        .map_err(|e| CredentialError::Internal(format!("failed to create cipher: {}", e)))?;

    let nonce = Nonce::from_slice(nonce);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| CredentialError::DecryptionFailed)
}

/// Computes SHA-256 hash of the input data.
pub fn sha256(data: &[u8]) -> Hash256 {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

/// Computes SHA-256 hash of a string.
pub fn sha256_str(data: &str) -> Hash256 {
    sha256(data.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::hex_encode;

    #[test]
    fn test_key_derivation() {
        let password = "test-password-123";
        let salt = generate_salt();

        let key1 = MasterKey::derive_from_password(password, &salt).unwrap();
        let key2 = MasterKey::derive_from_password(password, &salt).unwrap();

        // Same password + salt should produce same key
        assert_eq!(key1.as_bytes(), key2.as_bytes());

        // Different salt should produce different key
        let salt2 = generate_salt();
        let key3 = MasterKey::derive_from_password(password, &salt2).unwrap();
        assert_ne!(key1.as_bytes(), key3.as_bytes());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = MasterKey::from_bytes(&[0u8; KEY_SIZE]).unwrap();
        let nonce = generate_nonce();
        let plaintext = b"Hello, RockBot!";

        let ciphertext = encrypt(&key, &nonce, plaintext).unwrap();
        assert_ne!(&ciphertext, plaintext);

        let decrypted = decrypt(&key, &nonce, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = MasterKey::from_bytes(&[0u8; KEY_SIZE]).unwrap();
        let key2 = MasterKey::from_bytes(&[1u8; KEY_SIZE]).unwrap();
        let nonce = generate_nonce();
        let plaintext = b"Secret data";

        let ciphertext = encrypt(&key1, &nonce, plaintext).unwrap();
        let result = decrypt(&key2, &nonce, &ciphertext);

        assert!(matches!(result, Err(CredentialError::DecryptionFailed)));
    }

    #[test]
    fn test_decrypt_wrong_nonce_fails() {
        let key = MasterKey::from_bytes(&[0u8; KEY_SIZE]).unwrap();
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        let plaintext = b"Secret data";

        let ciphertext = encrypt(&key, &nonce1, plaintext).unwrap();
        let result = decrypt(&key, &nonce2, &ciphertext);

        assert!(matches!(result, Err(CredentialError::DecryptionFailed)));
    }

    #[test]
    fn test_sha256() {
        // Known test vector
        let hash = sha256(b"hello");
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(hex_encode(&hash), expected);
    }

    #[test]
    fn test_sha256_str() {
        let hash = sha256_str("hello");
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(hex_encode(&hash), expected);
    }

    #[test]
    fn test_salt_generation_is_random() {
        let salt1 = generate_salt();
        let salt2 = generate_salt();
        assert_ne!(salt1, salt2);
    }

    #[test]
    fn test_nonce_generation_is_random() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2);
    }
}
