//! TLS certificate management commands

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use crate::{CertCommands, load_config};

/// Run certificate commands
pub async fn run(command: &CertCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        CertCommands::Generate {
            output_dir,
            san,
            days,
            force,
            update_config,
        } => {
            let dir = output_dir.clone().unwrap_or_else(|| {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf()
            });
            let cert_path = dir.join("gateway.crt");
            let key_path = dir.join("gateway.key");

            if cert_path.exists() && !force {
                anyhow::bail!(
                    "Certificate already exists: {}\nUse --force to overwrite",
                    cert_path.display()
                );
            }

            tokio::fs::create_dir_all(&dir).await?;
            generate_self_signed_cert(&cert_path, &key_path, san, *days).await?;
            println!("Generated certificate:");
            println!("  cert: {}", cert_path.display());
            println!("  key:  {}", key_path.display());

            if *update_config {
                update_config_tls(config_path, &cert_path, &key_path).await?;
                println!("Updated config: {}", config_path.display());
            }

            Ok(())
        }

        CertCommands::Info { cert } => {
            let cert_path = resolve_cert_path(cert.as_deref(), config_path).await?;
            show_cert_info(&cert_path).await
        }

        CertCommands::Rotate {
            san,
            days,
            backup,
        } => rotate_cert(config_path, san, *days, *backup).await,

        CertCommands::Import { cert, key, copy } => {
            import_cert(config_path, cert, key, *copy).await
        }

        CertCommands::Verify { cert, key } => {
            let (cert_path, key_path) =
                resolve_cert_key_paths(cert.as_deref(), key.as_deref(), config_path).await?;
            verify_cert_key(&cert_path, &key_path).await
        }
    }
}

/// Generate a self-signed TLS certificate with custom SANs and validity.
pub async fn generate_self_signed_cert(
    cert_path: &Path,
    key_path: &Path,
    extra_sans: &[String],
    days: u32,
) -> Result<()> {
    use rcgen::{CertificateParams, DnType, KeyPair, SanType};

    let mut san_names: Vec<SanType> = vec![
        SanType::DnsName("localhost".try_into()?),
        SanType::IpAddress("127.0.0.1".parse()?),
        SanType::IpAddress("::1".parse()?),
    ];

    // Include hostname
    if let Ok(hostname) = std::process::Command::new("hostname").output() {
        if let Ok(name) = String::from_utf8(hostname.stdout) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                san_names.push(SanType::DnsName(name.try_into()?));
            }
        }
    }

    // Add user-specified SANs
    for san in extra_sans {
        if let Ok(ip) = san.parse::<std::net::IpAddr>() {
            san_names.push(SanType::IpAddress(ip));
        } else {
            san_names.push(SanType::DnsName(san.clone().try_into()?));
        }
    }

    let key_pair = KeyPair::generate()?;
    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params.subject_alt_names = san_names;
    params.distinguished_name.push(DnType::CommonName, "RockBot Gateway");
    params.distinguished_name.push(DnType::OrganizationName, "RockBot");

    let now = time::OffsetDateTime::now_utc();
    params.not_before = now;
    params.not_after = now + time::Duration::days(i64::from(days));

    let cert = params.self_signed(&key_pair)?;

    tokio::fs::write(cert_path, cert.pem()).await?;
    tokio::fs::write(key_path, key_pair.serialize_pem()).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

/// Display certificate details.
async fn show_cert_info(cert_path: &Path) -> Result<()> {
    let pem_data = tokio::fs::read(cert_path)
        .await
        .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;

    let (_, pem) = x509_parser::pem::parse_x509_pem(&pem_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse PEM: {e}"))?;

    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse X.509 certificate: {e}"))?;

    println!("Certificate: {}", cert_path.display());
    println!("  Subject:    {}", cert.subject());
    println!("  Issuer:     {}", cert.issuer());
    println!(
        "  Not before: {}",
        cert.validity()
            .not_before
            .to_rfc2822()
            .unwrap_or_else(|_| format!("{:?}", cert.validity().not_before))
    );
    println!(
        "  Not after:  {}",
        cert.validity()
            .not_after
            .to_rfc2822()
            .unwrap_or_else(|_| format!("{:?}", cert.validity().not_after))
    );

    let now = x509_parser::time::ASN1Time::now();
    if cert.validity().not_after < now {
        println!("  Status:     EXPIRED");
    } else {
        let remaining = cert.validity().not_after.timestamp() - now.timestamp();
        let days = remaining / 86400;
        println!("  Status:     valid ({days} days remaining)");
    }

    // SANs
    if let Ok(Some(san_ext)) = cert.subject_alternative_name() {
        use x509_parser::extensions::GeneralName;
        let names: Vec<String> = san_ext
            .value
            .general_names
            .iter()
            .map(|n| match n {
                GeneralName::DNSName(dns) => dns.to_string(),
                GeneralName::IPAddress(bytes) => {
                    if bytes.len() == 4 {
                        format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
                    } else if bytes.len() == 16 {
                        let addr = std::net::Ipv6Addr::from(
                            <[u8; 16]>::try_from(*bytes).unwrap_or([0; 16]),
                        );
                        format!("{addr}")
                    } else {
                        format!("{n}")
                    }
                }
                other => format!("{other}"),
            })
            .collect();
        println!("  SANs:       {}", names.join(", "));
    }

    // Serial
    println!("  Serial:     {}", cert.raw_serial_as_string());

    // Signature algorithm
    println!(
        "  Signature:  {}",
        cert.signature_algorithm.algorithm
    );

    // Self-signed check
    let self_signed = cert.subject() == cert.issuer();
    println!("  Self-signed: {self_signed}");

    Ok(())
}

/// Rotate the certificate: backup old, generate new, update config.
async fn rotate_cert(
    config_path: &Path,
    extra_sans: &[String],
    days: u32,
    backup: bool,
) -> Result<()> {
    let config = load_config(&config_path.to_path_buf()).await?;

    let cert_path = config
        .gateway
        .tls_cert
        .as_ref()
        .context("No tls_cert configured — nothing to rotate")?
        .clone();
    let key_path = config
        .gateway
        .tls_key
        .as_ref()
        .context("No tls_key configured — nothing to rotate")?
        .clone();

    // Backup old files
    if backup {
        let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
        for path in [&cert_path, &key_path] {
            if path.exists() {
                let backup_path = path.with_extension(format!(
                    "{}.bak.{ts}",
                    path.extension().unwrap_or_default().to_string_lossy()
                ));
                tokio::fs::copy(path, &backup_path).await?;
                println!("Backed up: {} -> {}", path.display(), backup_path.display());
            }
        }
    }

    // Show old cert info before rotation
    if cert_path.exists() {
        println!("Old certificate:");
        show_cert_info(&cert_path).await?;
        println!();
    }

    generate_self_signed_cert(&cert_path, &key_path, extra_sans, days).await?;
    println!("Rotated certificate:");
    println!("  cert: {}", cert_path.display());
    println!("  key:  {}", key_path.display());

    // Show new cert info
    println!();
    println!("New certificate:");
    show_cert_info(&cert_path).await?;

    println!();
    println!("Restart the gateway to use the new certificate.");

    Ok(())
}

/// Import an external cert/key pair.
async fn import_cert(
    config_path: &Path,
    cert_src: &Path,
    key_src: &Path,
    copy: bool,
) -> Result<()> {
    // Validate the cert and key before importing
    verify_cert_key(cert_src, key_src).await?;

    let (cert_path, key_path) = if copy {
        let config_dir = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let cert_dest = config_dir.join("gateway.crt");
        let key_dest = config_dir.join("gateway.key");
        tokio::fs::copy(cert_src, &cert_dest).await?;
        tokio::fs::copy(key_src, &key_dest).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_dest, std::fs::Permissions::from_mode(0o600))?;
        }

        println!("Copied certificate files to config directory:");
        println!("  cert: {}", cert_dest.display());
        println!("  key:  {}", key_dest.display());
        (cert_dest, key_dest)
    } else {
        println!("Referencing certificate files in-place:");
        println!("  cert: {}", cert_src.display());
        println!("  key:  {}", key_src.display());
        (cert_src.to_path_buf(), key_src.to_path_buf())
    };

    update_config_tls(config_path, &cert_path, &key_path).await?;
    println!("Updated config: {}", config_path.display());

    println!();
    show_cert_info(&cert_path).await?;

    println!();
    println!("Restart the gateway to use the new certificate.");

    Ok(())
}

/// Verify that a cert and key are valid PEM and that the key matches the cert.
async fn verify_cert_key(cert_path: &Path, key_path: &Path) -> Result<()> {
    let cert_pem = tokio::fs::read(cert_path)
        .await
        .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;
    let key_pem = tokio::fs::read(key_path)
        .await
        .with_context(|| format!("Failed to read key: {}", key_path.display()))?;

    // Parse certificate
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to parse PEM certificate")?;

    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", cert_path.display());
    }
    println!("Certificate: {} ({} cert(s) in chain)", cert_path.display(), certs.len());

    // Parse private key
    let key = rustls_pemfile::private_key(&mut &key_pem[..])
        .context("Failed to parse PEM private key")?
        .context("No private key found in file")?;
    println!("Private key: {} (OK)", key_path.display());

    // Try to build a rustls ServerConfig — this validates the cert/key pair match
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key);

    match config {
        Ok(_) => {
            println!("Verification: certificate and key MATCH");

            // Also show cert details
            let (_, pem) = x509_parser::pem::parse_x509_pem(&cert_pem)
                .map_err(|e| anyhow::anyhow!("Failed to parse PEM: {e}"))?;
            let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
                .map_err(|e| anyhow::anyhow!("Failed to parse X.509: {e}"))?;

            let now = x509_parser::time::ASN1Time::now();
            if cert.validity().not_after < now {
                println!("Warning:      certificate is EXPIRED");
            } else {
                let remaining = cert.validity().not_after.timestamp() - now.timestamp();
                let days = remaining / 86400;
                println!("Expiry:       {days} days remaining");
            }

            Ok(())
        }
        Err(e) => {
            anyhow::bail!("Verification FAILED: certificate and key do not match: {e}");
        }
    }
}

/// Update the tls_cert and tls_key fields in the TOML config file.
async fn update_config_tls(
    config_path: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<()> {
    let content = tokio::fs::read_to_string(config_path)
        .await
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .context("Failed to parse config as TOML")?;

    // Ensure [gateway] table exists
    if doc.get("gateway").is_none() {
        doc["gateway"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    doc["gateway"]["tls_cert"] =
        toml_edit::value(cert_path.to_string_lossy().as_ref());
    doc["gateway"]["tls_key"] =
        toml_edit::value(key_path.to_string_lossy().as_ref());

    tokio::fs::write(config_path, doc.to_string()).await?;

    Ok(())
}

/// Expand a leading `~` or `~/` to the user's home directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" || s.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    path.to_path_buf()
}

/// Resolve the cert path from an explicit argument or from the config.
async fn resolve_cert_path(
    explicit: Option<&Path>,
    config_path: &Path,
) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(expand_tilde(p));
    }
    let config = load_config(&config_path.to_path_buf()).await?;
    config
        .gateway
        .tls_cert
        .map(|p| expand_tilde(&p))
        .context("No --cert provided and no tls_cert in config")
}

/// Resolve both cert and key paths from explicit arguments or from config.
async fn resolve_cert_key_paths(
    cert: Option<&Path>,
    key: Option<&Path>,
    config_path: &Path,
) -> Result<(PathBuf, PathBuf)> {
    match (cert, key) {
        (Some(c), Some(k)) => Ok((expand_tilde(c), expand_tilde(k))),
        _ => {
            let config = load_config(&config_path.to_path_buf()).await?;
            let c = cert
                .map(|p| expand_tilde(p))
                .or(config.gateway.tls_cert.map(|p| expand_tilde(&p)))
                .context("No --cert provided and no tls_cert in config")?;
            let k = key
                .map(|p| expand_tilde(p))
                .or(config.gateway.tls_key.map(|p| expand_tilde(&p)))
                .context("No --key provided and no tls_key in config")?;
            Ok((c, k))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn install_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[tokio::test]
    async fn test_generate_and_verify() {
        install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test.crt");
        let key_path = dir.path().join("test.key");

        generate_self_signed_cert(&cert_path, &key_path, &[], 30)
            .await
            .unwrap();

        assert!(cert_path.exists());
        assert!(key_path.exists());

        // Should verify successfully
        verify_cert_key(&cert_path, &key_path).await.unwrap();
    }

    #[tokio::test]
    async fn test_generate_with_extra_sans() {
        install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("test.crt");
        let key_path = dir.path().join("test.key");

        let sans = vec!["example.local".to_string(), "192.168.1.100".to_string()];
        generate_self_signed_cert(&cert_path, &key_path, &sans, 365)
            .await
            .unwrap();

        // Parse and check SANs are present
        let pem_data: Vec<u8> = tokio::fs::read(&cert_path).await.unwrap();
        let (_, pem) = x509_parser::pem::parse_x509_pem(&pem_data).unwrap();
        let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).unwrap();
        let san_ext = cert.subject_alternative_name().unwrap().unwrap();
        let san_strs: Vec<String> = san_ext
            .value
            .general_names
            .iter()
            .map(|n| format!("{n}"))
            .collect();

        assert!(san_strs.iter().any(|s| s.contains("example.local")));
        // x509-parser formats IPs as hex bytes; 192.168.1.100 = c0:a8:01:64
        assert!(san_strs.iter().any(|s| s.contains("c0:a8:01:64")));
    }

    #[tokio::test]
    async fn test_verify_mismatched_key_fails() {
        install_crypto_provider();
        let dir = tempfile::tempdir().unwrap();

        // Generate two independent cert/key pairs
        let cert1 = dir.path().join("cert1.crt");
        let key1 = dir.path().join("key1.key");
        generate_self_signed_cert(&cert1, &key1, &[], 30)
            .await
            .unwrap();

        let cert2 = dir.path().join("cert2.crt");
        let key2 = dir.path().join("key2.key");
        generate_self_signed_cert(&cert2, &key2, &[], 30)
            .await
            .unwrap();

        // cert1 + key2 should fail
        let result = verify_cert_key(&cert1, &key2).await;
        assert!(result.is_err());
    }
}
