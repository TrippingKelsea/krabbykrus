//! Hash-chained audit log for rockbot-credentials.
//!
//! Provides tamper-evident logging using SHA-256 hash chains.
//! Each entry's hash includes the previous entry's hash, creating
//! an immutable chain that detects any modifications.
//!
//! # Storage Format
//!
//! JSON Lines format: one JSON object per line, append-only.
//! File: `$XDG_DATA_HOME/rockbot/credentials/audit.log`

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{CredentialError, Result};
use crate::types::{
    hex_encode_hash, AuditEntry, Hash256, HttpMethod, PermissionLevel, ResultStatus, ZERO_HASH,
};

/// Audit log manager for writing and verifying entries.
pub struct AuditLog {
    path: PathBuf,
    last_hash: Hash256,
    next_sequence: u64,
}

impl AuditLog {
    /// Opens or creates an audit log at the specified path.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Read existing entries to get last hash and sequence
        let (last_hash, next_sequence) = if path.exists() {
            Self::read_last_entry(&path)?
        } else {
            (ZERO_HASH, 1)
        };

        Ok(Self {
            path,
            last_hash,
            next_sequence,
        })
    }

    /// Reads the last entry from the log to get its hash and sequence.
    fn read_last_entry(path: &Path) -> Result<(Hash256, u64)> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut last_hash = ZERO_HASH;
        let mut next_sequence = 1u64;

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let entry: AuditEntry = serde_json::from_str(&line)
                .map_err(|e| CredentialError::AuditReadFailed(format!("failed to parse entry: {}", e)))?;

            last_hash = entry
                .entry_hash_bytes()
                .map_err(|e| CredentialError::AuditReadFailed(format!("invalid hash: {}", e)))?;
            next_sequence = entry.sequence + 1;
        }

        Ok((last_hash, next_sequence))
    }

    /// Creates a new audit entry builder.
    pub fn new_entry(&self) -> AuditEntryBuilder {
        AuditEntryBuilder {
            sequence: self.next_sequence,
            previous_hash: self.last_hash,
            request_id: Uuid::new_v4(),
            source: String::new(),
            endpoint_id: Uuid::nil(),
            method: HttpMethod::Get,
            path: String::new(),
            parameters_hash: ZERO_HASH,
            permission_level: PermissionLevel::Deny,
            approval_id: None,
            result_status: ResultStatus::Success,
            result_hash: ZERO_HASH,
            error_message: None,
        }
    }

    /// Appends an entry to the audit log.
    pub fn append(&mut self, entry: AuditEntry) -> Result<()> {
        // Verify chain integrity
        let expected_prev = self.last_hash;
        let actual_prev = entry
            .previous_hash_bytes()
            .map_err(|e| CredentialError::AuditWriteFailed(format!("invalid previous hash: {}", e)))?;

        if actual_prev != expected_prev {
            return Err(CredentialError::AuditChainBroken(entry.sequence));
        }

        // Verify sequence
        if entry.sequence != self.next_sequence {
            return Err(CredentialError::AuditWriteFailed(format!(
                "expected sequence {}, got {}",
                self.next_sequence, entry.sequence
            )));
        }

        // Verify entry hash
        let computed_hash = compute_entry_hash(&entry);
        let stored_hash = entry
            .entry_hash_bytes()
            .map_err(|e| CredentialError::AuditWriteFailed(format!("invalid entry hash: {}", e)))?;

        if computed_hash != stored_hash {
            return Err(CredentialError::AuditWriteFailed(
                "entry hash does not match computed hash".to_string(),
            ));
        }

        // Append to file
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let json = serde_json::to_string(&entry)
            .map_err(|e| CredentialError::AuditWriteFailed(format!("failed to serialize: {}", e)))?;

        writeln!(file, "{}", json)?;
        file.sync_all()?;

        // Update state
        self.last_hash = stored_hash;
        self.next_sequence = entry.sequence + 1;

        Ok(())
    }

    /// Verifies the integrity of the entire audit log.
    pub fn verify(&self) -> Result<VerificationResult> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(VerificationResult {
                    valid: true,
                    entries_checked: 0,
                    last_sequence: 0,
                    error: None,
                });
            }
            Err(e) => return Err(e.into()),
        };

        let reader = BufReader::new(file);
        let mut previous_hash = ZERO_HASH;
        let mut entries_checked = 0u64;
        let mut last_sequence = 0u64;

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let entry: AuditEntry = match serde_json::from_str(&line) {
                Ok(e) => e,
                Err(e) => {
                    return Ok(VerificationResult {
                        valid: false,
                        entries_checked,
                        last_sequence,
                        error: Some(format!("line {}: parse error: {}", line_num + 1, e)),
                    });
                }
            };

            // Verify previous hash chain
            let entry_prev = match entry.previous_hash_bytes() {
                Ok(h) => h,
                Err(e) => {
                    return Ok(VerificationResult {
                        valid: false,
                        entries_checked,
                        last_sequence,
                        error: Some(format!("line {}: invalid previous hash: {}", line_num + 1, e)),
                    });
                }
            };

            if entry_prev != previous_hash {
                return Ok(VerificationResult {
                    valid: false,
                    entries_checked,
                    last_sequence,
                    error: Some(format!(
                        "line {}: chain broken at sequence {}",
                        line_num + 1,
                        entry.sequence
                    )),
                });
            }

            // Verify entry hash
            let computed_hash = compute_entry_hash(&entry);
            let stored_hash = match entry.entry_hash_bytes() {
                Ok(h) => h,
                Err(e) => {
                    return Ok(VerificationResult {
                        valid: false,
                        entries_checked,
                        last_sequence,
                        error: Some(format!("line {}: invalid entry hash: {}", line_num + 1, e)),
                    });
                }
            };

            if computed_hash != stored_hash {
                return Ok(VerificationResult {
                    valid: false,
                    entries_checked,
                    last_sequence,
                    error: Some(format!(
                        "line {}: entry hash mismatch at sequence {}",
                        line_num + 1,
                        entry.sequence
                    )),
                });
            }

            previous_hash = stored_hash;
            entries_checked += 1;
            last_sequence = entry.sequence;
        }

        Ok(VerificationResult {
            valid: true,
            entries_checked,
            last_sequence,
            error: None,
        })
    }

    /// Returns the path to the audit log file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the hash of the last entry.
    pub fn last_hash(&self) -> &Hash256 {
        &self.last_hash
    }

    /// Returns the next sequence number.
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

/// Result of audit log verification.
#[derive(Debug)]
pub struct VerificationResult {
    /// Whether the log is valid.
    pub valid: bool,
    /// Number of entries checked.
    pub entries_checked: u64,
    /// Last valid sequence number.
    pub last_sequence: u64,
    /// Error message if verification failed.
    pub error: Option<String>,
}

/// Builder for creating audit entries.
pub struct AuditEntryBuilder {
    sequence: u64,
    previous_hash: Hash256,
    request_id: Uuid,
    source: String,
    endpoint_id: Uuid,
    method: HttpMethod,
    path: String,
    parameters_hash: Hash256,
    permission_level: PermissionLevel,
    approval_id: Option<Uuid>,
    result_status: ResultStatus,
    result_hash: Hash256,
    error_message: Option<String>,
}

impl AuditEntryBuilder {
    /// Sets the request ID.
    pub fn request_id(mut self, id: Uuid) -> Self {
        self.request_id = id;
        self
    }

    /// Sets the source of the request.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    /// Sets the endpoint ID.
    pub fn endpoint_id(mut self, id: Uuid) -> Self {
        self.endpoint_id = id;
        self
    }

    /// Sets the HTTP method.
    pub fn method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    /// Sets the request path.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Sets the parameters hash.
    pub fn parameters_hash(mut self, hash: Hash256) -> Self {
        self.parameters_hash = hash;
        self
    }

    /// Sets the permission level.
    pub fn permission_level(mut self, level: PermissionLevel) -> Self {
        self.permission_level = level;
        self
    }

    /// Sets the approval ID.
    pub fn approval_id(mut self, id: Option<Uuid>) -> Self {
        self.approval_id = id;
        self
    }

    /// Sets the result status.
    pub fn result_status(mut self, status: ResultStatus) -> Self {
        self.result_status = status;
        self
    }

    /// Sets the result hash.
    pub fn result_hash(mut self, hash: Hash256) -> Self {
        self.result_hash = hash;
        self
    }

    /// Sets the error message.
    pub fn error_message(mut self, message: Option<String>) -> Self {
        self.error_message = message;
        self
    }

    /// Builds the audit entry with computed hash.
    pub fn build(self) -> AuditEntry {
        let mut entry = AuditEntry {
            sequence: self.sequence,
            timestamp: Utc::now(),
            request_id: self.request_id,
            source: self.source,
            endpoint_id: self.endpoint_id,
            method: self.method,
            path: self.path,
            parameters_hash: hex_encode_hash(&self.parameters_hash),
            permission_level: self.permission_level,
            approval_id: self.approval_id,
            result_status: self.result_status,
            result_hash: hex_encode_hash(&self.result_hash),
            error_message: self.error_message,
            previous_hash: hex_encode_hash(&self.previous_hash),
            entry_hash: String::new(), // Will be computed below
        };

        let hash = compute_entry_hash(&entry);
        entry.entry_hash = hex_encode_hash(&hash);

        entry
    }
}

/// Computes the hash of an audit entry per specification.
fn compute_entry_hash(entry: &AuditEntry) -> Hash256 {
    let mut hasher = Sha256::new();

    hasher.update(entry.sequence.to_le_bytes());
    hasher.update(entry.timestamp.to_rfc3339().as_bytes());
    hasher.update(entry.request_id.as_bytes());
    hasher.update(entry.source.as_bytes());
    hasher.update(entry.endpoint_id.as_bytes());
    hasher.update(entry.method.as_str().as_bytes());
    hasher.update(entry.path.as_bytes());
    hasher.update(entry.parameters_hash.as_bytes());
    hasher.update(entry.permission_level.as_str().as_bytes());

    if let Some(ref approval_id) = entry.approval_id {
        hasher.update(approval_id.as_bytes());
    }

    hasher.update(entry.result_status.as_str().as_bytes());
    hasher.update(entry.result_hash.as_bytes());

    if let Some(ref error) = entry.error_message {
        hasher.update(error.as_bytes());
    }

    hasher.update(entry.previous_hash.as_bytes());

    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_audit_log_create_and_append() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let mut log = AuditLog::open(&path).unwrap();
        assert_eq!(log.next_sequence(), 1);

        let entry = log
            .new_entry()
            .source("test")
            .endpoint_id(Uuid::new_v4())
            .method(HttpMethod::Get)
            .path("/api/test")
            .result_status(ResultStatus::Success)
            .build();

        log.append(entry).unwrap();
        assert_eq!(log.next_sequence(), 2);

        // Verify file was written
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"sequence\":1"));
    }

    #[test]
    fn test_audit_log_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // Create and write first entry
        {
            let mut log = AuditLog::open(&path).unwrap();
            let entry = log
                .new_entry()
                .source("test")
                .method(HttpMethod::Get)
                .path("/test")
                .build();
            log.append(entry).unwrap();
        }

        // Reopen and continue
        {
            let mut log = AuditLog::open(&path).unwrap();
            assert_eq!(log.next_sequence(), 2);

            let entry = log
                .new_entry()
                .source("test2")
                .method(HttpMethod::Post)
                .path("/test2")
                .build();
            log.append(entry).unwrap();
            assert_eq!(log.next_sequence(), 3);
        }
    }

    #[test]
    fn test_audit_log_verify() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let mut log = AuditLog::open(&path).unwrap();

        // Add several entries
        for i in 0..5 {
            let entry = log
                .new_entry()
                .source(format!("test{}", i))
                .method(HttpMethod::Get)
                .path(format!("/test/{}", i))
                .build();
            log.append(entry).unwrap();
        }

        // Verify
        let result = log.verify().unwrap();
        assert!(result.valid);
        assert_eq!(result.entries_checked, 5);
        assert_eq!(result.last_sequence, 5);
    }

    #[test]
    fn test_audit_log_detects_tampering() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        // Create valid log
        {
            let mut log = AuditLog::open(&path).unwrap();
            for i in 0..3 {
                let entry = log
                    .new_entry()
                    .source(format!("test{}", i))
                    .method(HttpMethod::Get)
                    .path(format!("/test/{}", i))
                    .build();
                log.append(entry).unwrap();
            }
        }

        // Tamper with the file
        let content = fs::read_to_string(&path).unwrap();
        let tampered = content.replace("test1", "tampered");
        fs::write(&path, tampered).unwrap();

        // Verify should fail
        let log = AuditLog::open(&path).unwrap();
        let result = log.verify().unwrap();
        assert!(!result.valid);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_chain_prevents_out_of_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");

        let mut log = AuditLog::open(&path).unwrap();

        let entry = log
            .new_entry()
            .source("test")
            .method(HttpMethod::Get)
            .path("/test")
            .build();
        log.append(entry).unwrap();

        // Try to append entry with wrong sequence (create from scratch)
        let wrong_entry = AuditEntryBuilder {
            sequence: 5, // Wrong!
            previous_hash: *log.last_hash(),
            request_id: Uuid::new_v4(),
            source: "test".to_string(),
            endpoint_id: Uuid::nil(),
            method: HttpMethod::Get,
            path: "/test".to_string(),
            parameters_hash: ZERO_HASH,
            permission_level: PermissionLevel::Allow,
            approval_id: None,
            result_status: ResultStatus::Success,
            result_hash: ZERO_HASH,
            error_message: None,
        }
        .build();

        let result = log.append(wrong_entry);
        assert!(result.is_err());
    }
}
