//! Nebula-inspired x.509 certificate extensions for roles and groups.
//!
//! RockBot embeds authorization metadata directly in certificates using custom
//! x.509 v3 extensions under a private OID arc. This follows the same philosophy
//! as [Nebula](https://github.com/slackhq/nebula), where the certificate itself
//! is the single source of truth for identity and authorization — no external
//! directory lookups at connection time.
//!
//! ## OID Arc
//!
//! We use the IANA Private Enterprise Number (PEN) arc under `1.3.6.1.4.1`:
//!
//! ```text
//! 1.3.6.1.4.1.59584        — RockBot private arc (placeholder PEN)
//! 1.3.6.1.4.1.59584.1      — Certificate extensions
//! 1.3.6.1.4.1.59584.1.1    — Roles (SEQUENCE OF UTF8String)
//! 1.3.6.1.4.1.59584.1.2    — Groups (SEQUENCE OF UTF8String)
//! ```
//!
//! ## Encoding
//!
//! Both extensions use DER-encoded `SEQUENCE OF UTF8String`. The extensions are
//! marked non-critical so that TLS libraries that don't understand them will
//! still accept the certificate for transport-level authentication.
//!
//! ## Usage
//!
//! At the application layer, after TLS handshake, the gateway (or any peer) can
//! parse these extensions from the presented certificate to make authorization
//! decisions without consulting an external store.

use rcgen::CustomExtension;

/// OID for the Roles extension: `1.3.6.1.4.1.59584.1.1`
pub const OID_ROCKBOT_ROLES: &[u64] = &[1, 3, 6, 1, 4, 1, 59584, 1, 1];

/// OID for the Groups extension: `1.3.6.1.4.1.59584.1.2`
pub const OID_ROCKBOT_GROUPS: &[u64] = &[1, 3, 6, 1, 4, 1, 59584, 1, 2];

/// DER tag for UTF8String.
const TAG_UTF8STRING: u8 = 0x0C;

/// DER tag for SEQUENCE.
const TAG_SEQUENCE: u8 = 0x30;

/// Encode a list of strings as a DER `SEQUENCE OF UTF8String`.
fn encode_string_sequence(values: &[String]) -> Vec<u8> {
    // First, encode each string as a TLV (Tag-Length-Value)
    let mut items = Vec::new();
    for v in values {
        let bytes = v.as_bytes();
        let mut tlv = Vec::new();
        tlv.push(TAG_UTF8STRING);
        encode_der_length(bytes.len(), &mut tlv);
        tlv.extend_from_slice(bytes);
        items.extend(tlv);
    }

    // Wrap in a SEQUENCE
    let mut seq = Vec::new();
    seq.push(TAG_SEQUENCE);
    encode_der_length(items.len(), &mut seq);
    seq.extend(items);
    seq
}

/// Encode a DER length in the minimum number of octets.
fn encode_der_length(len: usize, out: &mut Vec<u8>) {
    if len < 128 {
        out.push(len as u8);
    } else if len < 256 {
        out.push(0x81);
        out.push(len as u8);
    } else {
        out.push(0x82);
        out.push((len >> 8) as u8);
        out.push(len as u8);
    }
}

/// Decode a DER `SEQUENCE OF UTF8String` back into a list of strings.
///
/// Returns an error if the encoding is malformed.
pub fn decode_string_sequence(der: &[u8]) -> anyhow::Result<Vec<String>> {
    if der.is_empty() {
        return Ok(Vec::new());
    }

    // Expect SEQUENCE tag
    if der[0] != TAG_SEQUENCE {
        anyhow::bail!("Expected SEQUENCE tag (0x30), got 0x{:02X}", der[0]);
    }

    let (seq_len, header_len) = decode_der_length(&der[1..])?;
    let seq_body = &der[header_len + 1..header_len + 1 + seq_len];

    let mut result = Vec::new();
    let mut pos = 0;
    while pos < seq_body.len() {
        if seq_body[pos] != TAG_UTF8STRING {
            anyhow::bail!(
                "Expected UTF8String tag (0x0C), got 0x{:02X} at offset {}",
                seq_body[pos],
                pos
            );
        }
        let (str_len, len_bytes) = decode_der_length(&seq_body[pos + 1..])?;
        let start = pos + 1 + len_bytes;
        let end = start + str_len;
        if end > seq_body.len() {
            anyhow::bail!("UTF8String length exceeds available data");
        }
        let s = std::str::from_utf8(&seq_body[start..end])
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in extension: {e}"))?;
        result.push(s.to_string());
        pos = end;
    }

    Ok(result)
}

/// Decode a DER length field. Returns `(length_value, number_of_bytes_consumed)`.
fn decode_der_length(data: &[u8]) -> anyhow::Result<(usize, usize)> {
    if data.is_empty() {
        anyhow::bail!("Unexpected end of DER data in length field");
    }
    let first = data[0];
    if first < 128 {
        Ok((first as usize, 1))
    } else if first == 0x81 {
        if data.len() < 2 {
            anyhow::bail!("Truncated DER length");
        }
        Ok((data[1] as usize, 2))
    } else if first == 0x82 {
        if data.len() < 3 {
            anyhow::bail!("Truncated DER length");
        }
        Ok((((data[1] as usize) << 8) | data[2] as usize, 3))
    } else {
        anyhow::bail!("Unsupported DER length encoding: 0x{first:02X}");
    }
}

/// Build rcgen `CustomExtension` entries for the given roles and groups.
///
/// Returns a (possibly empty) vec of extensions to add to `CertificateParams::custom_extensions`.
/// Extensions are non-critical — peers that don't understand them will ignore them.
pub fn build_extensions(roles: &[String], groups: &[String]) -> Vec<CustomExtension> {
    let mut exts = Vec::new();

    if !roles.is_empty() {
        let der = encode_string_sequence(roles);
        let mut ext = CustomExtension::from_oid_content(OID_ROCKBOT_ROLES, der);
        ext.set_criticality(false);
        exts.push(ext);
    }

    if !groups.is_empty() {
        let der = encode_string_sequence(groups);
        let mut ext = CustomExtension::from_oid_content(OID_ROCKBOT_GROUPS, der);
        ext.set_criticality(false);
        exts.push(ext);
    }

    exts
}

/// Parse RockBot roles and groups from a DER-encoded x.509 certificate.
///
/// Uses `x509-parser` to iterate over extensions and decode any that match
/// our private OIDs.
pub fn parse_extensions(cert_der: &[u8]) -> anyhow::Result<CertExtensions> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|e| anyhow::anyhow!("Failed to parse certificate: {e}"))?;

    let roles_oid = oid_to_x509_parser(OID_ROCKBOT_ROLES);
    let groups_oid = oid_to_x509_parser(OID_ROCKBOT_GROUPS);

    let mut roles = Vec::new();
    let mut groups = Vec::new();

    for ext in cert.extensions() {
        if ext.oid == roles_oid {
            roles = decode_string_sequence(ext.value)?;
        } else if ext.oid == groups_oid {
            groups = decode_string_sequence(ext.value)?;
        }
    }

    Ok(CertExtensions { roles, groups })
}

/// Roles and groups extracted from a certificate's custom extensions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CertExtensions {
    /// Authorization roles (e.g. "admin", "operator", "readonly").
    pub roles: Vec<String>,
    /// Group memberships (e.g. "engineering", "staging-cluster").
    pub groups: Vec<String>,
}

/// Convert our OID component slice to an `x509_parser::oid_registry::Oid` for comparison.
fn oid_to_x509_parser(components: &[u64]) -> x509_parser::oid_registry::Oid<'static> {
    // Oid::from takes &[u64] components directly
    x509_parser::oid_registry::Oid::from(components).expect("valid OID components")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn test_encode_decode_roundtrip_empty() {
        let der = encode_string_sequence(&[]);
        let decoded = decode_string_sequence(&der).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let values = vec![
            "admin".to_string(),
            "operator".to_string(),
            "readonly".to_string(),
        ];
        let der = encode_string_sequence(&values);
        let decoded = decode_string_sequence(&der).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn test_encode_decode_unicode() {
        let values = vec!["日本語".to_string(), "emoji-🚀".to_string()];
        let der = encode_string_sequence(&values);
        let decoded = decode_string_sequence(&der).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn test_build_extensions_empty() {
        let exts = build_extensions(&[], &[]);
        assert!(exts.is_empty());
    }

    #[test]
    fn test_build_extensions_roles_only() {
        let exts = build_extensions(&["admin".to_string()], &[]);
        assert_eq!(exts.len(), 1);
    }

    #[test]
    fn test_build_extensions_both() {
        let exts = build_extensions(&["admin".to_string()], &["engineering".to_string()]);
        assert_eq!(exts.len(), 2);
    }

    #[test]
    fn test_end_to_end_cert_extensions() {
        // Generate a CA and a client cert with roles+groups, then parse them back
        use crate::backend::{FileBackend, KeyBackend};
        use crate::ca;
        use crate::index::CertRole;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let backend = FileBackend::new(dir.path().to_path_buf());
        let ca_key = backend.generate("ca").unwrap();
        let ca_pem = ca::generate_ca(&ca_key, 365).unwrap();

        let client_key = backend.generate("test-agent").unwrap();
        let roles = vec!["admin".to_string(), "deploy".to_string()];
        let groups = vec!["engineering".to_string(), "us-west-2".to_string()];

        let (cert_pem, entry) = ca::generate_client_cert(
            &client_key,
            &ca_pem,
            &ca_key,
            "test-agent",
            CertRole::Agent,
            &[],
            365,
            2,
            &roles,
            &groups,
        )
        .unwrap();

        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert_eq!(entry.roles, roles);
        assert_eq!(entry.groups, groups);

        // Parse the extensions back from the cert DER
        let der = crate::manager::pem_to_der(&cert_pem).unwrap();
        let parsed = parse_extensions(&der).unwrap();
        assert_eq!(parsed.roles, roles);
        assert_eq!(parsed.groups, groups);
    }
}
