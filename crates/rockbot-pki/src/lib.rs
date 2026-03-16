//! `rockbot-pki` — Local PKI management for RockBot.
//!
//! Provides:
//! - CA key generation and self-signed CA certificate issuance
//! - Client certificate issuance and revocation (mTLS)
//! - CSR signing
//! - CRL generation
//! - Enrollment tokens for automated certificate provisioning
//! - Pluggable [`KeyBackend`] abstraction (file today, HSM/YubiKey in the future)
//!
//! # Quick start
//!
//! ```no_run
//! use rockbot_pki::{PkiManager, CertRole};
//!
//! # fn main() -> anyhow::Result<()> {
//! let mut mgr = PkiManager::new("/tmp/rockbot-pki".into())?;
//! mgr.init_ca(3650)?;
//!
//! let info = mgr.generate_client("gateway", CertRole::Gateway, &["localhost".to_string()], 365, &[], &[])?;
//! println!("Cert: {}", info.cert_path.display());
//! # Ok(())
//! # }
//! ```

pub mod backend;
pub mod ca;
pub mod extensions;
pub mod index;
pub mod manager;

// Convenience re-exports
pub use backend::{FileBackend, KeyBackend, KeyHandle};
pub use ca::{generate_csr, sha256_fingerprint, sign_csr};
pub use extensions::{
    build_extensions, parse_extensions, CertExtensions, OID_ROCKBOT_GROUPS, OID_ROCKBOT_ROLES,
};
pub use index::{CertEntry, CertRole, CertStatus, EnrollmentToken, PkiIndex};
pub use manager::{CaInfo, ClientCertInfo, PkiManager};
