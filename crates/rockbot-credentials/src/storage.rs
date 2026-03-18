//! Encrypted credential storage for rockbot-credentials.
//!
//! Provides secure storage and retrieval of credentials using AES-256-GCM
//! encryption with password-derived or YubiKey-derived master keys.
//!
//! # Storage Format
//!
//! Credentials, endpoints, and permissions are stored in a redb database:
//! - `$XDG_DATA_HOME/rockbot/credentials/vault.db` - All vault data
//! - `$XDG_DATA_HOME/rockbot/credentials/meta.json` - Vault metadata (salt, version)
//! - `$XDG_DATA_HOME/rockbot/credentials/audit.log` - Append-only audit trail

use std::fs::{self, File};
use std::io::BufReader;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{
    decrypt, encrypt, generate_nonce, generate_salt, MasterKey, KEY_SIZE, NONCE_SIZE,
};
use crate::error::{CredentialError, Result};
use crate::types::{
    hex_decode, hex_encode, Credential, CredentialType, Endpoint, EndpointType, RegisteredNodeKey,
    VaultGrantKind, VaultGrantRecord, VaultObjectRecord,
};

use rockbot_storage::tables;
use rockbot_storage::Store;

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

/// Encrypt arbitrary data for an Age recipient public key and return a
/// base64-encoded ciphertext.
fn encrypt_for_age_recipient(data: &[u8], public_key: &str) -> Result<String> {
    use age::Encryptor;
    use std::io::Write;

    let recipient: Box<dyn age::Recipient + Send> = public_key
        .parse::<age::x25519::Recipient>()
        .map(|r| Box::new(r) as Box<dyn age::Recipient + Send>)
        .map_err(|e| CredentialError::ValidationFailed(format!("Invalid Age public key: {e}")))?;

    let encryptor = Encryptor::with_recipients(vec![recipient]).ok_or_else(|| {
        CredentialError::Internal("No recipients provided for Age encryption".to_string())
    })?;

    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|e| CredentialError::Internal(format!("Age wrap error: {e}")))?;
    writer
        .write_all(data)
        .map_err(|e| CredentialError::Internal(format!("Age write error: {e}")))?;
    writer
        .finish()
        .map_err(|e| CredentialError::Internal(format!("Age finish error: {e}")))?;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    Ok(STANDARD.encode(&encrypted))
}

/// Decrypt a base64-encoded Age ciphertext with a node identity.
fn decrypt_with_age_identity(ciphertext: &str, identity_str: &str) -> Result<Vec<u8>> {
    use age::Decryptor;
    use std::io::Read;

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let encrypted = STANDARD
        .decode(ciphertext)
        .map_err(|e| CredentialError::DeserializationError(format!("Invalid base64: {e}")))?;

    let identity: Box<dyn age::Identity> = identity_str
        .parse::<age::x25519::Identity>()
        .map(|i| Box::new(i) as Box<dyn age::Identity>)
        .map_err(|e| CredentialError::ValidationFailed(format!("Invalid Age identity: {e}")))?;

    let decryptor = Decryptor::new(&encrypted[..])
        .map_err(|e| CredentialError::Internal(format!("Age decryptor error: {e}")))?;

    let mut decrypted = vec![];
    match decryptor {
        Decryptor::Recipients(d) => {
            let mut reader = d
                .decrypt(std::iter::once(&*identity as &dyn age::Identity))
                .map_err(|_| CredentialError::InvalidPassword)?;
            reader
                .read_to_end(&mut decrypted)
                .map_err(|e| CredentialError::Internal(format!("Age read error: {e}")))?;
        }
        _ => {
            return Err(CredentialError::Internal(
                "Unexpected Age decryptor type".to_string(),
            ))
        }
    }

    Ok(decrypted)
}

fn wrap_key_with_ssh(key_bytes: &[u8], public_key_path: &Path) -> Result<String> {
    let _ = (key_bytes, public_key_path);
    Err(CredentialError::ValidationFailed(
        "SSH-key vault wrapping is disabled because the current design derives wrapping material from public key data and is not secure. Use age, password, or keyfile unlock instead.".to_string(),
    ))
}

fn unwrap_key_with_ssh(
    wrapped_key: &str,
    private_key_path: &Path,
    passphrase: Option<&str>,
) -> Result<Vec<u8>> {
    let _ = (wrapped_key, private_key_path, passphrase);
    Err(CredentialError::ValidationFailed(
        "SSH-key vault wrapping is disabled because the current design derives wrapping material from public key data and is not secure. Reinitialize the vault with age, password, or keyfile unlock.".to_string(),
    ))
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
///
/// Uses redb (via `rockbot-storage`) for persistent storage of endpoints,
/// credentials, and permissions. Metadata is kept in a separate JSON file
/// for backwards compatibility with the unlock flow.
pub struct CredentialVault {
    /// Directory containing vault files.
    data_dir: PathBuf,
    /// Vault metadata (salt, version, etc.)
    meta: Option<VaultMeta>,
    /// Master key for encryption/decryption.
    master_key: Option<MasterKey>,
    /// Redb-backed store for endpoints, credentials, permissions, and KV data.
    store: Store,
}

impl CredentialVault {
    fn disk_path_for_dir(data_dir: &Path) -> PathBuf {
        data_dir
            .parent()
            .map(rockbot_storage::Store::default_disk_path)
            .unwrap_or_else(|| data_dir.join(rockbot_storage::Store::DEFAULT_DATA_FILE))
    }

    fn migrate_legacy_redb_volume(
        data_dir: &Path,
        disk_path: &Path,
        volume_name: &str,
    ) -> Result<()> {
        let legacy_path = data_dir.join("vault.db");
        if !legacy_path.exists() {
            return Ok(());
        }

        let legacy_size = fs::metadata(&legacy_path)?.len();
        let volume_info = rockbot_vdisk::volume_info(disk_path, volume_name)
            .map_err(|e| CredentialError::Internal(format!("Failed to inspect virtual disk: {e}")))?;
        let needs_import = match volume_info {
            None => true,
            Some(info) if info.len != legacy_size => true,
            Some(_) => {
                let prefix = rockbot_vdisk::read_volume_prefix(disk_path, volume_name, 4)
                    .map_err(|e| {
                        CredentialError::Internal(format!(
                            "Failed to inspect virtual disk contents: {e}"
                        ))
                    })?
                    .unwrap_or_default();
                prefix.as_slice() != b"redb"
            }
        };
        if !needs_import {
            return Ok(());
        }

        tracing::info!(
            "Importing legacy {} into {} volume",
            legacy_path.display(),
            volume_name
        );
        rockbot_vdisk::replace_file(disk_path, volume_name, &legacy_path, None).map_err(|e| {
            CredentialError::Internal(format!(
                "Failed to import legacy vault store {} into virtual disk: {e}",
                legacy_path.display()
            ))
        })?;
        Ok(())
    }

    /// Opens an existing credential vault at the specified directory.
    /// Returns an error if the vault hasn't been initialized.
    ///
    /// If legacy JSON files (`endpoints.json`, `credentials.json`) exist,
    /// they are automatically migrated into the redb database.
    pub fn open<P: AsRef<Path>>(data_dir: P) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let disk_path = Self::disk_path_for_dir(&data_dir);

        Self::migrate_legacy_redb_volume(&data_dir, &disk_path, "vault")?;

        let store = match catch_unwind(AssertUnwindSafe(|| {
            Store::open_volume(&disk_path, "vault", 256 * 1024 * 1024, None)
        })) {
            Ok(Ok(store)) => store,
            Ok(Err(e)) => {
                return Err(CredentialError::Internal(format!(
                    "Failed to open store: {e}"
                )))
            }
            Err(_) => {
                return Err(CredentialError::Internal(
                    "Failed to open store: redb panicked while opening the vault volume"
                        .to_string(),
                ))
            }
        };

        let mut vault = Self {
            data_dir,
            meta: None,
            master_key: None,
            store,
        };

        // Load metadata (required)
        vault.load_meta()?;

        // Migrate legacy JSON files if they exist
        vault.migrate_legacy_json()?;

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

        // Wrap master key with Age
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

        let store = Store::open_volume(
            &Self::disk_path_for_dir(&data_dir),
            "vault",
            256 * 1024 * 1024,
            None,
        )
        .map_err(|e| CredentialError::Internal(format!("Failed to open store: {e}")))?;

        let vault = Self {
            data_dir,
            meta: Some(meta),
            master_key: Some(master_key),
            store,
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
        let writer = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(writer, meta)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;

        Ok(())
    }

    /// Migrate legacy JSON files into the redb store.
    ///
    /// If `endpoints.json` or `credentials.json` exist, import their contents
    /// and rename the old files to `.json.migrated`.
    fn migrate_legacy_json(&mut self) -> Result<()> {
        let endpoints_path = self.data_dir.join("endpoints.json");
        let credentials_path = self.data_dir.join("credentials.json");

        if endpoints_path.exists() {
            tracing::info!("Migrating legacy endpoints.json to redb");
            let file = File::open(&endpoints_path)?;
            let reader = BufReader::new(file);
            let endpoints: Vec<Endpoint> = serde_json::from_reader(reader)
                .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;

            for endpoint in &endpoints {
                let json = serde_json::to_vec(endpoint)
                    .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
                self.store
                    .put(tables::ENDPOINTS, &endpoint.id.to_string(), &json)
                    .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
            }

            fs::rename(
                &endpoints_path,
                self.data_dir.join("endpoints.json.migrated"),
            )?;
        }

        if credentials_path.exists() {
            tracing::info!("Migrating legacy credentials.json to redb");
            let file = File::open(&credentials_path)?;
            let reader = BufReader::new(file);
            let credentials: Vec<Credential> = serde_json::from_reader(reader)
                .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;

            for credential in &credentials {
                let json = serde_json::to_vec(credential)
                    .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
                self.store
                    .put(tables::CREDENTIALS, &credential.id.to_string(), &json)
                    .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
            }

            fs::rename(
                &credentials_path,
                self.data_dir.join("credentials.json.migrated"),
            )?;
        }

        Ok(())
    }

    // === Store helpers ===

    fn store_get_endpoint(&self, id: Uuid) -> Result<Option<Endpoint>> {
        let bytes = self
            .store
            .get(tables::ENDPOINTS, &id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store get error: {e}")))?;
        match bytes {
            Some(b) => {
                let endpoint: Endpoint = serde_json::from_slice(&b)
                    .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;
                Ok(Some(endpoint))
            }
            None => Ok(None),
        }
    }

    fn store_put_endpoint(&self, endpoint: &Endpoint) -> Result<()> {
        let json = serde_json::to_vec(endpoint)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        self.store
            .put(tables::ENDPOINTS, &endpoint.id.to_string(), &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    fn store_get_credential(&self, id: Uuid) -> Result<Option<Credential>> {
        let bytes = self
            .store
            .get(tables::CREDENTIALS, &id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store get error: {e}")))?;
        match bytes {
            Some(b) => {
                let credential: Credential = serde_json::from_slice(&b)
                    .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;
                Ok(Some(credential))
            }
            None => Ok(None),
        }
    }

    fn store_put_credential(&self, credential: &Credential) -> Result<()> {
        let json = serde_json::to_vec(credential)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        self.store
            .put(tables::CREDENTIALS, &credential.id.to_string(), &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    fn store_list_endpoints(&self) -> Result<Vec<Endpoint>> {
        let entries = self
            .store
            .list(tables::ENDPOINTS)
            .map_err(|e| CredentialError::Internal(format!("Store list error: {e}")))?;
        let mut endpoints = Vec::with_capacity(entries.len());
        for (_key, bytes) in entries {
            let endpoint: Endpoint = serde_json::from_slice(&bytes)
                .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;
            endpoints.push(endpoint);
        }
        Ok(endpoints)
    }

    fn store_list_credentials(&self) -> Result<Vec<Credential>> {
        let entries = self
            .store
            .list(tables::CREDENTIALS)
            .map_err(|e| CredentialError::Internal(format!("Store list error: {e}")))?;
        let mut credentials = Vec::with_capacity(entries.len());
        for (_key, bytes) in entries {
            let credential: Credential = serde_json::from_slice(&bytes)
                .map_err(|e| CredentialError::DeserializationError(e.to_string()))?;
            credentials.push(credential);
        }
        Ok(credentials)
    }

    fn store_get_node_key(&self, node_id: &str) -> Result<Option<RegisteredNodeKey>> {
        let bytes = self
            .store
            .get(tables::NODE_KEYS, node_id)
            .map_err(|e| CredentialError::Internal(format!("Store get error: {e}")))?;
        match bytes {
            Some(b) => {
                Ok(Some(serde_json::from_slice(&b).map_err(|e| {
                    CredentialError::DeserializationError(e.to_string())
                })?))
            }
            None => Ok(None),
        }
    }

    fn store_put_node_key(&self, node: &RegisteredNodeKey) -> Result<()> {
        let json = serde_json::to_vec(node)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        self.store
            .put(tables::NODE_KEYS, &node.node_id, &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    fn store_list_node_keys(&self) -> Result<Vec<RegisteredNodeKey>> {
        let entries = self
            .store
            .list(tables::NODE_KEYS)
            .map_err(|e| CredentialError::Internal(format!("Store list error: {e}")))?;
        entries
            .into_iter()
            .map(|(_, bytes)| {
                serde_json::from_slice(&bytes)
                    .map_err(|e| CredentialError::DeserializationError(e.to_string()))
            })
            .collect()
    }

    fn store_get_vault_object(&self, object_id: Uuid) -> Result<Option<VaultObjectRecord>> {
        let bytes = self
            .store
            .get(tables::VAULT_OBJECTS, &object_id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store get error: {e}")))?;
        match bytes {
            Some(b) => {
                Ok(Some(serde_json::from_slice(&b).map_err(|e| {
                    CredentialError::DeserializationError(e.to_string())
                })?))
            }
            None => Ok(None),
        }
    }

    fn store_put_vault_object(&self, object: &VaultObjectRecord) -> Result<()> {
        let json = serde_json::to_vec(object)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        self.store
            .put(tables::VAULT_OBJECTS, &object.id.to_string(), &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    fn store_list_vault_objects(&self) -> Result<Vec<VaultObjectRecord>> {
        let entries = self
            .store
            .list(tables::VAULT_OBJECTS)
            .map_err(|e| CredentialError::Internal(format!("Store list error: {e}")))?;
        entries
            .into_iter()
            .map(|(_, bytes)| {
                serde_json::from_slice(&bytes)
                    .map_err(|e| CredentialError::DeserializationError(e.to_string()))
            })
            .collect()
    }

    fn grant_table(
        kind: VaultGrantKind,
    ) -> rockbot_storage::TableDefinition<'static, &'static str, &'static [u8]> {
        match kind {
            VaultGrantKind::Provider => tables::VAULT_PROVIDER_GRANTS,
            VaultGrantKind::Node => tables::VAULT_NODE_GRANTS,
        }
    }

    fn grant_row_key(object_id: Uuid, recipient_node_id: &str) -> String {
        format!("{object_id}\0{recipient_node_id}")
    }

    fn store_get_grant(
        &self,
        object_id: Uuid,
        recipient_node_id: &str,
        kind: VaultGrantKind,
    ) -> Result<Option<VaultGrantRecord>> {
        let key = Self::grant_row_key(object_id, recipient_node_id);
        let bytes = self
            .store
            .get(Self::grant_table(kind), &key)
            .map_err(|e| CredentialError::Internal(format!("Store get error: {e}")))?;
        match bytes {
            Some(b) => {
                Ok(Some(serde_json::from_slice(&b).map_err(|e| {
                    CredentialError::DeserializationError(e.to_string())
                })?))
            }
            None => Ok(None),
        }
    }

    fn store_put_grant(&self, grant: &VaultGrantRecord) -> Result<()> {
        let json = serde_json::to_vec(grant)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        let key = Self::grant_row_key(grant.object_id, &grant.recipient_node_id);
        self.store
            .put(Self::grant_table(grant.grant_kind), &key, &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    fn store_list_grants(&self, kind: VaultGrantKind) -> Result<Vec<VaultGrantRecord>> {
        let entries = self
            .store
            .list(Self::grant_table(kind))
            .map_err(|e| CredentialError::Internal(format!("Store list error: {e}")))?;
        entries
            .into_iter()
            .map(|(_, bytes)| {
                serde_json::from_slice(&bytes)
                    .map_err(|e| CredentialError::DeserializationError(e.to_string()))
            })
            .collect()
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

        self.store_put_endpoint(&endpoint)?;
        Ok(endpoint)
    }

    /// Gets an endpoint by ID.
    pub fn get_endpoint(&self, id: Uuid) -> Result<Endpoint> {
        self.store_get_endpoint(id)?
            .ok_or(CredentialError::EndpointNotFound(id))
    }

    /// Gets an endpoint by name.
    pub fn get_endpoint_by_name(&self, name: &str) -> Option<Endpoint> {
        self.store_list_endpoints()
            .ok()?
            .into_iter()
            .find(|e| e.name == name)
    }

    /// Lists all endpoints.
    pub fn list_endpoints(&self) -> Vec<Endpoint> {
        self.store_list_endpoints().unwrap_or_default()
    }

    /// Updates an endpoint.
    pub fn update_endpoint(&mut self, endpoint: Endpoint) -> Result<()> {
        // Verify it exists
        let _ = self
            .store_get_endpoint(endpoint.id)?
            .ok_or(CredentialError::EndpointNotFound(endpoint.id))?;

        self.store_put_endpoint(&endpoint)?;
        Ok(())
    }

    /// Deletes an endpoint and its associated credential.
    pub fn delete_endpoint(&mut self, id: Uuid) -> Result<()> {
        let endpoint = self
            .store_get_endpoint(id)?
            .ok_or(CredentialError::EndpointNotFound(id))?;

        // Also remove associated credential
        if endpoint.credential_id != Uuid::nil() {
            self.store
                .delete(tables::CREDENTIALS, &endpoint.credential_id.to_string())
                .map_err(|e| CredentialError::Internal(format!("Store delete error: {e}")))?;
        }

        self.store
            .delete(tables::ENDPOINTS, &id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store delete error: {e}")))?;

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
        if let Ok(Some(mut endpoint)) = self.store_get_endpoint(endpoint_id) {
            endpoint.credential_id = credential.id;
            endpoint.updated_at = now;
            self.store_put_endpoint(&endpoint)?;
        }

        self.store_put_credential(&credential)?;
        Ok(credential)
    }

    /// Gets the raw (still encrypted) credential by ID.
    pub fn get_credential(&self, id: Uuid) -> Result<Credential> {
        self.store_get_credential(id)?
            .ok_or(CredentialError::CredentialNotFound(id))
    }

    /// Gets the credential for an endpoint.
    pub fn get_credential_for_endpoint(&self, endpoint_id: Uuid) -> Result<Credential> {
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

        let mut credential = self.get_credential(credential_id)?;

        // Encrypt new secret
        let nonce = generate_nonce();
        let encrypted_data = encrypt(master_key, &nonce, new_secret)?;

        credential.encrypted_data = hex_encode(&encrypted_data);
        credential.nonce = hex_encode(&nonce);
        credential.rotated_at = Some(Utc::now());

        self.store_put_credential(&credential)?;
        Ok(())
    }

    /// Deletes a credential.
    pub fn delete_credential(&mut self, id: Uuid) -> Result<()> {
        let credential = self
            .store_get_credential(id)?
            .ok_or(CredentialError::CredentialNotFound(id))?;

        // Clear the credential reference from the endpoint
        if let Ok(Some(mut endpoint)) = self.store_get_endpoint(credential.endpoint_id) {
            endpoint.credential_id = Uuid::nil();
            endpoint.updated_at = Utc::now();
            self.store_put_endpoint(&endpoint)?;
        }

        self.store
            .delete(tables::CREDENTIALS, &id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store delete error: {e}")))?;

        Ok(())
    }

    /// Lists all credentials.
    pub fn list_credentials(&self) -> Vec<Credential> {
        self.store_list_credentials().unwrap_or_default()
    }

    // === Distributed Vault Operations ===

    /// Register or update a node's vault public key and role metadata.
    pub fn register_node_key(&self, node: RegisteredNodeKey) -> Result<()> {
        self.store_put_node_key(&node)
    }

    /// Get a registered node key record.
    pub fn get_registered_node_key(&self, node_id: &str) -> Result<RegisteredNodeKey> {
        self.store_get_node_key(node_id)?.ok_or_else(|| {
            CredentialError::ValidationFailed(format!("node '{node_id}' is not registered"))
        })
    }

    /// List all registered node key records.
    pub fn list_registered_node_keys(&self) -> Vec<RegisteredNodeKey> {
        self.store_list_node_keys().unwrap_or_default()
    }

    /// Create a logical distributed vault object. Secret material is shared via grants.
    pub fn create_vault_object(
        &self,
        namespace: String,
        name: String,
        description: Option<String>,
        created_by: Option<String>,
    ) -> Result<VaultObjectRecord> {
        let now = Utc::now();
        let object = VaultObjectRecord {
            id: Uuid::new_v4(),
            namespace,
            name,
            description,
            created_by,
            created_at: now,
            updated_at: now,
            version: 1,
        };
        self.store_put_vault_object(&object)?;
        Ok(object)
    }

    /// List distributed vault objects.
    pub fn list_vault_objects(&self) -> Vec<VaultObjectRecord> {
        self.store_list_vault_objects().unwrap_or_default()
    }

    /// Issue a per-recipient encrypted grant for an existing object.
    pub fn issue_vault_grant(
        &self,
        object_id: Uuid,
        recipient_node_id: &str,
        issued_by: Option<String>,
        kind: VaultGrantKind,
        plaintext_secret: &[u8],
    ) -> Result<VaultGrantRecord> {
        let object = self.store_get_vault_object(object_id)?.ok_or_else(|| {
            CredentialError::ValidationFailed(format!("vault object '{object_id}' not found"))
        })?;
        let node = self.get_registered_node_key(recipient_node_id)?;
        let ciphertext = encrypt_for_age_recipient(plaintext_secret, &node.vault_public_key)?;
        let grant = VaultGrantRecord {
            object_id,
            recipient_node_id: recipient_node_id.to_string(),
            issued_by,
            grant_kind: kind,
            algorithm: "age_x25519".to_string(),
            key_id: Some(node.vault_public_key.clone()),
            ciphertext,
            version: object.version,
            created_at: Utc::now(),
        };
        self.store_put_grant(&grant)?;
        Ok(grant)
    }

    /// Get a vault grant for a specific object and recipient.
    pub fn get_vault_grant(
        &self,
        object_id: Uuid,
        recipient_node_id: &str,
        kind: VaultGrantKind,
    ) -> Result<VaultGrantRecord> {
        self.store_get_grant(object_id, recipient_node_id, kind)?
            .ok_or_else(|| CredentialError::ValidationFailed("vault grant not found".to_string()))
    }

    /// List grants by type.
    pub fn list_vault_grants(&self, kind: VaultGrantKind) -> Vec<VaultGrantRecord> {
        self.store_list_grants(kind).unwrap_or_default()
    }

    /// Decrypt a vault grant with the recipient Age identity.
    pub fn decrypt_vault_grant(
        &self,
        object_id: Uuid,
        recipient_node_id: &str,
        kind: VaultGrantKind,
        age_identity: &str,
    ) -> Result<Vec<u8>> {
        let grant = self.get_vault_grant(object_id, recipient_node_id, kind)?;
        decrypt_with_age_identity(&grant.ciphertext, age_identity)
    }

    // === KV Store Operations ===

    /// Store arbitrary data in the generic KV store.
    pub fn kv_put(&self, namespace: &str, key: &str, value: &[u8]) -> Result<()> {
        self.store
            .kv_put(namespace, key, value)
            .map_err(|e| CredentialError::Internal(format!("KV put error: {e}")))
    }

    /// Retrieve data from the generic KV store.
    pub fn kv_get(&self, namespace: &str, key: &str) -> Result<Option<Vec<u8>>> {
        self.store
            .kv_get(namespace, key)
            .map_err(|e| CredentialError::Internal(format!("KV get error: {e}")))
    }

    /// Delete data from the generic KV store.
    pub fn kv_delete(&self, namespace: &str, key: &str) -> Result<()> {
        self.store
            .kv_delete(namespace, key)
            .map_err(|e| CredentialError::Internal(format!("KV delete error: {e}")))?;
        Ok(())
    }

    /// List all keys in a KV store namespace.
    pub fn kv_list(&self, namespace: &str) -> Vec<String> {
        self.store
            .kv_list(namespace)
            .unwrap_or_default()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    // === Permission Persistence ===

    /// Store a permission rule in the vault.
    pub fn store_permission(&self, permission: &crate::types::Permission) -> Result<()> {
        let json = serde_json::to_vec(permission)
            .map_err(|e| CredentialError::SerializationError(e.to_string()))?;
        self.store
            .put(tables::PERMISSIONS, &permission.id.to_string(), &json)
            .map_err(|e| CredentialError::Internal(format!("Store put error: {e}")))?;
        Ok(())
    }

    /// Delete a permission rule from the vault.
    pub fn delete_permission(&self, id: Uuid) -> Result<bool> {
        self.store
            .delete(tables::PERMISSIONS, &id.to_string())
            .map_err(|e| CredentialError::Internal(format!("Store delete error: {e}")))
    }

    /// List all stored permission rules.
    pub fn list_permissions(&self) -> Vec<crate::types::Permission> {
        let entries = self.store.list(tables::PERMISSIONS).unwrap_or_default();
        entries
            .into_iter()
            .filter_map(|(_key, bytes)| serde_json::from_slice(&bytes).ok())
            .collect()
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

    #[test]
    fn test_kv_operations() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let vault = CredentialVault::open(dir.path()).unwrap();

        // KV operations don't require unlock (they're for non-secret data)
        vault.kv_put("test-ns", "key1", b"value1").unwrap();
        vault.kv_put("test-ns", "key2", b"value2").unwrap();
        vault.kv_put("other-ns", "key1", b"other-value").unwrap();

        let val = vault.kv_get("test-ns", "key1").unwrap();
        assert_eq!(val.as_deref(), Some(b"value1".as_ref()));

        let keys = vault.kv_list("test-ns");
        assert_eq!(keys.len(), 2);

        vault.kv_delete("test-ns", "key1").unwrap();
        let val = vault.kv_get("test-ns", "key1").unwrap();
        assert!(val.is_none());

        // Other namespace unaffected
        let val = vault.kv_get("other-ns", "key1").unwrap();
        assert_eq!(val.as_deref(), Some(b"other-value".as_ref()));
    }

    #[test]
    fn test_permission_persistence() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let vault = CredentialVault::open(dir.path()).unwrap();

        let perm = crate::types::Permission {
            id: Uuid::new_v4(),
            endpoint_id: Uuid::new_v4(),
            path_pattern: "/api/**".to_string(),
            method: None,
            permission_level: crate::types::PermissionLevel::Allow,
            created_at: Utc::now(),
        };

        vault.store_permission(&perm).unwrap();

        let perms = vault.list_permissions();
        assert_eq!(perms.len(), 1);
        assert_eq!(perms[0].id, perm.id);
        assert_eq!(perms[0].path_pattern, "/api/**");

        vault.delete_permission(perm.id).unwrap();
        let perms = vault.list_permissions();
        assert!(perms.is_empty());
    }

    #[test]
    fn test_legacy_json_migration() {
        let dir = tempdir().unwrap();

        // Create a vault first (for meta.json)
        CredentialVault::init_with_password(dir.path(), "test").unwrap();

        // Write legacy JSON files
        let endpoint = Endpoint {
            id: Uuid::new_v4(),
            name: "Legacy Endpoint".to_string(),
            endpoint_type: EndpointType::GenericRest,
            base_url: "http://legacy.test".to_string(),
            credential_id: Uuid::nil(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let endpoints_json = serde_json::to_string_pretty(&vec![&endpoint]).unwrap();
        fs::write(dir.path().join("endpoints.json"), &endpoints_json).unwrap();

        // Remove the vault.db so a fresh open triggers migration
        let _ = fs::remove_file(dir.path().join("vault.db"));

        // Reopen — should migrate
        let vault = CredentialVault::open(dir.path()).unwrap();

        // Legacy files should be renamed
        assert!(!dir.path().join("endpoints.json").exists());
        assert!(dir.path().join("endpoints.json.migrated").exists());

        // Data should be accessible via the store
        let endpoints = vault.list_endpoints();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0].name, "Legacy Endpoint");
    }

    #[test]
    fn test_distributed_vault_node_registration_and_grants() {
        use age::secrecy::ExposeSecret;

        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();
        let vault = CredentialVault::open(dir.path()).unwrap();

        let provider_identity = age::x25519::Identity::generate();
        let node_identity = age::x25519::Identity::generate();

        vault
            .register_node_key(RegisteredNodeKey {
                node_id: "provider-a".to_string(),
                identity_fingerprint: Some("fp-provider".to_string()),
                vault_public_key: provider_identity.to_public().to_string(),
                roles: vec![crate::types::ClusterNodeRole::VaultProvider],
                active: true,
                created_at: Utc::now(),
                rotated_at: None,
                revoked_at: None,
            })
            .unwrap();
        vault
            .register_node_key(RegisteredNodeKey {
                node_id: "node-b".to_string(),
                identity_fingerprint: Some("fp-node".to_string()),
                vault_public_key: node_identity.to_public().to_string(),
                roles: vec![crate::types::ClusterNodeRole::Client],
                active: true,
                created_at: Utc::now(),
                rotated_at: None,
                revoked_at: None,
            })
            .unwrap();

        let object = vault
            .create_vault_object(
                "prod".to_string(),
                "anthropic_api_key".to_string(),
                Some("Primary Anthropic credential".to_string()),
                Some("provider-a".to_string()),
            )
            .unwrap();

        let provider_grant = vault
            .issue_vault_grant(
                object.id,
                "provider-a",
                Some("provider-a".to_string()),
                VaultGrantKind::Provider,
                b"secret-value",
            )
            .unwrap();
        assert_eq!(provider_grant.grant_kind, VaultGrantKind::Provider);

        let node_grant = vault
            .issue_vault_grant(
                object.id,
                "node-b",
                Some("provider-a".to_string()),
                VaultGrantKind::Node,
                b"secret-value",
            )
            .unwrap();
        assert_eq!(node_grant.recipient_node_id, "node-b");

        let decrypted = vault
            .decrypt_vault_grant(
                object.id,
                "node-b",
                VaultGrantKind::Node,
                node_identity.to_string().expose_secret(),
            )
            .unwrap();
        assert_eq!(decrypted, b"secret-value");
    }

    #[test]
    fn test_open_repairs_stale_vault_volume_from_legacy_file() {
        let dir = tempdir().unwrap();
        CredentialVault::init_with_password(dir.path(), "test").unwrap();

        let legacy_path = dir.path().join("vault.db");
        let disk_path = CredentialVault::disk_path_for_dir(dir.path());
        rockbot_vdisk::materialize_file(&disk_path, "vault", &legacy_path, None).unwrap();
        let legacy_bytes = fs::read(&legacy_path).unwrap();

        rockbot_vdisk::import_bytes(&disk_path, "vault", b"bad!", None).unwrap();
        let info_before = rockbot_vdisk::volume_info(&disk_path, "vault")
            .unwrap()
            .expect("vault volume exists");
        assert_ne!(info_before.len, legacy_bytes.len() as u64);

        let _vault = CredentialVault::open(dir.path()).unwrap();

        let info_after = rockbot_vdisk::volume_info(&disk_path, "vault")
            .unwrap()
            .expect("vault volume exists after repair");
        assert_eq!(info_after.len, legacy_bytes.len() as u64);
        let prefix = rockbot_vdisk::read_volume_prefix(&disk_path, "vault", 4)
            .unwrap()
            .expect("vault volume prefix");
        assert_eq!(prefix.as_slice(), b"redb");
    }
}
