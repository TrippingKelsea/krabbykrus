//! Self-learning store for doctor fixes.
//!
//! Remembers verified fixes so future identical errors can be resolved
//! without invoking the LLM. Uses plain JSONL (not encrypted) since
//! doctor learned data is not sensitive.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Hex SHA-256 hash identifying an error+field combination.
pub type ErrorFingerprint = String;

/// A recorded successful fix, stored in the learned store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedFix {
    pub fingerprint: ErrorFingerprint,
    pub diagnosis_kind: String,
    pub field_pattern: String,
    pub fix_description: String,
    /// JSON-encoded `DoctorFix` for deserialization.
    pub fix_serialized: String,
    pub recorded_at: DateTime<Utc>,
    pub apply_count: u32,
}

/// Persistent store of learned fixes, backed by a JSONL file.
pub struct LearnedStore {
    path: PathBuf,
    entries: Vec<LearnedFix>,
}

impl LearnedStore {
    /// Open (or create) the learned store at the standard data path.
    ///
    /// Path: `{data_local_dir}/rockbot/doctor/learned.jsonl`
    pub fn open() -> anyhow::Result<Self> {
        let path = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rockbot")
            .join("doctor")
            .join("learned.jsonl");

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let entries = if path.exists() {
            let file = std::fs::File::open(&path)?;
            let reader = std::io::BufReader::new(file);
            let mut entries = Vec::new();
            for line in reader.lines() {
                let line = line?;
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<LearnedFix>(line) {
                    Ok(entry) => entries.push(entry),
                    Err(e) => {
                        tracing::warn!("Skipping malformed learned fix entry: {e}");
                    }
                }
            }
            entries
        } else {
            Vec::new()
        };

        Ok(Self { path, entries })
    }

    /// Compute a deterministic fingerprint for an error+field combination.
    ///
    /// SHA-256 hex of `"{error}\0{field_path}"`.
    pub fn fingerprint(error: &str, field_path: &str) -> ErrorFingerprint {
        let mut hasher = Sha256::new();
        hasher.update(error.as_bytes());
        hasher.update(b"\0");
        hasher.update(field_path.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Look up a previously learned fix by fingerprint.
    pub fn lookup(&self, fingerprint: &str) -> Option<&LearnedFix> {
        self.entries.iter().find(|e| e.fingerprint == fingerprint)
    }

    /// Record a fix. If a matching fingerprint already exists, increments
    /// `apply_count` rather than adding a duplicate.
    pub fn record(&mut self, fix: LearnedFix) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.fingerprint == fix.fingerprint)
        {
            existing.apply_count += 1;
            existing.recorded_at = Utc::now();
        } else {
            self.entries.push(fix);
        }
    }

    /// Return the last `n` entries for few-shot prompting.
    pub fn recent_examples(&self, n: usize) -> Vec<&LearnedFix> {
        let start = self.entries.len().saturating_sub(n);
        self.entries[start..].iter().collect()
    }

    /// Write all entries to the JSONL file (overwrites existing content).
    pub fn save(&self) -> anyhow::Result<()> {
        let parent = self
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        for entry in &self.entries {
            let line = serde_json::to_string(entry)?;
            writeln!(file, "{line}")?;
        }
        file.as_file().sync_all()?;
        file.persist(&self.path)?;
        Ok(())
    }

    /// Number of stored learned fixes.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns true if the store has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tempfile::TempDir;

    fn make_store(dir: &TempDir) -> LearnedStore {
        let path = dir.path().join("learned.jsonl");
        LearnedStore {
            path,
            entries: Vec::new(),
        }
    }

    fn sample_fix(fingerprint: &str) -> LearnedFix {
        LearnedFix {
            fingerprint: fingerprint.to_string(),
            diagnosis_kind: "wrong_type".to_string(),
            field_pattern: "gateway.port".to_string(),
            fix_description: "Set `gateway.port` = 18080".to_string(),
            fix_serialized: r#"{"SetField":{"path":["gateway","port"],"new_value":"18080"}}"#
                .to_string(),
            recorded_at: Utc::now(),
            apply_count: 1,
        }
    }

    #[test]
    fn test_fingerprint_determinism() {
        let a = LearnedStore::fingerprint("invalid type", "gateway.port");
        let b = LearnedStore::fingerprint("invalid type", "gateway.port");
        let c = LearnedStore::fingerprint("invalid type", "gateway.host");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_lookup() {
        let dir = TempDir::new().unwrap();
        let mut store = make_store(&dir);
        let fp = "abc123".to_string();
        store.entries.push(sample_fix(&fp));

        assert!(store.lookup(&fp).is_some());
        assert!(store.lookup("not_there").is_none());
    }

    #[test]
    fn test_record_new_entry() {
        let dir = TempDir::new().unwrap();
        let mut store = make_store(&dir);
        assert_eq!(store.len(), 0);

        store.record(sample_fix("fp1"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_record_increments_apply_count() {
        let dir = TempDir::new().unwrap();
        let mut store = make_store(&dir);
        store.record(sample_fix("fp1"));
        assert_eq!(store.lookup("fp1").unwrap().apply_count, 1);

        store.record(sample_fix("fp1"));
        assert_eq!(store.len(), 1);
        assert_eq!(store.lookup("fp1").unwrap().apply_count, 2);
    }

    #[test]
    fn test_save_and_reload() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("learned.jsonl");

        let mut store = LearnedStore {
            path: path.clone(),
            entries: Vec::new(),
        };
        store.record(sample_fix("fp1"));
        store.record(sample_fix("fp2"));
        store.save().unwrap();

        let reloaded = LearnedStore {
            path: path.clone(),
            entries: {
                let file = std::fs::File::open(&path).unwrap();
                let reader = std::io::BufReader::new(file);
                reader
                    .lines()
                    .filter_map(|l| l.ok())
                    .filter(|l| !l.trim().is_empty())
                    .filter_map(|l| serde_json::from_str::<LearnedFix>(&l).ok())
                    .collect()
            },
        };

        assert_eq!(reloaded.len(), 2);
        assert!(reloaded.lookup("fp1").is_some());
        assert!(reloaded.lookup("fp2").is_some());
    }

    #[test]
    fn test_recent_examples() {
        let dir = TempDir::new().unwrap();
        let mut store = make_store(&dir);
        for i in 0..5u32 {
            store.record(sample_fix(&format!("fp{i}")));
        }
        let recent = store.recent_examples(3);
        assert_eq!(recent.len(), 3);
        // Should be the last 3
        assert_eq!(recent[0].fingerprint, "fp2");
        assert_eq!(recent[2].fingerprint, "fp4");
    }
}
