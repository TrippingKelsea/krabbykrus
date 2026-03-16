//! CA and certificate signing operations.

use std::net::IpAddr;
use std::str::FromStr;

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams,
    CertificateSigningRequestParams, ExtendedKeyUsagePurpose, IsCa, KeyIdMethod, KeyUsagePurpose,
    RevocationReason, RevokedCertParams, SanType, SerialNumber,
};
use time::OffsetDateTime;

use crate::extensions;
use crate::index::{CertEntry, CertRole, CertStatus};
use crate::KeyHandle;

/// Compute a SHA-256 fingerprint from raw DER bytes.
///
/// Returns a colon-separated uppercase hex string, e.g. `"AA:BB:CC:..."`.
pub fn sha256_fingerprint(der_bytes: &[u8]) -> String {
    use ring::digest;
    let hash = digest::digest(&digest::SHA256, der_bytes);
    hash.as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Convert a `chrono::Duration`-equivalent number of days into a `time::Duration`.
fn days_duration(days: u32) -> time::Duration {
    time::Duration::days(i64::from(days))
}

/// Parse a SAN string: if it looks like an IP address parse it as `IpAddress`, otherwise `DnsName`.
fn parse_san(s: &str) -> anyhow::Result<SanType> {
    if let Ok(ip) = IpAddr::from_str(s) {
        Ok(SanType::IpAddress(ip))
    } else {
        Ok(SanType::DnsName(
            s.try_into().map_err(|e: rcgen::Error| anyhow::anyhow!(e))?,
        ))
    }
}

/// Generate a self-signed CA certificate and return its PEM representation.
///
/// The CA key must already be generated and accessible via `key`.
pub fn generate_ca(key: &KeyHandle, days: u32) -> anyhow::Result<String> {
    let key_pair = key.key_pair()?;

    let now = OffsetDateTime::now_utc();
    let not_after = now + days_duration(days);

    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params.not_before = now;
    params.not_after = not_after;
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "RockBot CA");
        dn.push(rcgen::DnType::OrganizationName, "RockBot");
        dn
    };
    params.serial_number = Some(SerialNumber::from(1u64));
    // Use Sha256 key identifier method (default for crypto feature)
    params.key_identifier_method = KeyIdMethod::Sha256;
    params.use_authority_key_identifier_extension = false;

    let cert = params.self_signed(key_pair)?;
    Ok(cert.pem())
}

/// Reconstruct a `rcgen::Certificate` from its PEM string and the associated `KeyHandle`.
///
/// Internally this parses the PEM into `CertificateParams` and then re-signs it with
/// the same key, producing a `Certificate` object whose DN and key identifier match the
/// original.  The resulting `Certificate` is suitable for use as the issuer in
/// `CertificateParams::signed_by` / `CertificateSigningRequestParams::signed_by`.
fn reconstruct_ca_cert(
    ca_cert_pem: &str,
    ca_key: &KeyHandle,
) -> anyhow::Result<rcgen::Certificate> {
    let ca_key_pair = ca_key.key_pair()?;
    let ca_params = CertificateParams::from_ca_cert_pem(ca_cert_pem)?;
    let ca_cert = ca_params.self_signed(ca_key_pair)?;
    Ok(ca_cert)
}

/// Generate a client certificate signed by the given CA.
///
/// Returns the PEM-encoded certificate and a fully populated [`CertEntry`] for the index.
///
/// `roles` and `groups` are embedded as custom x.509 extensions (Nebula-inspired)
/// for certificate-based authorization.
#[allow(clippy::too_many_arguments)]
pub fn generate_client_cert(
    client_key: &KeyHandle,
    ca_cert_pem: &str,
    ca_key: &KeyHandle,
    name: &str,
    role: CertRole,
    sans: &[String],
    days: u32,
    serial: u64,
    roles: &[String],
    groups: &[String],
) -> anyhow::Result<(String, CertEntry)> {
    let client_key_pair = client_key.key_pair()?;
    let ca_key_pair = ca_key.key_pair()?;

    let now = OffsetDateTime::now_utc();
    let not_after = now + days_duration(days);

    let mut params = CertificateParams::default();
    params.serial_number = Some(SerialNumber::from(serial));
    params.not_before = now;
    params.not_after = not_after;
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, name);
        dn.push(rcgen::DnType::OrganizationName, "RockBot");
        dn
    };
    params.use_authority_key_identifier_extension = true;

    // Set EKU based on role
    match role {
        CertRole::Gateway => {
            params.extended_key_usages = vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ];
        }
        CertRole::Agent | CertRole::Tui => {
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        }
    }

    // Add SANs
    for s in sans {
        let san = parse_san(s)?;
        params.subject_alt_names.push(san);
    }

    // Embed roles and groups as custom x.509 extensions
    for ext in extensions::build_extensions(roles, groups) {
        params.custom_extensions.push(ext);
    }

    // Reconstruct the CA Certificate object
    let ca_cert = reconstruct_ca_cert(ca_cert_pem, ca_key)?;

    // Sign with the CA
    let cert = params.signed_by(client_key_pair, &ca_cert, ca_key_pair)?;

    let der = cert.der().to_vec();
    let fingerprint = sha256_fingerprint(&der);
    let pem = cert.pem();

    let entry = CertEntry {
        serial,
        name: name.to_string(),
        role,
        status: CertStatus::Active,
        not_before: chrono::DateTime::from_timestamp(now.unix_timestamp(), 0).unwrap_or_default(),
        not_after: chrono::DateTime::from_timestamp(not_after.unix_timestamp(), 0)
            .unwrap_or_default(),
        fingerprint_sha256: fingerprint,
        subject: format!("CN={name},O=RockBot"),
        sans: sans.to_vec(),
        roles: roles.to_vec(),
        groups: groups.to_vec(),
    };

    Ok((pem, entry))
}

/// Sign an externally-generated CSR with the CA.
///
/// Returns the signed certificate PEM and an index entry.
/// `roles` and `groups` are embedded as custom x.509 extensions.
#[allow(clippy::too_many_arguments)]
pub fn sign_csr(
    csr_pem: &str,
    ca_cert_pem: &str,
    ca_key: &KeyHandle,
    name: &str,
    role: CertRole,
    days: u32,
    serial: u64,
    roles: &[String],
    groups: &[String],
) -> anyhow::Result<(String, CertEntry)> {
    let ca_key_pair = ca_key.key_pair()?;

    // Parse the CSR
    let csr_params = CertificateSigningRequestParams::from_pem(csr_pem)?;

    let now = OffsetDateTime::now_utc();
    let not_after = now + days_duration(days);

    // Reconstruct CA cert object
    let ca_cert = reconstruct_ca_cert(ca_cert_pem, ca_key)?;

    // Sign the CSR
    // Note: rcgen's CSR signing doesn't support injecting custom extensions into
    // the signed certificate — the extensions would need to be in the CSR itself
    // or applied via a wrapper. For now, roles/groups are recorded in the index
    // and will be fully embedded once rcgen supports extension injection on CSR signing.
    let cert = csr_params.signed_by(&ca_cert, ca_key_pair)?;

    let der = cert.der().to_vec();
    let fingerprint = sha256_fingerprint(&der);
    let pem = cert.pem();

    let entry = CertEntry {
        serial,
        name: name.to_string(),
        role,
        status: CertStatus::Active,
        not_before: chrono::DateTime::from_timestamp(now.unix_timestamp(), 0).unwrap_or_default(),
        not_after: chrono::DateTime::from_timestamp(not_after.unix_timestamp(), 0)
            .unwrap_or_default(),
        fingerprint_sha256: fingerprint,
        subject: format!("CN={name}"),
        sans: vec![],
        roles: roles.to_vec(),
        groups: groups.to_vec(),
    };

    Ok((pem, entry))
}

/// Generate a CRL (Certificate Revocation List) from the given revoked entries.
///
/// Returns PEM-encoded CRL string.
pub fn generate_crl(
    revoked_entries: &[&CertEntry],
    ca_cert_pem: &str,
    ca_key: &KeyHandle,
) -> anyhow::Result<String> {
    let ca_key_pair = ca_key.key_pair()?;

    let now = OffsetDateTime::now_utc();
    // CRL valid for 7 days
    let next_update = now + time::Duration::days(7);

    let ca_cert = reconstruct_ca_cert(ca_cert_pem, ca_key)?;

    let revoked_certs: Vec<RevokedCertParams> = revoked_entries
        .iter()
        .map(|e| RevokedCertParams {
            serial_number: SerialNumber::from(e.serial),
            revocation_time: now,
            reason_code: Some(RevocationReason::Unspecified),
            invalidity_date: None,
        })
        .collect();

    let crl_params = CertificateRevocationListParams {
        this_update: now,
        next_update,
        crl_number: SerialNumber::from(1u64),
        issuing_distribution_point: None,
        revoked_certs,
        key_identifier_method: KeyIdMethod::Sha256,
    };

    let crl = crl_params.signed_by(&ca_cert, ca_key_pair)?;
    Ok(crl.pem()?)
}

/// Generate a Certificate Signing Request (CSR) from an existing key handle.
///
/// Returns the PEM-encoded CSR string.
pub fn generate_csr(key: &KeyHandle, name: &str) -> anyhow::Result<String> {
    let key_pair = key.key_pair()?;

    let mut params = CertificateParams::default();
    params.distinguished_name = {
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, name);
        dn.push(rcgen::DnType::OrganizationName, "RockBot");
        dn
    };

    let csr = params.serialize_request(key_pair)?;
    Ok(csr.pem()?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::backend::{FileBackend, KeyBackend};
    use tempfile::TempDir;

    fn setup_ca(dir: &TempDir) -> (FileBackend, KeyHandle) {
        let backend = FileBackend::new(dir.path().to_path_buf());
        let ca_key = backend.generate("ca").unwrap();
        (backend, ca_key)
    }

    #[test]
    fn test_ca_generation() {
        let dir = TempDir::new().unwrap();
        let (_, ca_key) = setup_ca(&dir);
        let pem = generate_ca(&ca_key, 3650).unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"), "Should be a PEM cert");
    }

    #[test]
    fn test_client_cert_generation() {
        let dir = TempDir::new().unwrap();
        let (backend, ca_key) = setup_ca(&dir);
        let ca_pem = generate_ca(&ca_key, 3650).unwrap();

        let client_key = backend.generate("client").unwrap();
        let (cert_pem, entry) = generate_client_cert(
            &client_key,
            &ca_pem,
            &ca_key,
            "test-agent",
            CertRole::Agent,
            &[],
            365,
            2,
            &[],
            &[],
        )
        .unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(entry.name, "test-agent");
        assert_eq!(entry.role, CertRole::Agent);
        assert_eq!(entry.status, CertStatus::Active);
        assert!(!entry.fingerprint_sha256.is_empty());
    }

    #[test]
    fn test_client_cert_gateway_role() {
        let dir = TempDir::new().unwrap();
        let (backend, ca_key) = setup_ca(&dir);
        let ca_pem = generate_ca(&ca_key, 3650).unwrap();

        let gw_key = backend.generate("gateway").unwrap();
        let (cert_pem, entry) = generate_client_cert(
            &gw_key,
            &ca_pem,
            &ca_key,
            "gateway",
            CertRole::Gateway,
            &["localhost".to_string(), "127.0.0.1".to_string()],
            365,
            2,
            &[],
            &[],
        )
        .unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(entry.role, CertRole::Gateway);
        assert_eq!(entry.sans.len(), 2);
    }

    #[test]
    fn test_csr_signing() {
        let dir = TempDir::new().unwrap();
        let (backend, ca_key) = setup_ca(&dir);
        let ca_pem = generate_ca(&ca_key, 3650).unwrap();

        // Generate a CSR
        let csr_key = backend.generate("csr-client").unwrap();
        let csr_key_pair = csr_key.key_pair().unwrap();

        let mut csr_params = CertificateParams::default();
        csr_params.distinguished_name = {
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, "csr-test");
            dn
        };
        let csr = csr_params.serialize_request(csr_key_pair).unwrap();
        let csr_pem = csr.pem().unwrap();

        let (cert_pem, entry) = sign_csr(
            &csr_pem,
            &ca_pem,
            &ca_key,
            "csr-test",
            CertRole::Agent,
            365,
            3,
            &[],
            &[],
        )
        .unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(entry.name, "csr-test");
    }

    #[test]
    fn test_revocation_crl() {
        let dir = TempDir::new().unwrap();
        let (backend, ca_key) = setup_ca(&dir);
        let ca_pem = generate_ca(&ca_key, 3650).unwrap();

        let client_key = backend.generate("to-revoke").unwrap();
        let (_, entry) = generate_client_cert(
            &client_key,
            &ca_pem,
            &ca_key,
            "to-revoke",
            CertRole::Agent,
            &[],
            365,
            2,
            &[],
            &[],
        )
        .unwrap();

        let revoked = CertEntry {
            status: CertStatus::Revoked,
            ..entry
        };

        let crl_pem = generate_crl(&[&revoked], &ca_pem, &ca_key).unwrap();
        assert!(crl_pem.contains("BEGIN X509 CRL"));
    }

    #[test]
    fn test_sha256_fingerprint_format() {
        let data = b"hello world";
        let fp = sha256_fingerprint(data);
        // Should be colon-separated hex bytes
        let parts: Vec<_> = fp.split(':').collect();
        assert_eq!(parts.len(), 32, "SHA-256 fingerprint should have 32 bytes");
        for part in parts {
            assert_eq!(part.len(), 2, "Each byte should be 2 hex chars");
        }
    }
}
