//! Persistent certificate registry — stored as `index.json` in the PKI directory.

use std::fmt;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

/// The role a certificate is issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertRole {
    /// Gateway TLS server certificate (ServerAuth + ClientAuth EKU).
    Gateway,
    /// Agent client certificate (ClientAuth EKU only).
    Agent,
    /// TUI client certificate (ClientAuth EKU only).
    Tui,
}

impl fmt::Display for CertRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertRole::Gateway => write!(f, "gateway"),
            CertRole::Agent => write!(f, "agent"),
            CertRole::Tui => write!(f, "tui"),
        }
    }
}

impl CertRole {
    /// Parse a role from a string. Returns `None` for unrecognized values.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gateway" => Some(Self::Gateway),
            "agent" => Some(Self::Agent),
            "tui" => Some(Self::Tui),
            _ => None,
        }
    }
}

/// Lifecycle status of a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertStatus {
    /// Certificate is valid and in active use.
    Active,
    /// Certificate has been explicitly revoked.
    Revoked,
    /// Certificate has passed its `not_after` date.
    Expired,
}

impl fmt::Display for CertStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CertStatus::Active => write!(f, "active"),
            CertStatus::Revoked => write!(f, "revoked"),
            CertStatus::Expired => write!(f, "expired"),
        }
    }
}

/// A single entry in the certificate index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertEntry {
    /// Monotonically increasing serial number assigned at issuance.
    pub serial: u64,
    /// Human-readable name / CN for this certificate.
    pub name: String,
    /// The intended role of this certificate.
    pub role: CertRole,
    /// Current lifecycle status.
    pub status: CertStatus,
    /// Validity start.
    pub not_before: DateTime<Utc>,
    /// Validity end.
    pub not_after: DateTime<Utc>,
    /// Colon-separated uppercase hex SHA-256 fingerprint of the DER certificate.
    pub fingerprint_sha256: String,
    /// Subject distinguished name as a string (for display).
    pub subject: String,
    /// Subject alternative names included in the certificate.
    pub sans: Vec<String>,
    /// Authorization roles embedded in the certificate via x.509 extension.
    /// Nebula-inspired: the cert is the single source of truth for authorization.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Group memberships embedded in the certificate via x.509 extension.
    #[serde(default)]
    pub groups: Vec<String>,
}

/// A one-time (or limited-use) enrollment token used by agents/TUIs to obtain a certificate
/// without requiring manual CA interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentToken {
    /// Unique token identifier (UUID).
    pub id: String,
    /// The secret token value presented by the enrolling party.
    pub token: String,
    /// Maximum remaining uses; `None` means unlimited.
    pub remaining_uses: Option<u32>,
    /// Optional expiry; `None` means the token never expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// When the token was created.
    pub created_at: DateTime<Utc>,
    /// The role that will be assigned to certificates issued via this token.
    pub role: CertRole,
    /// Additional authorization roles embedded in issued certificates.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Persistent registry of all issued certificates and active enrollment tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkiIndex {
    /// Counter used to assign unique, monotonically increasing serial numbers.
    pub next_serial: u64,
    /// All certificates that have ever been issued.
    pub entries: Vec<CertEntry>,
    /// Active enrollment tokens.
    pub enrollments: Vec<EnrollmentToken>,
}

impl Default for PkiIndex {
    fn default() -> Self {
        Self {
            next_serial: 2,
            entries: Vec::new(),
            enrollments: Vec::new(),
        }
    }
}

impl PkiIndex {
    /// Load the index from `path`.  If the file does not exist a fresh default index is returned.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let json = std::fs::read_to_string(path)?;
        let index = serde_json::from_str(&json)?;
        Ok(index)
    }

    /// Persist the index to `path` as pretty-printed JSON.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Return the next serial number and advance the internal counter.
    pub fn next_serial(&mut self) -> u64 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    /// Append a new certificate entry to the registry.
    pub fn add_entry(&mut self, entry: CertEntry) {
        self.entries.push(entry);
    }

    /// Find a certificate entry by name (CN).  Returns the first match.
    pub fn find_by_name(&self, name: &str) -> Option<&CertEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Mark the most-recently issued certificate with the given name as revoked.
    ///
    /// Returns an error if no active certificate with that name exists.
    pub fn revoke(&mut self, name: &str) -> anyhow::Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| e.name == name && e.status == CertStatus::Active)
            .ok_or_else(|| anyhow::anyhow!("No active certificate found with name '{name}'"))?;
        entry.status = CertStatus::Revoked;
        Ok(())
    }

    /// Iterate over all active (non-revoked, non-expired) entries.
    pub fn active_entries(&self) -> impl Iterator<Item = &CertEntry> {
        self.entries
            .iter()
            .filter(|e| e.status == CertStatus::Active)
    }

    /// Append an enrollment token.
    pub fn add_enrollment(&mut self, token: EnrollmentToken) {
        self.enrollments.push(token);
    }

    /// Validate an enrollment token string for a given role.
    ///
    /// Checks:
    /// - Token exists and matches `role`.
    /// - Token has not expired.
    /// - Token has remaining uses (or is unlimited).
    ///
    /// On success the use count is decremented and exhausted tokens are removed.
    pub fn validate_enrollment(
        &mut self,
        token_str: &str,
        role: CertRole,
    ) -> anyhow::Result<EnrollmentToken> {
        let now = Utc::now();

        let pos = self
            .enrollments
            .iter()
            .enumerate()
            .find_map(|(idx, t)| {
                let lhs = t.token.as_bytes();
                let rhs = token_str.as_bytes();
                if lhs.len() == rhs.len() && lhs.ct_eq(rhs).into() {
                    Some(idx)
                } else {
                    None
                }
            })
            .ok_or_else(|| anyhow::anyhow!("Enrollment token not found"))?;

        {
            let t = &self.enrollments[pos];

            if t.role != role {
                anyhow::bail!(
                    "Token role '{}' does not match requested role '{}'",
                    t.role,
                    role
                );
            }

            if let Some(expires_at) = t.expires_at {
                if now > expires_at {
                    anyhow::bail!("Enrollment token has expired");
                }
            }

            if let Some(uses) = t.remaining_uses {
                if uses == 0 {
                    anyhow::bail!("Enrollment token has no remaining uses");
                }
            }
        }

        // Decrement or remove
        let matched = self.enrollments[pos].clone();
        let token = &mut self.enrollments[pos];
        if let Some(uses) = token.remaining_uses.as_mut() {
            *uses -= 1;
            if *uses == 0 {
                self.enrollments.remove(pos);
            }
        }

        Ok(matched)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::TempDir;

    fn make_entry(name: &str, serial: u64) -> CertEntry {
        CertEntry {
            serial,
            name: name.to_string(),
            role: CertRole::Agent,
            status: CertStatus::Active,
            not_before: Utc::now(),
            not_after: Utc::now(),
            fingerprint_sha256: "AA:BB".to_string(),
            subject: format!("CN={name}"),
            sans: vec![],
            roles: vec![],
            groups: vec![],
        }
    }

    #[test]
    fn test_index_load_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.json");
        let idx = PkiIndex::load(&path).unwrap();
        assert_eq!(idx.next_serial, 2);
        assert!(idx.entries.is_empty());
    }

    #[test]
    fn test_index_save_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.json");

        let mut idx = PkiIndex::default();
        idx.add_entry(make_entry("alice", 1));
        idx.save(&path).unwrap();

        let idx2 = PkiIndex::load(&path).unwrap();
        assert_eq!(idx2.entries.len(), 1);
        assert_eq!(idx2.entries[0].name, "alice");
    }

    #[test]
    fn test_revoke() {
        let mut idx = PkiIndex::default();
        idx.add_entry(make_entry("bob", 1));
        idx.revoke("bob").unwrap();
        assert_eq!(idx.entries[0].status, CertStatus::Revoked);
        // Revoking again should fail — no active cert
        assert!(idx.revoke("bob").is_err());
    }

    #[test]
    fn test_active_entries_filter() {
        let mut idx = PkiIndex::default();
        idx.add_entry(make_entry("active-one", 1));
        let mut revoked = make_entry("revoked-one", 2);
        revoked.status = CertStatus::Revoked;
        idx.add_entry(revoked);

        let active: Vec<_> = idx.active_entries().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "active-one");
    }

    #[test]
    fn test_enrollment_token_lifecycle() {
        let mut idx = PkiIndex::default();
        let token = EnrollmentToken {
            id: "tok-1".to_string(),
            token: "secret-abc".to_string(),
            remaining_uses: Some(2),
            expires_at: None,
            created_at: Utc::now(),
            role: CertRole::Agent,
            roles: vec!["agent".to_string()],
        };
        idx.add_enrollment(token);

        // First use should succeed
        idx.validate_enrollment("secret-abc", CertRole::Agent)
            .unwrap();
        assert_eq!(idx.enrollments[0].remaining_uses, Some(1));

        // Second use should consume and remove the token
        idx.validate_enrollment("secret-abc", CertRole::Agent)
            .unwrap();
        assert!(idx.enrollments.is_empty());

        // Third use should fail
        assert!(idx
            .validate_enrollment("secret-abc", CertRole::Agent)
            .is_err());
    }

    #[test]
    fn test_enrollment_wrong_role() {
        let mut idx = PkiIndex::default();
        idx.add_enrollment(EnrollmentToken {
            id: "tok-2".to_string(),
            token: "rolecheck".to_string(),
            remaining_uses: None,
            expires_at: None,
            created_at: Utc::now(),
            role: CertRole::Tui,
            roles: vec!["tui".to_string()],
        });
        assert!(idx
            .validate_enrollment("rolecheck", CertRole::Agent)
            .is_err());
    }

    #[test]
    fn test_cert_role_display() {
        assert_eq!(CertRole::Gateway.to_string(), "gateway");
        assert_eq!(CertRole::Agent.to_string(), "agent");
        assert_eq!(CertRole::Tui.to_string(), "tui");
    }
}
