//! Key backend abstraction: file-backed PEM keys today, HSM/YubiKey tomorrow.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Opaque handle to a key pair managed by a backend.
pub enum KeyHandle {
    /// In-memory key pair loaded from a PEM file on disk.
    File {
        key_pair: rcgen::KeyPair,
        path: PathBuf,
    },
    /// Placeholder for hardware-backed keys (PKCS#11, YubiKey, cloud KMS).
    /// The string is an opaque locator (e.g. slot ID, key label).
    Hardware {
        locator: String,
        backend_name: String,
    },
}

impl KeyHandle {
    /// Return a reference to the underlying `rcgen::KeyPair`.
    ///
    /// Returns an error for hardware-backed handles, which are not yet implemented.
    pub fn key_pair(&self) -> anyhow::Result<&rcgen::KeyPair> {
        match self {
            KeyHandle::File { key_pair, .. } => Ok(key_pair),
            KeyHandle::Hardware { .. } => {
                anyhow::bail!("Hardware key backends not yet supported")
            }
        }
    }

    /// Return the path to the key file, if this is a file-backed handle.
    pub fn path(&self) -> Option<&Path> {
        match self {
            KeyHandle::File { path, .. } => Some(path),
            KeyHandle::Hardware { .. } => None,
        }
    }
}

/// Trait abstracting private key operations.
///
/// The default implementation is [`FileBackend`], which stores PEM-encoded keys on disk.
/// Future implementations might target PKCS#11 (HSM), PIV (YubiKey), or cloud KMS services.
pub trait KeyBackend: Send + Sync {
    /// Human-readable backend name ("file", "pkcs11", "yubikey", …).
    fn name(&self) -> &str;

    /// Generate a new key pair, persist it under `label`, and return a handle.
    fn generate(&self, label: &str) -> anyhow::Result<KeyHandle>;

    /// Load an existing key pair from `path` and return a handle.
    fn load(&self, path: &Path) -> anyhow::Result<KeyHandle>;
}

/// File-system backed key storage.
///
/// Keys are written as PEM files with `0600` permissions so that only the
/// owning user can read them.
pub struct FileBackend {
    base_dir: PathBuf,
}

impl FileBackend {
    /// Create a new `FileBackend` that stores keys under `base_dir`.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

impl KeyBackend for FileBackend {
    fn name(&self) -> &str {
        "file"
    }

    fn generate(&self, label: &str) -> anyhow::Result<KeyHandle> {
        let key_pair = rcgen::KeyPair::generate()?;
        let pem = key_pair.serialize_pem();

        let path = self.base_dir.join(format!("{label}.key"));
        fs::write(&path, &pem)?;

        // Restrict permissions to owner-read/write only (0600)
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;

        tracing::debug!(path = %path.display(), "Generated key pair");

        // Re-parse so we own a fresh KeyPair that is not consumed
        let key_pair = rcgen::KeyPair::from_pem(&pem)?;
        Ok(KeyHandle::File { key_pair, path })
    }

    fn load(&self, path: &Path) -> anyhow::Result<KeyHandle> {
        let pem = fs::read_to_string(path)?;
        let key_pair = rcgen::KeyPair::from_pem(&pem)?;
        Ok(KeyHandle::File {
            key_pair,
            path: path.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_file_backend_generate_and_load() {
        let dir = TempDir::new().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf());

        let handle = backend.generate("test-key").unwrap();
        let key_path = handle.path().unwrap().to_path_buf();

        // File should exist with restricted permissions
        let meta = fs::metadata(&key_path).unwrap();
        assert!(meta.is_file());
        let mode = meta.permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "Key file permissions should be 0600");

        // Public key bytes from the original handle
        let pubkey_original = handle.key_pair().unwrap().public_key_der();

        // Load back and verify same key
        let loaded = backend.load(&key_path).unwrap();
        let pubkey_loaded = loaded.key_pair().unwrap().public_key_der();

        assert_eq!(
            pubkey_original, pubkey_loaded,
            "Loaded key should match generated key"
        );
    }

    #[test]
    fn test_hardware_handle_returns_error() {
        let handle = KeyHandle::Hardware {
            locator: "slot:1".to_string(),
            backend_name: "pkcs11".to_string(),
        };
        assert!(handle.key_pair().is_err());
        assert!(handle.path().is_none());
    }
}
