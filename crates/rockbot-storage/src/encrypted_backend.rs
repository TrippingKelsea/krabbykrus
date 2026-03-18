use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20;
use redb::StorageBackend;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;

/// A redb `StorageBackend` that transparently encrypts data using
/// ChaCha20 (stream cipher, no length expansion).
///
/// Each file offset maps 1:1 to the same offset in the ciphertext,
/// making random-access reads and writes consistent: the same (key,
/// offset) pair always produces the same keystream bytes.
pub struct EncryptedBackend {
    inner: Mutex<File>,
    key: [u8; 32],
}

impl fmt::Debug for EncryptedBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncryptedBackend")
            .field("key", &"[redacted]")
            .finish()
    }
}

impl EncryptedBackend {
    /// Open (or create) an encrypted file at `path` with the given 32-byte key.
    pub fn open(path: &Path, key: [u8; 32]) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Ok(Self {
            inner: Mutex::new(file),
            key,
        })
    }

    /// Derive a deterministic 96-bit nonce from a byte offset.
    ///
    /// ChaCha20 has a 64-bit block counter, so different offsets within
    /// the same "nonce block" produce different keystream segments.
    /// We encode the 64-bit offset in the first 8 bytes and leave the
    /// remaining 4 bytes zero.
    fn nonce_for_offset(offset: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&offset.to_le_bytes());
        nonce
    }

    /// XOR `data` in place with the keystream for `offset`.
    fn apply_keystream(&self, offset: u64, data: &mut [u8]) {
        if data.is_empty() {
            return;
        }
        let nonce = Self::nonce_for_offset(offset);
        let mut cipher = ChaCha20::new(
            chacha20::Key::from_slice(&self.key),
            chacha20::Nonce::from_slice(&nonce),
        );
        cipher.apply_keystream(data);
    }
}

impl StorageBackend for EncryptedBackend {
    fn len(&self) -> Result<u64, io::Error> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        Ok(guard.metadata()?.len())
    }

    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, io::Error> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        guard.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        guard.read_exact(&mut buf)?;
        drop(guard);
        self.apply_keystream(offset, &mut buf);
        Ok(buf)
    }

    fn set_len(&self, len: u64) -> Result<(), io::Error> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        guard.set_len(len)
    }

    fn sync_data(&self, _eventual: bool) -> Result<(), io::Error> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        guard.sync_data()
    }

    fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
        let mut encrypted = data.to_vec();
        self.apply_keystream(offset, &mut encrypted);
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("mutex poisoned"))?;
        guard.seek(SeekFrom::Start(offset))?;
        guard.write_all(&encrypted)
    }
}
