//! High-level PKI manager — the main entry point for all PKI operations.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::backend::{FileBackend, KeyBackend, KeyHandle};
use crate::ca;
use crate::index::{CertEntry, CertRole, CertStatus, EnrollmentToken, PkiIndex};

/// Path constants relative to `pki_dir`.
const CA_CERT_FILE: &str = "ca.crt";
const CA_KEY_LABEL: &str = "ca";
const INDEX_FILE: &str = "index.json";
const CERTS_DIR: &str = "certs";
const KEYS_DIR: &str = "keys";
const CRL_FILE: &str = "crl.pem";
const STORAGE_KEYS_DIR: &str = "storage";
const VAULT_KEYS_DIR: &str = "vault";

/// Result of ensuring a node-local vault keypair.
#[derive(Debug, Clone)]
pub struct NodeVaultKeypairInfo {
    pub node_label: String,
    pub identity_path: PathBuf,
    pub public_key_path: PathBuf,
    pub public_key: String,
}

/// Information about the CA certificate.
#[derive(Debug, Clone)]
pub struct CaInfo {
    /// PEM certificate.
    pub pem: String,
    /// SHA-256 fingerprint.
    pub fingerprint: String,
    /// Certificate not-before date.
    pub not_before: DateTime<Utc>,
    /// Certificate not-after (expiry) date.
    pub not_after: DateTime<Utc>,
}

/// Information about a newly issued client certificate.
#[derive(Debug, Clone)]
pub struct ClientCertInfo {
    /// Human-readable certificate name.
    pub name: String,
    /// Path to the PEM certificate on disk.
    pub cert_path: PathBuf,
    /// Path to the PEM private key on disk.
    pub key_path: PathBuf,
}

/// High-level PKI manager.
///
/// Owns the PKI directory structure, the in-memory index, and delegates
/// cryptographic operations to the configured [`KeyBackend`].
///
/// Directory layout:
/// ```text
/// <pki_dir>/
///   ca.crt          — CA certificate (PEM)
///   ca.key          — CA private key (PEM, 0600)
///   index.json      — Certificate registry
///   crl.pem         — Current CRL (regenerated on revocation)
///   certs/          — Issued leaf certificates (<name>.crt)
///   keys/           — Leaf private keys (<name>.key, 0600)
/// ```
pub struct PkiManager {
    pki_dir: PathBuf,
    backend: Box<dyn KeyBackend>,
    index: PkiIndex,
}

impl PkiManager {
    /// Create (or open) a PKI directory and return a `PkiManager`.
    ///
    /// Creates subdirectories as needed and loads the existing index.
    pub fn new(pki_dir: PathBuf) -> anyhow::Result<Self> {
        fs::create_dir_all(&pki_dir)?;
        fs::create_dir_all(pki_dir.join(CERTS_DIR))?;
        fs::create_dir_all(pki_dir.join(KEYS_DIR))?;
        fs::create_dir_all(pki_dir.join(STORAGE_KEYS_DIR))?;
        fs::create_dir_all(pki_dir.join(VAULT_KEYS_DIR))?;

        let keys_dir = pki_dir.join(KEYS_DIR);
        let backend = Box::new(FileBackend::new(keys_dir));
        let index = PkiIndex::load(&pki_dir.join(INDEX_FILE))?;

        Ok(Self {
            pki_dir,
            backend,
            index,
        })
    }

    /// Return the PKI directory path.
    pub fn pki_dir(&self) -> &Path {
        &self.pki_dir
    }

    /// Return the managed storage-key path for a logical store label.
    pub fn storage_key_path(&self, label: &str) -> PathBuf {
        self.pki_dir
            .join(STORAGE_KEYS_DIR)
            .join(format!("{label}.storage.key"))
    }

    /// Ensure a node-local 32-byte storage key exists and return it.
    pub fn ensure_local_storage_key(&self, label: &str) -> anyhow::Result<[u8; 32]> {
        let path = self.storage_key_path(label);
        if path.exists() {
            let bytes = fs::read(&path)?;
            if bytes.len() != 32 {
                anyhow::bail!(
                    "Storage key at {} has wrong length: expected 32 bytes, got {}",
                    path.display(),
                    bytes.len()
                );
            }
            return Ok(bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid storage key length"))?);
        }

        let rng = ring::rand::SystemRandom::new();
        let mut key = [0u8; 32];
        ring::rand::SecureRandom::fill(&rng, &mut key)
            .map_err(|_| anyhow::anyhow!("Failed to generate storage key"))?;

        let mut file = fs::File::create(&path)?;
        file.write_all(&key)?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
        Ok(key)
    }

    /// Ensure a node-local Age keypair exists for vault grant decryption.
    pub fn ensure_vault_keypair(&self, node_label: &str) -> anyhow::Result<NodeVaultKeypairInfo> {
        let identity_path = self
            .pki_dir
            .join(VAULT_KEYS_DIR)
            .join(format!("{node_label}.vault.agekey"));
        let public_key_path = self
            .pki_dir
            .join(VAULT_KEYS_DIR)
            .join(format!("{node_label}.vault.pub"));

        let public_key = if identity_path.exists() {
            let identity_text = fs::read_to_string(&identity_path)?;
            let identity = identity_text
                .trim()
                .parse::<age::x25519::Identity>()
                .map_err(|e| anyhow::anyhow!("Failed to parse vault identity: {e}"))?;
            identity.to_public().to_string()
        } else {
            use age::secrecy::ExposeSecret;

            let identity = age::x25519::Identity::generate();
            let identity_text = identity.to_string();
            let public_key = identity.to_public().to_string();

            let mut file = fs::File::create(&identity_path)?;
            file.write_all(identity_text.expose_secret().as_bytes())?;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&identity_path, perms)?;
            fs::write(&public_key_path, format!("{public_key}\n"))?;

            public_key
        };

        if !public_key_path.exists() {
            fs::write(&public_key_path, format!("{public_key}\n"))?;
        }

        Ok(NodeVaultKeypairInfo {
            node_label: node_label.to_string(),
            identity_path,
            public_key_path,
            public_key,
        })
    }

    /// Persist the in-memory index to disk.
    pub fn save_index(&self) -> anyhow::Result<()> {
        self.index.save(&self.pki_dir.join(INDEX_FILE))
    }

    // -------------------------------------------------------------------------
    // CA management
    // -------------------------------------------------------------------------

    /// Initialise the CA: generate a CA key pair and self-signed certificate.
    ///
    /// Returns an error if the CA already exists (call `ca_cert_pem()` to check first).
    pub fn init_ca(&mut self, days: u32) -> anyhow::Result<()> {
        let ca_cert_path = self.pki_dir.join(CA_CERT_FILE);
        if ca_cert_path.exists() {
            anyhow::bail!("CA already initialised at {}", ca_cert_path.display());
        }

        let ca_key = self.backend.generate(CA_KEY_LABEL)?;
        let ca_pem = ca::generate_ca(&ca_key, days)?;

        fs::write(&ca_cert_path, &ca_pem)?;
        self.save_index()?;

        tracing::info!(path = %ca_cert_path.display(), "CA initialised");
        Ok(())
    }

    /// Return the CA certificate as a PEM string.
    pub fn ca_cert_pem(&self) -> anyhow::Result<String> {
        let path = self.pki_dir.join(CA_CERT_FILE);
        if !path.exists() {
            anyhow::bail!("CA not initialised — run init_ca() first");
        }
        Ok(fs::read_to_string(path)?)
    }

    /// Return high-level information about the CA certificate.
    pub fn ca_info(&self) -> anyhow::Result<CaInfo> {
        let pem = self.ca_cert_pem()?;
        let fingerprint = self.ca_fingerprint(&pem)?;

        // Parse dates from PEM using x509-parser
        let (not_before, not_after) = parse_cert_validity(&pem)?;

        Ok(CaInfo {
            pem,
            fingerprint,
            not_before,
            not_after,
        })
    }

    fn ca_fingerprint(&self, ca_pem: &str) -> anyhow::Result<String> {
        // Decode PEM to DER to fingerprint
        let der = pem_to_der(ca_pem)?;
        Ok(ca::sha256_fingerprint(&der))
    }

    /// Load the CA key handle.
    pub fn ca_key_handle(&self) -> anyhow::Result<KeyHandle> {
        let key_path = self
            .pki_dir
            .join(KEYS_DIR)
            .join(format!("{CA_KEY_LABEL}.key"));
        self.backend.load(&key_path)
    }

    // -------------------------------------------------------------------------
    // Client certificate management
    // -------------------------------------------------------------------------

    /// Generate a new client certificate signed by the CA.
    ///
    /// `roles` and `groups` are embedded as custom x.509 extensions for
    /// certificate-based authorization (Nebula-inspired).
    pub fn generate_client(
        &mut self,
        name: &str,
        role: CertRole,
        sans: &[String],
        days: u32,
        roles: &[String],
        groups: &[String],
    ) -> anyhow::Result<ClientCertInfo> {
        let ca_pem = self.ca_cert_pem()?;
        let ca_key = self.ca_key_handle()?;

        let serial = self.index.next_serial();
        let client_key = self.backend.generate(name)?;

        let (cert_pem, entry) = ca::generate_client_cert(
            &client_key,
            &ca_pem,
            &ca_key,
            name,
            role,
            sans,
            days,
            serial,
            roles,
            groups,
        )?;

        // Save cert to certs/<name>.crt
        let cert_path = self.pki_dir.join(CERTS_DIR).join(format!("{name}.crt"));
        fs::write(&cert_path, &cert_pem)?;

        // Key was already saved by the backend into keys/<name>.key
        let key_path = self.pki_dir.join(KEYS_DIR).join(format!("{name}.key"));

        self.index.add_entry(entry);
        self.save_index()?;

        tracing::info!(name, serial, "Issued client certificate");

        Ok(ClientCertInfo {
            name: name.to_string(),
            cert_path,
            key_path,
        })
    }

    /// List all certificate entries in the index.
    pub fn list_clients(&self) -> Vec<&CertEntry> {
        self.index.entries.iter().collect()
    }

    /// Look up a certificate entry by name.
    pub fn client_info(&self, name: &str) -> anyhow::Result<&CertEntry> {
        self.index
            .find_by_name(name)
            .ok_or_else(|| anyhow::anyhow!("Certificate '{name}' not found"))
    }

    /// Revoke the active certificate with the given name and regenerate the CRL.
    pub fn revoke_client(&mut self, name: &str) -> anyhow::Result<()> {
        self.index.revoke(name)?;
        self.save_index()?;
        self.regenerate_crl()?;
        tracing::info!(name, "Certificate revoked");
        Ok(())
    }

    /// Rotate a client certificate: revoke the old one, issue a fresh one.
    ///
    /// Preserves the existing role, roles, and groups from the previous certificate.
    pub fn rotate_client(
        &mut self,
        name: &str,
        sans: &[String],
        days: u32,
    ) -> anyhow::Result<ClientCertInfo> {
        // Find the existing entry before revoking (preserve role, roles, groups)
        let existing = self
            .index
            .find_by_name(name)
            .ok_or_else(|| anyhow::anyhow!("Certificate '{name}' not found"))?;
        let role = existing.role;
        let roles = existing.roles.clone();
        let groups = existing.groups.clone();

        self.revoke_client(name)?;

        // Remove old key file so backend generates a fresh one
        let old_key = self.pki_dir.join(KEYS_DIR).join(format!("{name}.key"));
        if old_key.exists() {
            fs::remove_file(&old_key)?;
        }

        self.generate_client(name, role, sans, days, &roles, &groups)
    }

    /// Sign an externally-provided CSR with the CA.
    ///
    /// Returns the signed certificate as a PEM string.
    /// `roles` and `groups` are recorded in the index (and embedded in the cert
    /// when the signing path supports extension injection).
    pub fn sign_csr(
        &mut self,
        csr_pem: &str,
        name: &str,
        role: CertRole,
        days: u32,
        roles: &[String],
        groups: &[String],
    ) -> anyhow::Result<String> {
        let ca_pem = self.ca_cert_pem()?;
        let ca_key = self.ca_key_handle()?;
        let serial = self.index.next_serial();

        let (cert_pem, entry) = ca::sign_csr(
            csr_pem, &ca_pem, &ca_key, name, role, days, serial, roles, groups,
        )?;

        self.index.add_entry(entry);
        self.save_index()?;

        Ok(cert_pem)
    }

    /// Import a previously signed client certificate into the local PKI index.
    ///
    /// Useful on enrolled client nodes, where the certificate/key are obtained
    /// remotely and then need to be locally installable via the normal PKI flows.
    pub fn import_signed_client(
        &mut self,
        name: &str,
        role: CertRole,
        cert_pem: &str,
    ) -> anyhow::Result<()> {
        let (not_before, not_after) = parse_cert_validity(cert_pem)?;
        let der = pem_to_der(cert_pem)?;
        let fingerprint = ca::sha256_fingerprint(&der);

        let serial = self.index.next_serial();
        let entry = CertEntry {
            serial,
            name: name.to_string(),
            role,
            status: CertStatus::Active,
            not_before,
            not_after,
            fingerprint_sha256: fingerprint,
            subject: format!("CN={name}"),
            sans: vec![],
            roles: vec![],
            groups: vec![],
        };

        self.index.add_entry(entry);
        self.save_index()?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Enrollment tokens
    // -------------------------------------------------------------------------

    /// Create a new enrollment token for a given role.
    ///
    /// - `uses`: maximum number of times the token can be used (`None` = unlimited).
    /// - `expires`: optional absolute expiry time.
    ///
    /// Returns the secret token string (UUID).
    pub fn create_enrollment(
        &mut self,
        role: CertRole,
        roles: &[String],
        uses: Option<u32>,
        expires: Option<DateTime<Utc>>,
    ) -> anyhow::Result<String> {
        let token_str = Uuid::new_v4().to_string();
        let token = EnrollmentToken {
            id: Uuid::new_v4().to_string(),
            token: token_str.clone(),
            remaining_uses: uses,
            expires_at: expires,
            created_at: Utc::now(),
            role,
            roles: roles.to_vec(),
        };
        self.index.add_enrollment(token);
        self.save_index()?;
        Ok(token_str)
    }

    /// List all active enrollment tokens.
    pub fn list_enrollments(&self) -> &[EnrollmentToken] {
        &self.index.enrollments
    }

    /// Revoke (remove) an enrollment token by its ID.
    pub fn revoke_enrollment(&mut self, id: &str) -> anyhow::Result<()> {
        let pos = self
            .index
            .enrollments
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow::anyhow!("Enrollment token '{id}' not found"))?;
        self.index.enrollments.remove(pos);
        self.save_index()?;
        Ok(())
    }

    /// Validate and consume an enrollment token.
    ///
    /// Returns `Ok(())` if the token is valid and was successfully consumed.
    pub fn validate_enrollment(
        &mut self,
        token: &str,
        role: CertRole,
    ) -> anyhow::Result<EnrollmentToken> {
        let enrollment = self.index.validate_enrollment(token, role)?;
        self.save_index()?;
        Ok(enrollment)
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    fn regenerate_crl(&self) -> anyhow::Result<()> {
        let ca_pem = self.ca_cert_pem()?;
        let ca_key = self.ca_key_handle()?;

        let revoked: Vec<&CertEntry> = self
            .index
            .entries
            .iter()
            .filter(|e| e.status == crate::index::CertStatus::Revoked)
            .collect();

        let crl_pem = ca::generate_crl(&revoked, &ca_pem, &ca_key)?;
        let crl_path = self.pki_dir.join(CRL_FILE);
        fs::write(crl_path, crl_pem)?;
        Ok(())
    }
}

/// Decode the first PEM block in `pem_str` to raw DER bytes.
pub fn pem_to_der(pem_str: &str) -> anyhow::Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(pem_str.as_bytes());
    let item = rustls_pemfile::read_one(&mut cursor)?
        .ok_or_else(|| anyhow::anyhow!("No PEM block found"))?;
    match item {
        rustls_pemfile::Item::X509Certificate(der) => Ok(der.to_vec()),
        other => anyhow::bail!("Unexpected PEM item type: {:?}", other),
    }
}

/// Parse the validity period from a PEM certificate using x509-parser.
fn parse_cert_validity(pem_str: &str) -> anyhow::Result<(DateTime<Utc>, DateTime<Utc>)> {
    let der = pem_to_der(pem_str)?;
    let (_, cert) = x509_parser::parse_x509_certificate(&der)
        .map_err(|e| anyhow::anyhow!("Failed to parse certificate: {e}"))?;

    let not_before = chrono::DateTime::from_timestamp(cert.validity().not_before.timestamp(), 0)
        .unwrap_or_default();

    let not_after = chrono::DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0)
        .unwrap_or_default();

    Ok((not_before, not_after))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_pki_manager_full_flow() {
        let dir = TempDir::new().unwrap();
        let mut mgr = PkiManager::new(dir.path().to_path_buf()).unwrap();

        // CA not yet initialised
        assert!(mgr.ca_cert_pem().is_err());

        // Init CA
        mgr.init_ca(3650).unwrap();
        let ca_pem = mgr.ca_cert_pem().unwrap();
        assert!(ca_pem.contains("BEGIN CERTIFICATE"));

        // Generate two clients
        let alice = mgr
            .generate_client("alice", CertRole::Agent, &[], 365, &[], &[])
            .unwrap();
        let _bob = mgr
            .generate_client("bob", CertRole::Tui, &[], 365, &[], &[])
            .unwrap();

        assert!(alice.cert_path.exists());
        assert!(alice.key_path.exists());

        assert_eq!(mgr.list_clients().len(), 2);

        // Verify entries
        let alice_info = mgr.client_info("alice").unwrap();
        assert_eq!(alice_info.role, CertRole::Agent);

        // Revoke alice
        mgr.revoke_client("alice").unwrap();
        let alice_info = mgr.client_info("alice").unwrap();
        assert_eq!(alice_info.status, crate::index::CertStatus::Revoked);

        // CRL should exist after revocation
        let crl_path = dir.path().join("crl.pem");
        assert!(crl_path.exists());
        let crl_pem = std::fs::read_to_string(&crl_path).unwrap();
        assert!(crl_pem.contains("BEGIN X509 CRL"));

        // Bob should still be active
        let active: Vec<_> = mgr.index.active_entries().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "bob");
    }

    #[test]
    fn test_ca_info() {
        let dir = TempDir::new().unwrap();
        let mut mgr = PkiManager::new(dir.path().to_path_buf()).unwrap();
        mgr.init_ca(365).unwrap();
        let info = mgr.ca_info().unwrap();
        assert!(!info.fingerprint.is_empty());
        assert!(info.not_after > info.not_before);
    }

    #[test]
    fn test_rotate_client() {
        let dir = TempDir::new().unwrap();
        let mut mgr = PkiManager::new(dir.path().to_path_buf()).unwrap();
        mgr.init_ca(3650).unwrap();

        mgr.generate_client(
            "svc",
            CertRole::Agent,
            &[],
            365,
            &["operator".to_string()],
            &["team-a".to_string()],
        )
        .unwrap();
        let old_serial = mgr.client_info("svc").unwrap().serial;

        mgr.rotate_client("svc", &[], 365).unwrap();

        let all_clients = mgr.list_clients();
        let svc_entries: Vec<_> = all_clients.iter().filter(|e| e.name == "svc").collect();
        assert_eq!(
            svc_entries.len(),
            2,
            "Should have old (revoked) + new entry"
        );

        // The new entry has the higher serial number
        let new_entry = svc_entries.iter().max_by_key(|e| e.serial).unwrap();
        assert_ne!(new_entry.serial, old_serial);
        assert_eq!(new_entry.status, crate::index::CertStatus::Active);
    }

    #[test]
    fn test_sign_csr_via_manager() {
        let dir = TempDir::new().unwrap();
        let mut mgr = PkiManager::new(dir.path().to_path_buf()).unwrap();
        mgr.init_ca(3650).unwrap();

        // Build a CSR externally
        let csr_key = rcgen::KeyPair::generate().unwrap();
        let mut csr_params = rcgen::CertificateParams::default();
        csr_params.distinguished_name = {
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, "external-svc");
            dn
        };
        let csr = csr_params.serialize_request(&csr_key).unwrap();
        let csr_pem = csr.pem().unwrap();

        let cert_pem = mgr
            .sign_csr(&csr_pem, "external-svc", CertRole::Agent, 365, &[], &[])
            .unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(mgr.list_clients().len(), 1);
    }

    #[test]
    fn test_enrollment_token_lifecycle() {
        let dir = TempDir::new().unwrap();
        let mut mgr = PkiManager::new(dir.path().to_path_buf()).unwrap();
        mgr.init_ca(3650).unwrap();

        let token = mgr
            .create_enrollment(CertRole::Agent, &["agent".to_string()], Some(2), None)
            .unwrap();
        assert!(!token.is_empty());

        // First use
        mgr.validate_enrollment(&token, CertRole::Agent).unwrap();
        assert_eq!(mgr.index.enrollments[0].remaining_uses, Some(1));

        // Second use — exhausts token
        mgr.validate_enrollment(&token, CertRole::Agent).unwrap();
        assert!(mgr.index.enrollments.is_empty());

        // Third use — should fail
        assert!(mgr.validate_enrollment(&token, CertRole::Agent).is_err());
    }
}
