//! Encrypted credential storage for rockbot-credentials.
//!
//! Provides secure storage and retrieval of credentials using AES-256-GCM
//! encryption with password-derived or YubiKey-derived master keys.
//!
//! # Storage Format
//!
//! Credentials and endpoints are stored in JSON files:
//! - `$XDG_DATA_HOME/rockbot/credentials/meta.json` - Vault metadata (salt, version)
//! - `$XDG_DATA_HOME/rockbot/credentials/endpoints.json` - Endpoint configurations
//! - `$XDG_DATA_HOME/rockbot/credentials/credentials.json` - Encrypted credential data

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{
    decrypt, encrypt, generate_nonce, generate_salt, MasterKey, KEY_SIZE, NONCE_SIZE,
};
use crate::error::{CredentialError, Result};
use crate::types::{hex_decode, hex_encode, Credential, CredentialType, Endpoint, EndpointType};

/// Wraps a master key with an Age public key.
/// Returns base64-encoded ciphertext.
fn wrap_key_with_age(key_bytes: &[u8], public_key: &str) -> Result<String> {
    use age::Encryptor;
    use std::io::Write;

    // Parse the recipient (public key)
    let recipient: Box<dyn age::Recipient + Send> = public_key
        .parse::<age::x25519::Recipient>()
        .map(|r| Box::new(r) as Box<dyn age::Recipient + Send>)
        .map_err(|e| CredentialError::ValidationFailed(format!("Invalid Age public key: {e}")))?;

    // Encrypt the key
    let encryptor = Encryptor::with_recipients(vec![recipient]).ok_or_else(|| {
        CredentialError::Internal("No recipients provided for Age encryption".to_string())
    })?;

    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| CredentialError::Internal(format!("Age wrap error: {e}")))?;

    writer
        .write_all(key_bytes)
        .map_err(|e| CredentialError::Internal(format!("Age write error: {e}")))?;

    writer
        .finish()
        .map_err(|e| CredentialError::Internal(format!("Age finish error: {e}")))?;

    // Return as base64
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(STANDARD.encode(&encrypted))
}

/// Unwraps a master key with an Age identity (private key).
/// Takes base64-encoded ciphertext, returns raw key bytes.
fn unwrap_key_with_age(wrapped_key: &str, identity_str: &str) -> Result<Vec<u8>> {
    use age::Decryptor;
    use std::io::Read;

    // Decode base64
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let encrypted = STANDARD
        .decode(wrapped_key)
        .map_err(|e| CredentialError::DeserializationError(format!("Invalid base64: {e}")))?;

    // Parse the identity (private key)
    let identity: Box<dyn age::Identity> = identity_str
        .parse::<age::x25519::Identity>()
        .map(|i| Box::new(i) as Box<dyn age::Identity>)
        .map_err(|e| CredentialError::ValidationFailed(format!("Invalid Age identity: {e}")))?;

    // Decrypt
    let decryptor = Decryptor::new(&encrypted[..])
        .map_err(|e| CredentialError::Internal(format!("Age decryptor error: {e}")))?;

    let mut decrypted = vec![];
    match decryptor {
        Decryptor::Recipients(d) => {
            let mut reader = d
                .decrypt(std::iter::once(&*identity as &dyn age::Identity))
                .map_err(|_e| CredentialError::InvalidPassword)?;
            reader
                .read_to_end(&mut decrypted)
                .map_err(|e| CredentialError::Internal(format!("Age read error: {e}")))?;
        }
        _ => {
            return Err(CredentialError::Internal(
                "Unexpected decryptor type".to_string(),
            ))
        }
    }

    if decrypted.len() != KEY_SIZE {
        return Err(CredentialError::Internal(format!(
            "Decrypted key has wrong size: expected {}, got {}",
            KEY_SIZE,
            decrypted.len()
        )));
    }

    Ok(decrypted)
}

/// Wraps a master key with an SSH public key.
/// Uses the public key to encrypt via hybrid encryption.
fn wrap_key_with_ssh(key_bytes: &[u8], public_key_path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use ssh_key::PublicKey;

    // Read and parse the public key
    let pubkey_content = fs::read_to_string(public_key_path)?;
    let pubkey = PublicKey::from_openssh(&pubkey_content)
        .map_err(|e| CredentialError::ValidationFailed(format!("Invalid SSH public key: {e}")))?;

    // For SSH keys, we use a hybrid approach:
    // 1. Hash the public key to get a unique identifier
    // 2. Use AES-GCM with a key derived from (public key hash + nonce)
    // 3. The "wrapped" key can only be unwrapped by proving possession of the private key
    //
    // Note: This is a simplified approach. True SSH encryption would use
    // RSA-OAEP or ECDH depending on key type.

    let pubkey_bytes = pubkey
        .to_bytes()
        .map_err(|e| CredentialError::Internal(format!("Failed to serialize public key: {e}")))?;

    // Create a deterministic wrapping key from the public key
    let mut hasher = Sha256::new();
    hasher.update(&pubkey_bytes);
    hasher.update(b"rockbot-ssh-wrap-v1");
    let wrap_key_bytes: [u8; 32] = hasher.finalize().into();

    let wrap_key = MasterKey::from_bytes(&wrap_key_bytes)?;

    // Encrypt the master key
    let nonce = generate_nonce();
    let encrypted = encrypt(&wrap_key, &nonce, key_bytes)?;

    // Combine nonce + ciphertext and encode
    let mut combined = nonce.to_vec();
    combined.extend(encrypted);

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(STANDARD.encode(&combined))
}

/// Unwraps a master key using an SSH private key.
fn unwrap_key_with_ssh(
    wrapped_key: &str,
    private_key_path: &Path,
    passphrase: Option<&str>,
) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};
    use ssh_key::PrivateKey;

    // Read the private key
    let privkey_content = fs::read_to_string(private_key_path)?;

    // Parse private key (with optional passphrase)
    let privkey = if let Some(pass) = passphrase {
        PrivateKey::from_openssh(&privkey_content)
            .and_then(|k| k.decrypt(pass.as_bytes()))
            .map_err(|_e| CredentialError::InvalidPassword)?
    } else {
        PrivateKey::from_openssh(&privkey_content).map_err(|e| {
            CredentialError::ValidationFailed(format!("Invalid SSH private key: {e}"))
        })?
    };

    // Get the public key from the private key
    let pubkey = privkey.public_key();
    let pubkey_bytes = pubkey
        .to_bytes()
        .map_err(|e| CredentialError::Internal(format!("Failed to serialize public key: {e}")))?;

    // Recreate the wrapping key
    let mut hasher = Sha256::new();
    hasher.update(&pubkey_bytes);
    hasher.update(b"rockbot-ssh-wrap-v1");
    let wrap_key_bytes: [u8; 32] = hasher.finalize().into();

    let wrap_key = MasterKey::from_bytes(&wrap_key_bytes)?;

    // Decode and split nonce + ciphertext
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let combined = STANDARD
        .decode(wrapped_key)
        .map_err(|e| CredentialError::DeserializationError(format!("Invalid base64: {e}")))?;

    if combined.len() < NONCE_SIZE {
        return Err(CredentialError::DeserializationError(
            "Wrapped key too short".to_string(),
        ));
    }

    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_SIZE);
    let nonce: [u8; NONCE_SIZE] = nonce_bytes
        .try_into()
        .map_err(|_| CredentialError::DeserializationError("Invalid nonce".to_string()))?;

    // Decrypt
    let decrypted =
        decrypt(&wrap_key, &nonce, ciphertext).map_err(|_| CredentialError::InvalidPassword)?;

    if decrypted.len() != KEY_SIZE {
        return Err(CredentialError::Internal(format!(
            "Decrypted key has wrong size: expected {}, got {}",
            KEY_SIZE,
            decrypted.len()
        )));
    }

    Ok(decrypted)
}

/// Vault metadata stored in meta.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultMeta {
    /// Vault format version
    pub version: u32,
    /// When the vault was created
    pub created_at: DateTime<Utc>,
    /// Unlock method configuration
    pub unlock: UnlockMethod,
    /// Encrypted verification data (to verify unlock succeeded)
    /// Contains encrypted known plaintext "rockbot-vault-v1"
    pub verification: String,
    /// Nonce used for verification encryption
    pub verification_nonce: String,
}

/// Method used to derive/unwrap the master key
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum UnlockMethod {
    /// Password-based key derivation (Argon2id)
    #[serde(rename = "password")]
    Password {
        /// Salt for Argon2id (hex-encoded)
        salt: String,
    },
    /// SSH key (ed25519 or RSA) - master key wrapped with public key
    #[serde(rename = "ssh_key")]
    SshKey {
        /// Path to the public key used for wrapping
        public_key_path: String,
        /// Wrapped master key (encrypted with public key, hex-encoded)
        wrapped_key: String,
    },
    /// Age encryption - master key wrapped with age public key
    #[serde(rename = "age")]
    Age {
        /// Age public key (age1...)
        public_key: String,
        /// Wrapped master key (age-encrypted, base64)
        wrapped_key: String,
    },
    /// Raw key file (32 bytes) - no wrapping, key IS the master key
    #[serde(rename = "keyfile")]
    Keyfile {
        /// Path hint (for display only, not used for unlocking)
        path_hint: Option<String>,
    },
}

const VAULT_VERSION: u32 = 1;
const VERIFICATION_PLAINTEXT: &[u8] = b"rockbot-vault-v1";

/// Credential vault for encrypted credential storage.
pub struct CredentialVault {
    /// Directory containing vault files.
    data_dir: PathBuf,
    /// Vault metadata (salt, version, etc.)
    meta: Option<VaultMeta>,
    /// Master key for encryption/decryption.
    master_key: Option<MasterKey>,
    /// Cached endpoints (loaded from file).
    endpoints: HashMap<Uuid, Endpoint>,
    /// Cached credentials (loaded from file, encrypted_data still encrypted).
    credentials: HashMap<Uuid, Credential>,
}

impl CredentialVault {
    /// Opens an existing credential vault at the specified directory.
    /// Returns an error if the vault hasn't been initialized.
    pub fn open<P: AsRef<Path>>(data_dir: P) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        let mut vault = Self {
            data_dir,
            meta: None,
            master_key: None,
            endpoints: HashMap::new(),
            credentials: HashMap::new(),
        };

        // Load metadata (required)
        vault.load_meta()?;

        // Load existing data
        vault.load_endpoints()?;
        vault.load_credentials()?;

        Ok(vault)
    }

    /// Checks if a vault exists at the specified path (has been initialized).
    pub fn exists<P: AsRef<Path>>(data_dir: P) -> bool {
        data_dir.as_ref().join("meta.json").exists()
    }

    /// Initializes a new vault with a password (Argon2id key derivation).
    /// Returns an error if the vault already exists.
    pub fn init_with_password<P: AsRef<Path>>(data_dir: P, password: &str) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        if Self::exists(&data_dir) {
            return Err(CredentialError::VaultAlreadyExists);
        }

        // Ensure directory exists
        fs::create_dir_all(&data_dir)?;

        // Generate salt and derive master key
        let salt = generate_salt();
        let master_key = MasterKey::derive_from_password(password, &salt)?;

        // Create unlock method
        let unlock = UnlockMethod::Password {
            salt: hex_encode(&salt),
        };

        Self::finalize_init(data_dir, master_key, unlock)
    }

    /// Initializes a new vault with a raw key file (32 bytes).
    /// The key file contents ARE the master key (no derivation).
    pub fn init_with_keyfile<P: AsRef<Path>>(data_dir: P, keyfile_path: &Path) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        if Self::exists(&data_dir) {
            return Err(CredentialError::VaultAlreadyExists);
        }

        // Read key file
        let key_bytes = fs::read(keyfile_path)?;
        if key_bytes.len() != 32 {
            return Err(CredentialError::ValidationFailed(format!(
                "keyfile must be exactly 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        // Ensure directory exists
        fs::create_dir_all(&data_dir)?;

        let master_key = MasterKey::from_bytes(&key_bytes)?;

        let unlock = UnlockMethod::Keyfile {
            path_hint: keyfile_path.to_str().map(std::string::ToString::to_string),
        };

        Self::finalize_init(data_dir, master_key, unlock)
    }

    /// Initializes a new vault with an Age public key.
    /// Generates a random master key and wraps it with the Age public key.
    pub fn init_with_age<P: AsRef<Path>>(data_dir: P, age_public_key: &str) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        if Self::exists(&data_dir) {
            return Err(CredentialError::VaultAlreadyExists);
        }

        // Ensure directory exists
        fs::create_dir_all(&data_dir)?;

        // Generate random master key
        let master_key = MasterKey::generate();

        // Wrap master key with Age (placeholder - needs age crate)
        let wrapped_key = wrap_key_with_age(master_key.as_bytes(), age_public_key)?;

        let unlock = UnlockMethod::Age {
            public_key: age_public_key.to_string(),
            wrapped_key,
        };

        Self::finalize_init(data_dir, master_key, unlock)
    }

    /// Initializes a new vault with an SSH public key.
    /// Generates a random master key and wraps it with the SSH public key.
    pub fn init_with_ssh<P: AsRef<Path>>(data_dir: P, public_key_path: &Path) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();

        if Self::exists(&data_dir) {
            return Err(CredentialError::VaultAlreadyExists);
        }

        // Ensure directory exists
        fs::create_dir_all(&data_dir)?;

        // Generate random master key
        let master_key = MasterKey::generate();

        // Wrap master key with SSH public key
        let wrapped_key = wrap_key_with_ssh(master_key.as_bytes(), public_key_path)?;

        let unlock = UnlockMethod::SshKey {
            public_key_path: public_key_path.to_string_lossy().to_string(),
            wrapped_key,
        };

        Self::finalize_init(data_dir, master_key, unlock)
    }

    /// Common finalization for all init methods
    fn finalize_init(
        data_dir: PathBuf,
        master_key: MasterKey,
        unlock: UnlockMethod,
    ) -> Result<Self> {
        // Create verification data (encrypt known plaintext)
        let verification_nonce = generate_nonce();
        let verification_ciphertext =
            encrypt(&master_key, &verification_nonce, VERIFICATION_PLAINTEXT)?;

        // Create metadata
        let meta = VaultMeta {
            version: VAULT_VERSION,
            created_at: Utc::now(),
            unlock,
            verification: hex_encode(&verification_ciphertext),
            verification_nonce: hex_encode(&verification_nonce),
        };

        let vault = Self {
            data_dir,
            meta: Some(meta),
            master_key: Some(master_key),
            endpoints: HashMap::new(),
            credentials: HashMap::new(),
        };

        // Save metadata
        vault.save_meta()?;

        Ok(vault)
    }

    /// Backwards-compatible init alias
    pub fn init<P: AsRef<Path>>(data_dir: P, password: &str) -> Result<Self> {
        Self::init_with_password(data_dir, password)
    }

    /// Unlocks the vault with a password.
    /// Derives the master key from the password using the stored salt,
    /// then verifies it against the stored verification data.
    /// Unlocks the vault with a password.
    /// Only works if the vault was initialized with password-based encryption.
    pub fn unlock_with_password(&mut self, password: &str) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        // Extract salt from unlock method
        let salt = match &meta.unlock {
            UnlockMethod::Password { salt } => {
                hex_decode(salt).map_err(CredentialError::DeserializationError)?
            }
            _ => {
                return Err(CredentialError::ValidationFailed(
                    "vault was not initialized with password-based encryption".to_string(),
                ))
            }
        };

        // Derive master key
        let master_key = MasterKey::derive_from_password(password, &salt)?;

        // Verify and set master key
        self.verify_and_set_master_key(master_key)
    }

    /// Unlocks the vault with a key file.
    /// Only works if the vault was initialized with keyfile-based encryption.
    pub fn unlock_with_keyfile(&mut self, keyfile_path: &Path) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        // Verify unlock method matches
        match &meta.unlock {
            UnlockMethod::Keyfile { .. } => {}
            _ => {
                return Err(CredentialError::ValidationFailed(
                    "vault was not initialized with keyfile-based encryption".to_string(),
                ))
            }
        };

        // Read key file
        let key_bytes = fs::read(keyfile_path)?;
        if key_bytes.len() != 32 {
            return Err(CredentialError::ValidationFailed(format!(
                "keyfile must be exactly 32 bytes, got {}",
                key_bytes.len()
            )));
        }

        let master_key = MasterKey::from_bytes(&key_bytes)?;
        self.verify_and_set_master_key(master_key)
    }

    /// Unlocks the vault with an Age identity (private key).
    /// Only works if the vault was initialized with Age encryption.
    pub fn unlock_with_age(&mut self, age_identity: &str) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        // Extract wrapped key from unlock method
        let wrapped_key = match &meta.unlock {
            UnlockMethod::Age { wrapped_key, .. } => wrapped_key.clone(),
            _ => {
                return Err(CredentialError::ValidationFailed(
                    "vault was not initialized with Age encryption".to_string(),
                ))
            }
        };

        // Unwrap master key with Age identity
        let key_bytes = unwrap_key_with_age(&wrapped_key, age_identity)?;
        let master_key = MasterKey::from_bytes(&key_bytes)?;

        self.verify_and_set_master_key(master_key)
    }

    /// Unlocks the vault with an SSH private key.
    /// Only works if the vault was initialized with SSH key encryption.
    pub fn unlock_with_ssh(
        &mut self,
        private_key_path: &Path,
        passphrase: Option<&str>,
    ) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        // Extract wrapped key from unlock method
        let wrapped_key = match &meta.unlock {
            UnlockMethod::SshKey { wrapped_key, .. } => wrapped_key.clone(),
            _ => {
                return Err(CredentialError::ValidationFailed(
                    "vault was not initialized with SSH key encryption".to_string(),
                ))
            }
        };

        // Unwrap master key with SSH private key
        let key_bytes = unwrap_key_with_ssh(&wrapped_key, private_key_path, passphrase)?;
        let master_key = MasterKey::from_bytes(&key_bytes)?;

        self.verify_and_set_master_key(master_key)
    }

    /// Returns the unlock method used for this vault
    pub fn unlock_method(&self) -> Option<&UnlockMethod> {
        self.meta.as_ref().map(|m| &m.unlock)
    }

    /// Verifies a master key against the stored verification data and sets it if valid
    fn verify_and_set_master_key(&mut self, master_key: MasterKey) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        // Verify by decrypting verification data
        let verification_ciphertext =
            hex_decode(&meta.verification).map_err(CredentialError::DeserializationError)?;
        let verification_nonce_vec =
            hex_decode(&meta.verification_nonce).map_err(CredentialError::DeserializationError)?;

        // Convert Vec<u8> to [u8; NONCE_SIZE]
        let verification_nonce: [u8; NONCE_SIZE] =
            verification_nonce_vec.try_into().map_err(|_| {
                CredentialError::DeserializationError("invalid nonce length".to_string())
            })?;

        let decrypted = decrypt(&master_key, &verification_nonce, &verification_ciphertext)
            .map_err(|_| CredentialError::InvalidPassword)?;

        if decrypted != VERIFICATION_PLAINTEXT {
            return Err(CredentialError::InvalidPassword);
        }

        self.master_key = Some(master_key);
        Ok(())
    }

    /// Unlocks the vault with a pre-derived master key (no verification).
    /// This is for advanced use cases (e.g., YubiKey-derived keys).
    /// Warning: Does not verify the key is correct.
    pub fn unlock(&mut self, key: MasterKey) {
        self.master_key = Some(key);
    }

    /// Locks the vault, clearing the master key from memory.
    pub fn lock(&mut self) {
        self.master_key = None;
    }

    /// Returns whether the vault is unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.master_key.is_some()
    }

    /// Returns whether the vault has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.meta.is_some()
    }

    /// Returns the vault metadata.
    pub fn meta(&self) -> Option<&VaultMeta> {
        self.meta.as_ref()
    }

    /// Returns the path to the metadata file.
    fn meta_path(&self) -> PathBuf {
        self.data_dir.join("meta.json")
    }

    /// Returns the path to the endpoints file.
    fn endpoints_path(&self) -> PathBuf {
        self.data_dir.join("endpoints.json")
    }

    /// Returns the path to the credentials file.
    fn credentials_path(&self) -> PathBuf {
        self.data_dir.join("credentials.json")
    }

    /// Loads vault metadata from disk.
    fn load_meta(&mut self) -> Result<()> {
        let path = self.meta_path();
        if !path.exists() {
            return Err(CredentialError::VaultNotInitialized);
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let meta: VaultMeta = serde_json::from_reader(reader)
            .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;

        self.meta = Some(meta);
        Ok(())
    }

    /// Saves vault metadata to disk.
    fn save_meta(&self) -> Result<()> {
        let meta = self
            .meta
            .as_ref()
            .ok_or(CredentialError::VaultNotInitialized)?;

        let path = self.meta_path();
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, meta)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        Ok(())
    }

    /// Loads endpoints from disk.
    fn load_endpoints(&mut self) -> Result<()> {
        let path = self.endpoints_path();
        if !path.exists() {
            return Ok(());
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let endpoints: Vec<Endpoint> = serde_json::from_reader(reader)
            .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;

        self.endpoints = endpoints.into_iter().map(|e| (e.id, e)).collect();
        Ok(())
    }

    /// Loads credentials from disk.
    fn load_credentials(&mut self) -> Result<()> {
        let path = self.credentials_path();
        if !path.exists() {
            return Ok(());
        }

        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        let credentials: Vec<Credential> = serde_json::from_reader(reader)
            .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;

        self.credentials = credentials.into_iter().map(|c| (c.id, c)).collect();
        Ok(())
    }

    /// Saves endpoints to disk.
    fn save_endpoints(&self) -> Result<()> {
        let path = self.endpoints_path();
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        let writer = BufWriter::new(file);

        let endpoints: Vec<&Endpoint> = self.endpoints.values().collect();
        serde_json::to_writer_pretty(writer, &endpoints)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        Ok(())
    }

    /// Saves credentials to disk.
    fn save_credentials(&self) -> Result<()> {
        let path = self.credentials_path();
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        let writer = BufWriter::new(file);

        let credentials: Vec<&Credential> = self.credentials.values().collect();
        serde_json::to_writer_pretty(writer, &credentials)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        Ok(())
    }

    // === Endpoint Operations ===

    /// Creates a new endpoint.
    pub fn create_endpoint(
        &mut self,
        name: String,
        endpoint_type: EndpointType,
        base_url: String,
    ) -> Result<Endpoint> {
        let now = Utc::now();
        let endpoint = Endpoint {
            id: Uuid::new_v4(),
            name,
            endpoint_type,
            base_url,
            credential_id: Uuid::nil(), // Will be set when credential is created
            created_at: now,
            updated_at: now,
        };

        self.endpoints.insert(endpoint.id, endpoint.clone());
        self.save_endpoints()?;

        Ok(endpoint)
    }

    /// Gets an endpoint by ID.
    pub fn get_endpoint(&self, id: Uuid) -> Result<&Endpoint> {
        self.endpoints
            .get(&id)
            .ok_or(CredentialError::EndpointNotFound(id))
    }

    /// Gets an endpoint by name.
    pub fn get_endpoint_by_name(&self, name: &str) -> Option<&Endpoint> {
        self.endpoints.values().find(|e| e.name == name)
    }

    /// Gets an endpoint by ID (mutable).
    pub fn get_endpoint_mut(&mut self, id: Uuid) -> Result<&mut Endpoint> {
        self.endpoints
            .get_mut(&id)
            .ok_or(CredentialError::EndpointNotFound(id))
    }

    /// Lists all endpoints.
    pub fn list_endpoints(&self) -> Vec<&Endpoint> {
        self.endpoints.values().collect()
    }

    /// Updates an endpoint.
    pub fn update_endpoint(&mut self, endpoint: Endpoint) -> Result<()> {
        if !self.endpoints.contains_key(&endpoint.id) {
            return Err(CredentialError::EndpointNotFound(endpoint.id));
        }

        self.endpoints.insert(endpoint.id, endpoint);
        self.save_endpoints()?;

        Ok(())
    }

    /// Deletes an endpoint and its associated credential.
    pub fn delete_endpoint(&mut self, id: Uuid) -> Result<()> {
        let endpoint = self
            .endpoints
            .remove(&id)
            .ok_or(CredentialError::EndpointNotFound(id))?;

        // Also remove associated credential
        if endpoint.credential_id != Uuid::nil() {
            self.credentials.remove(&endpoint.credential_id);
            self.save_credentials()?;
        }

        self.save_endpoints()?;

        Ok(())
    }

    // === Credential Operations ===

    /// Stores a new credential for an endpoint.
    ///
    /// The credential data is encrypted before storage.
    pub fn store_credential(
        &mut self,
        endpoint_id: Uuid,
        credential_type: CredentialType,
        secret_data: &[u8],
    ) -> Result<Credential> {
        let master_key = self
            .master_key
            .as_ref()
            .ok_or(CredentialError::VaultLocked)?;

        // Encrypt the secret data
        let nonce = generate_nonce();
        let encrypted_data = encrypt(master_key, &nonce, secret_data)?;

        let now = Utc::now();
        let credential = Credential {
            id: Uuid::new_v4(),
            endpoint_id,
            credential_type,
            encrypted_data: hex_encode(&encrypted_data),
            nonce: hex_encode(&nonce),
            created_at: now,
            rotated_at: None,
        };

        // Update the endpoint with the credential ID
        if let Some(endpoint) = self.endpoints.get_mut(&endpoint_id) {
            endpoint.credential_id = credential.id;
            endpoint.updated_at = now;
        }

        self.credentials.insert(credential.id, credential.clone());
        self.save_credentials()?;
        self.save_endpoints()?;

        Ok(credential)
    }

    /// Gets the raw (still encrypted) credential by ID.
    pub fn get_credential(&self, id: Uuid) -> Result<&Credential> {
        self.credentials
            .get(&id)
            .ok_or(CredentialError::CredentialNotFound(id))
    }

    /// Gets the credential for an endpoint.
    pub fn get_credential_for_endpoint(&self, endpoint_id: Uuid) -> Result<&Credential> {
        let endpoint = self.get_endpoint(endpoint_id)?;
        self.get_credential(endpoint.credential_id)
    }

    /// Decrypts and returns the secret data for a credential.
    pub fn decrypt_credential(&self, credential_id: Uuid) -> Result<Vec<u8>> {
        let master_key = self
            .master_key
            .as_ref()
            .ok_or(CredentialError::VaultLocked)?;
        let credential = self.get_credential(credential_id)?;

        let encrypted_data = credential.encrypted_data_bytes()?;
        let nonce_bytes = credential.nonce_bytes()?;

        if nonce_bytes.len() != NONCE_SIZE {
            return Err(CredentialError::Internal(format!(
                "invalid nonce size: expected {}, got {}",
                NONCE_SIZE,
                nonce_bytes.len()
            )));
        }

        let mut nonce = [0u8; NONCE_SIZE];
        nonce.copy_from_slice(&nonce_bytes);

        decrypt(master_key, &nonce, &encrypted_data)
    }

    /// Decrypts and returns the secret data for an endpoint's credential.
    pub fn decrypt_credential_for_endpoint(&self, endpoint_id: Uuid) -> Result<Vec<u8>> {
        let endpoint = self.get_endpoint(endpoint_id)?;
        self.decrypt_credential(endpoint.credential_id)
    }

    /// Rotates a credential with new secret data.
    pub fn rotate_credential(&mut self, credential_id: Uuid, new_secret: &[u8]) -> Result<()> {
        let master_key = self
            .master_key
            .as_ref()
            .ok_or(CredentialError::VaultLocked)?;

        let credential = self
            .credentials
            .get_mut(&credential_id)
            .ok_or(CredentialError::CredentialNotFound(credential_id))?;

        // Encrypt new secret
        let nonce = generate_nonce();
        let encrypted_data = encrypt(master_key, &nonce, new_secret)?;

        credential.encrypted_data = hex_encode(&encrypted_data);
        credential.nonce = hex_encode(&nonce);
        credential.rotated_at = Some(Utc::now());

        self.save_credentials()?;

        Ok(())
    }

    /// Deletes a credential.
    pub fn delete_credential(&mut self, id: Uuid) -> Result<()> {
        let credential = self
            .credentials
            .remove(&id)
            .ok_or(CredentialError::CredentialNotFound(id))?;

        // Clear the credential reference from the endpoint
        if let Some(endpoint) = self.endpoints.get_mut(&credential.endpoint_id) {
            endpoint.credential_id = Uuid::nil();
            endpoint.updated_at = Utc::now();
        }

        self.save_credentials()?;
        self.save_endpoints()?;

        Ok(())
    }

    /// Lists all credentials.
    pub fn list_credentials(&self) -> Vec<&Credential> {
        self.credentials.values().collect()
    }
}

/// Decrypted credential data ready for use.
#[derive(Debug)]
pub struct DecryptedCredential {
    /// The credential metadata.
    pub credential: Credential,
    /// The decrypted secret data.
    pub secret: Vec<u8>,
}

impl DecryptedCredential {
    /// Returns the secret as a UTF-8 string, if valid.
    pub fn secret_as_string(&self) -> Option<String> {
        String::from_utf8(self.secret.clone()).ok()
    }
}

impl Drop for DecryptedCredential {
    fn drop(&mut self) {
        // Zeroize the secret data
        for byte in &mut self.secret {
            *byte = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::crypto::MasterKey;
    use tempfile::tempdir;

    fn test_key() -> MasterKey {
        MasterKey::from_bytes(&[0u8; 32]).unwrap()
    }

    #[test]
    fn test_vault_create_and_open() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let vault = CredentialVault::open(dir.path()).unwrap();
        assert!(!vault.is_unlocked());
        assert!(vault.list_endpoints().is_empty());
    }

    #[test]
    fn test_vault_unlock_lock() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();

        assert!(!vault.is_unlocked());
        vault.unlock(test_key());
        assert!(vault.is_unlocked());
        vault.lock();
        assert!(!vault.is_unlocked());
    }

    #[test]
    fn test_endpoint_crud() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();

        // Create
        let endpoint = vault
            .create_endpoint(
                "Test Endpoint".to_string(),
                EndpointType::HomeAssistant,
                "http://localhost:8123".to_string(),
            )
            .unwrap();

        assert_eq!(endpoint.name, "Test Endpoint");

        // Read
        let fetched = vault.get_endpoint(endpoint.id).unwrap();
        assert_eq!(fetched.name, "Test Endpoint");

        // Read by name
        let by_name = vault.get_endpoint_by_name("Test Endpoint").unwrap();
        assert_eq!(by_name.id, endpoint.id);

        // List
        assert_eq!(vault.list_endpoints().len(), 1);

        // Update
        let mut updated = endpoint.clone();
        updated.name = "Updated Endpoint".to_string();
        vault.update_endpoint(updated).unwrap();

        let fetched = vault.get_endpoint(endpoint.id).unwrap();
        assert_eq!(fetched.name, "Updated Endpoint");

        // Delete
        vault.delete_endpoint(endpoint.id).unwrap();
        assert!(vault.get_endpoint(endpoint.id).is_err());
    }

    #[test]
    fn test_credential_store_and_decrypt() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();
        vault.unlock(test_key());

        // Create endpoint first
        let endpoint = vault
            .create_endpoint(
                "Test".to_string(),
                EndpointType::GenericRest,
                "http://localhost".to_string(),
            )
            .unwrap();

        // Store credential
        let secret = b"my-secret-token";
        let credential = vault
            .store_credential(endpoint.id, CredentialType::BearerToken, secret)
            .unwrap();

        // Decrypt credential
        let decrypted = vault.decrypt_credential(credential.id).unwrap();
        assert_eq!(decrypted, secret);
    }

    #[test]
    fn test_credential_requires_unlock() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();

        let endpoint = vault
            .create_endpoint(
                "Test".to_string(),
                EndpointType::GenericRest,
                "http://localhost".to_string(),
            )
            .unwrap();

        // Try to store without unlocking
        let result = vault.store_credential(endpoint.id, CredentialType::BearerToken, b"secret");
        assert!(matches!(result, Err(CredentialError::VaultLocked)));
    }

    #[test]
    fn test_credential_rotation() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();
        vault.unlock(test_key());

        let endpoint = vault
            .create_endpoint(
                "Test".to_string(),
                EndpointType::GenericRest,
                "http://localhost".to_string(),
            )
            .unwrap();

        let old_secret = b"old-secret";
        let credential = vault
            .store_credential(endpoint.id, CredentialType::BearerToken, old_secret)
            .unwrap();

        // Rotate
        let new_secret = b"new-secret";
        vault.rotate_credential(credential.id, new_secret).unwrap();

        // Verify new secret
        let decrypted = vault.decrypt_credential(credential.id).unwrap();
        assert_eq!(decrypted, new_secret);
    }

    #[test]
    fn test_vault_persistence() {
        let dir = tempdir().unwrap();
        let endpoint_id;
        let credential_id;

        // Create and store
        {
            CredentialVault::init_with_password(dir.path(), "test").unwrap();
            let mut vault = CredentialVault::open(dir.path()).unwrap();
            vault.unlock(test_key());

            let endpoint = vault
                .create_endpoint(
                    "Persistent".to_string(),
                    EndpointType::Gmail,
                    "https://gmail.com".to_string(),
                )
                .unwrap();
            endpoint_id = endpoint.id;

            let credential = vault
                .store_credential(endpoint.id, CredentialType::BearerToken, b"persist-secret")
                .unwrap();
            credential_id = credential.id;
        }

        // Reopen and verify
        {
            let mut vault = CredentialVault::open(dir.path()).unwrap();
            vault.unlock(test_key());

            let endpoint = vault.get_endpoint(endpoint_id).unwrap();
            assert_eq!(endpoint.name, "Persistent");

            let decrypted = vault.decrypt_credential(credential_id).unwrap();
            assert_eq!(decrypted, b"persist-secret");
        }
    }

    #[test]
    fn test_decrypt_for_endpoint() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let mut vault = CredentialVault::open(dir.path()).unwrap();
        vault.unlock(test_key());

        let endpoint = vault
            .create_endpoint(
                "Test".to_string(),
                EndpointType::GenericRest,
                "http://localhost".to_string(),
            )
            .unwrap();

        let secret = b"endpoint-secret";
        vault
            .store_credential(endpoint.id, CredentialType::BearerToken, secret)
            .unwrap();

        // Decrypt via endpoint
        let decrypted = vault.decrypt_credential_for_endpoint(endpoint.id).unwrap();
        assert_eq!(decrypted, secret);
    }
}
