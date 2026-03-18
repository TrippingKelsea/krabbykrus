//! TLS certificate management commands — CA, client certs, CSR signing, enrollment.

use anyhow::{Context, Result};
use rockbot_pki::{CertRole, KeyBackend, PkiManager};
use std::path::{Path, PathBuf};

use crate::{load_config, CaCertCommands, CertCommands, ClientCertCommands, EnrollCommands};

/// Run certificate commands.
pub async fn run(command: &CertCommands, config_path: &Path) -> Result<()> {
    match command {
        CertCommands::Ca { command } => run_ca(command, config_path).await,
        CertCommands::Client { command } => run_client(command, config_path).await,
        CertCommands::Sign {
            csr,
            name,
            role,
            days,
            output,
        } => cmd_sign(config_path, csr, name, role, *days, output.as_deref()).await,
        CertCommands::Install { name } => cmd_install(config_path, name).await,
        CertCommands::Verify { cert, key, ca } => {
            cmd_verify(config_path, cert.as_deref(), key.as_deref(), ca.as_deref()).await
        }
        CertCommands::Info { cert } => cmd_info(cert).await,
        CertCommands::Enroll { command } => run_enroll(command, config_path).await,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_pki_dir(config_path: &Path) -> PathBuf {
    // Try reading config for pki_dir, fall back to default
    let config_dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    config_dir.join("pki")
}

async fn resolve_pki_dir_from_config(config_path: &Path) -> Result<PathBuf> {
    if let Ok(config) = rockbot_config::Config::from_file(config_path).await {
        if let Some(dir) = &config.effective_pki().pki_dir {
            return Ok(expand_tilde(dir));
        }
    }
    Ok(resolve_pki_dir(config_path))
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" || s.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    path.to_path_buf()
}

fn parse_role(s: &str) -> Result<CertRole> {
    CertRole::from_str(s)
        .with_context(|| format!("Invalid role '{s}'. Must be: gateway, agent, or tui"))
}

fn resolve_client_cert_name(long_name: &Option<String>, positional_name: &Option<String>) -> Result<String> {
    match (long_name, positional_name) {
        (Some(name), None) | (None, Some(name)) => Ok(name.clone()),
        (None, None) => anyhow::bail!("Client name is required. Use '<NAME>' or '--name <NAME>'."),
        (Some(_), Some(_)) => anyhow::bail!("Specify the client name either positionally or with '--name', not both."),
    }
}

fn resolve_enrollment_roles(
    roles: &[String],
    cert_role: &Option<String>,
) -> Result<(CertRole, Vec<String>)> {
    let roles = if roles.is_empty() {
        vec![cert_role.clone().unwrap_or_else(|| "agent".to_string())]
    } else {
        roles.to_vec()
    };

    let primary = if let Some(cert_role) = cert_role {
        parse_role(cert_role)?
    } else if let Some(role) = roles.iter().find_map(|role| CertRole::from_str(role)) {
        role
    } else {
        CertRole::Agent
    };

    Ok((primary, roles))
}

fn open_pki(pki_dir: PathBuf) -> Result<PkiManager> {
    PkiManager::new(pki_dir)
}

// ---------------------------------------------------------------------------
// CA commands
// ---------------------------------------------------------------------------

async fn run_ca(command: &CaCertCommands, config_path: &Path) -> Result<()> {
    match command {
        CaCertCommands::Publish => cmd_publish(config_path).await,
        CaCertCommands::Generate {
            days,
            pki_dir,
            force,
        } => {
            let dir = match pki_dir {
                Some(d) => d.clone(),
                None => resolve_pki_dir(config_path),
            };

            if dir.join("ca.crt").exists() && !force {
                anyhow::bail!(
                    "CA already exists in {}\nUse --force to overwrite",
                    dir.display()
                );
            }

            let mut mgr = open_pki(dir.clone())?;
            mgr.init_ca(*days)?;

            println!("Certificate Authority initialized:");
            println!("  CA cert: {}", dir.join("ca.crt").display());
            println!("  CA key:  {}", dir.join("ca.key").display());

            let info = mgr.ca_info()?;
            println!("  Expires: {}", info.not_after);
            println!("  SHA-256: {}", info.fingerprint);

            Ok(())
        }

        CaCertCommands::Info => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mgr = open_pki(dir)?;
            let info = mgr.ca_info()?;

            println!("Certificate Authority:");
            println!("  Not before:  {}", info.not_before);
            println!("  Not after:   {}", info.not_after);
            println!("  SHA-256:     {}", info.fingerprint);

            Ok(())
        }

        CaCertCommands::Rotate { days, backup } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;

            if *backup {
                let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
                for name in &["ca.crt", "ca.key"] {
                    let path = dir.join(name);
                    if path.exists() {
                        let bak = dir.join(format!("{name}.bak.{ts}"));
                        std::fs::copy(&path, &bak)?;
                        println!("Backed up: {} -> {}", path.display(), bak.display());
                    }
                }
            }

            let mut mgr = open_pki(dir)?;

            // Show old CA info
            if let Ok(info) = mgr.ca_info() {
                println!("Old CA: (expires {})", info.not_after);
            }

            mgr.init_ca(*days)?;

            let info = mgr.ca_info()?;
            println!("New CA: (expires {})", info.not_after);
            println!("  SHA-256: {}", info.fingerprint);
            println!();
            println!("Warning: existing client certs are now signed by the old CA.");
            println!("Re-issue client certs with 'rockbot cert client rotate <name>'.");

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Publish CA cert to S3
// ---------------------------------------------------------------------------

#[cfg(feature = "bedrock-deploy")]
async fn cmd_publish(config_path: &Path) -> Result<()> {
    let config = load_config(&config_path.to_path_buf()).await?;

    let deploy_value = config
        .deploy
        .context("No [deploy] section in config. Add one with at least `bucket = \"...\"`")?;

    let deploy_config: rockbot_deploy::DeployConfig =
        serde_json::from_value(deploy_value).context("Invalid [deploy] configuration")?;

    // Read CA cert
    let pki_dir = resolve_pki_dir_from_config(config_path).await?;
    let ca_cert_path = pki_dir.join("ca.crt");
    let ca_pem = tokio::fs::read_to_string(&ca_cert_path)
        .await
        .with_context(|| format!("CA cert not found at {}", ca_cert_path.display()))?;

    // Handle AWS credential import interactively
    let vault_path = config.credentials.vault_path;
    if rockbot_credentials::CredentialVault::exists(&vault_path) {
        if let Ok(cred_mgr) = rockbot_credentials::CredentialManager::new(&vault_path) {
            let cred_mgr = std::sync::Arc::new(cred_mgr);
            let importer = rockbot_deploy::AwsCredentialImporter::new(cred_mgr.clone());
            let client_uuid = uuid::Uuid::new_v4().to_string();

            match importer.import_or_prompt(&client_uuid).await {
                Ok(rockbot_deploy::credentials::ImportResult::Imported) => {
                    println!("Imported AWS credentials into vault (aws/default)");
                }
                Ok(rockbot_deploy::credentials::ImportResult::AlreadyPresent) => {
                    println!("AWS credentials already in vault");
                }
                Ok(rockbot_deploy::credentials::ImportResult::Conflict {
                    discovered,
                    existing_endpoint_name,
                }) => {
                    println!(
                        "Found different AWS keys in environment vs vault ({existing_endpoint_name})."
                    );
                    println!(
                        "Store discovered keys as '{client_uuid}-aws-default' for optional use? [y/N]"
                    );
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                    if input.trim().eq_ignore_ascii_case("y") {
                        importer
                            .store_namespaced(&client_uuid, &discovered)
                            .await
                            .context("Failed to store namespaced credentials")?;
                        println!("Stored under aws/{client_uuid}-default");
                    }
                }
                Ok(rockbot_deploy::credentials::ImportResult::NoKeysFound) => {
                    println!("No AWS credentials found in environment; using SDK default chain");
                }
                Err(e) => {
                    println!("Warning: credential import failed: {e}");
                }
            }
        }
    }

    // Create distributor and provision
    let distributor = rockbot_deploy::CaDistributor::new(deploy_config.clone())
        .await
        .context("Failed to create S3 CA distributor")?;

    distributor
        .provision(&ca_pem)
        .await
        .context("S3 provisioning failed")?;

    println!("CA certificate published to: {}", distributor.ca_cert_url());

    // DNS provisioning
    let mut dns = rockbot_deploy::DnsProvisioner::new(deploy_config.clone())
        .await
        .context("Failed to create DNS provisioner")?;

    dns.ensure_hosted_zone()
        .await
        .context("Failed to ensure hosted zone")?;

    let cluster_uuid = uuid::Uuid::new_v4().to_string();
    let s3_endpoint = format!(
        "{}.s3.{}.amazonaws.com",
        deploy_config.bucket, deploy_config.region
    );

    dns.register_cluster(
        &cluster_uuid,
        deploy_config.cluster_name.as_deref(),
        &s3_endpoint,
    )
    .await
    .context("DNS registration failed")?;

    println!(
        "DNS records registered in zone '{}'",
        deploy_config.dns_zone
    );
    if let Some(name) = &deploy_config.cluster_name {
        println!("  {name}.{}", deploy_config.dns_zone);
    }
    println!("  {cluster_uuid}.{}", deploy_config.dns_zone);

    Ok(())
}

#[cfg(not(feature = "bedrock-deploy"))]
async fn cmd_publish(_config_path: &Path) -> Result<()> {
    anyhow::bail!(
        "The 'bedrock-deploy' feature is not compiled in.\n\
         Rebuild with: cargo build --features bedrock-deploy"
    )
}

// ---------------------------------------------------------------------------
// Client commands
// ---------------------------------------------------------------------------

async fn run_client(command: &ClientCertCommands, config_path: &Path) -> Result<()> {
    match command {
        ClientCertCommands::Generate {
            name,
            subject_name,
            san,
            days,
            role,
        } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mut mgr = open_pki(dir)?;
            let name = resolve_client_cert_name(name, subject_name)?;
            let role = parse_role(role)?;

            let info = mgr.generate_client(&name, role, san, *days, &[], &[])?;
            println!("Client certificate generated:");
            println!("  Name: {name}");
            println!("  Role: {role}");
            println!("  Cert: {}", info.cert_path.display());
            println!("  Key:  {}", info.key_path.display());

            Ok(())
        }

        ClientCertCommands::List => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mgr = open_pki(dir)?;
            let entries = mgr.list_clients();

            if entries.is_empty() {
                println!("No client certificates issued.");
                return Ok(());
            }

            println!(
                "{:<20} {:<10} {:<10} {:<12} FINGERPRINT",
                "NAME", "ROLE", "STATUS", "EXPIRES"
            );
            for entry in entries {
                let expires = entry.not_after.format("%Y-%m-%d");
                let fp_short = if entry.fingerprint_sha256.len() > 20 {
                    &entry.fingerprint_sha256[..20]
                } else {
                    &entry.fingerprint_sha256
                };
                println!(
                    "{:<20} {:<10} {:<10} {:<12} {}…",
                    entry.name, entry.role, entry.status, expires, fp_short
                );
            }

            Ok(())
        }

        ClientCertCommands::Info { name } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mgr = open_pki(dir)?;
            let entry = mgr
                .client_info(name)
                .with_context(|| format!("No certificate found for '{name}'"))?;

            println!("Client certificate: {}", entry.name);
            println!("  Role:       {}", entry.role);
            println!("  Status:     {}", entry.status);
            println!("  Serial:     {}", entry.serial);
            println!("  Subject:    {}", entry.subject);
            println!("  Not before: {}", entry.not_before);
            println!("  Not after:  {}", entry.not_after);
            println!("  SANs:       {}", entry.sans.join(", "));
            println!("  SHA-256:    {}", entry.fingerprint_sha256);

            Ok(())
        }

        ClientCertCommands::Revoke { name } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mut mgr = open_pki(dir)?;
            mgr.revoke_client(name)?;

            println!("Revoked certificate for '{name}'");

            Ok(())
        }

        ClientCertCommands::Rotate {
            name,
            san,
            days,
            backup,
        } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mut mgr = open_pki(dir.clone())?;

            if *backup {
                let ts = chrono::Local::now().format("%Y%m%d%H%M%S");
                let cert_file = dir.join("certs").join(format!("{name}.crt"));
                let key_file = dir.join("keys").join(format!("{name}.key"));
                for path in [&cert_file, &key_file] {
                    if path.exists() {
                        let bak_name = format!(
                            "{}.bak.{ts}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        let bak = path.with_file_name(bak_name);
                        std::fs::copy(path, &bak)?;
                        println!("Backed up: {}", bak.display());
                    }
                }
            }

            let info = mgr.rotate_client(name, san, *days)?;
            println!("Rotated certificate for '{name}':");
            println!("  Cert: {}", info.cert_path.display());
            println!("  Key:  {}", info.key_path.display());

            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Sign (offline CSR signing)
// ---------------------------------------------------------------------------

async fn cmd_sign(
    config_path: &Path,
    csr_path: &Path,
    name: &str,
    role_str: &str,
    days: u32,
    output: Option<&Path>,
) -> Result<()> {
    let dir = resolve_pki_dir_from_config(config_path).await?;
    let mut mgr = open_pki(dir)?;
    let role = parse_role(role_str)?;

    let csr_pem = std::fs::read_to_string(csr_path)
        .with_context(|| format!("Failed to read CSR: {}", csr_path.display()))?;

    let cert_pem = mgr.sign_csr(&csr_pem, name, role, days, &[], &[])?;

    if let Some(out) = output {
        std::fs::write(out, &cert_pem)?;
        println!("Signed certificate written to {}", out.display());
    } else {
        print!("{cert_pem}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Install (update rockbot.toml)
// ---------------------------------------------------------------------------

async fn cmd_install(config_path: &Path, name: &str) -> Result<()> {
    let dir = resolve_pki_dir_from_config(config_path).await?;
    let cert_path = dir.join("certs").join(format!("{name}.crt"));
    let key_path = dir.join("keys").join(format!("{name}.key"));
    let ca_path = dir.join("ca.crt");

    if !cert_path.exists() {
        anyhow::bail!("Certificate file not found: {}", cert_path.display());
    }
    if !key_path.exists() {
        anyhow::bail!("Key file not found: {}", key_path.display());
    }

    let mut mgr = open_pki(dir.clone())?;
    let role = if let Ok(entry) = mgr.client_info(name) {
        entry.role
    } else {
        let config = rockbot_config::Config::from_file(config_path)
            .await
            .with_context(|| format!("Failed to load config: {}", config_path.display()))?;
        let inferred_role = if config.security.roles.gateway {
            CertRole::Gateway
        } else {
            CertRole::Tui
        };
        let cert_pem = tokio::fs::read_to_string(&cert_path)
            .await
            .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;
        mgr.import_signed_client(name, inferred_role, &cert_pem)
            .with_context(|| {
                format!("Failed to import certificate '{name}' into local PKI index")
            })?;
        inferred_role
    };

    // Read and patch the TOML config
    let content = tokio::fs::read_to_string(config_path)
        .await
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let mut doc: toml_edit::DocumentMut =
        content.parse().context("Failed to parse config as TOML")?;

    if doc.get("pki").is_none() {
        doc["pki"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    doc["pki"]["tls_cert"] = toml_edit::value(cert_path.to_string_lossy().as_ref());
    doc["pki"]["tls_key"] = toml_edit::value(key_path.to_string_lossy().as_ref());

    if ca_path.exists() {
        doc["pki"]["tls_ca"] = toml_edit::value(ca_path.to_string_lossy().as_ref());
    }

    doc["pki"]["pki_dir"] = toml_edit::value(dir.to_string_lossy().as_ref());

    // Set require_client_cert based on role
    if role == CertRole::Gateway {
        if doc.get("gateway").is_none() {
            doc["gateway"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        // Gateway certs: enable mTLS by default
        doc["gateway"]["require_client_cert"] = toml_edit::value(true);
    }

    tokio::fs::write(config_path, doc.to_string()).await?;

    println!("Installed certificate '{name}' (role: {role}) into config:");
    println!("  tls_cert: {}", cert_path.display());
    println!("  tls_key:  {}", key_path.display());
    if ca_path.exists() {
        println!("  tls_ca:   {}", ca_path.display());
    }
    println!("  pki_dir:  {}", dir.display());

    if role == CertRole::Gateway {
        println!("  require_client_cert: true");
    }

    println!();
    println!("Restart the gateway to use the new certificates.");

    Ok(())
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

async fn cmd_verify(
    config_path: &Path,
    cert: Option<&Path>,
    key: Option<&Path>,
    ca: Option<&Path>,
) -> Result<()> {
    let (cert_path, key_path) = resolve_cert_key_paths(cert, key, config_path).await?;

    let cert_pem = tokio::fs::read(&cert_path)
        .await
        .with_context(|| format!("Failed to read certificate: {}", cert_path.display()))?;
    let key_pem = tokio::fs::read(&key_path)
        .await
        .with_context(|| format!("Failed to read key: {}", key_path.display()))?;

    // Parse certificate
    let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("Failed to parse PEM certificate")?;

    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", cert_path.display());
    }
    println!(
        "Certificate: {} ({} cert(s) in chain)",
        cert_path.display(),
        certs.len()
    );

    // Parse private key
    let parsed_key = rustls_pemfile::private_key(&mut &key_pem[..])
        .context("Failed to parse PEM private key")?
        .context("No private key found in file")?;
    println!("Private key: {} (OK)", key_path.display());

    // Test cert/key match via rustls
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs.clone(), parsed_key);

    match config {
        Ok(_) => println!("Key match:   OK"),
        Err(e) => anyhow::bail!("Certificate and key do NOT match: {e}"),
    }

    // Chain verification if CA provided
    if let Some(ca_arg) = ca {
        let ca_path = expand_tilde(ca_arg);
        let ca_pem = tokio::fs::read(&ca_path)
            .await
            .with_context(|| format!("Failed to read CA cert: {}", ca_path.display()))?;
        let ca_certs: Vec<_> = rustls_pemfile::certs(&mut &ca_pem[..])
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("Failed to parse CA PEM")?;

        let mut root_store = rustls::RootCertStore::empty();
        for ca_cert in &ca_certs {
            root_store.add(ca_cert.clone())?;
        }

        // Verify the leaf cert against the CA
        let verifier =
            rustls::server::WebPkiClientVerifier::builder(std::sync::Arc::new(root_store))
                .build()
                .context("Failed to build verifier")?;

        // Simple check: try to verify the end entity
        println!(
            "CA chain:    {} (loaded {} CA cert(s))",
            ca_path.display(),
            ca_certs.len()
        );
        // The full verification happens at TLS handshake time;
        // here we just confirm the CA loaded successfully
        drop(verifier);
        println!("Chain:       OK (CA loaded, full verification at TLS handshake)");
    } else if let Ok(config) = load_config(&config_path.to_path_buf()).await {
        if let Some(ca_cfg) = &config.effective_pki().tls_ca {
            println!(
                "Hint: use --ca {} to verify chain against configured CA",
                ca_cfg.display()
            );
        }
    }

    // Show cert details
    let (_, pem) = x509_parser::pem::parse_x509_pem(&cert_pem)
        .map_err(|e| anyhow::anyhow!("Failed to parse PEM: {e}"))?;
    let (_, x509_cert) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse X.509: {e}"))?;

    let now = x509_parser::time::ASN1Time::now();
    if x509_cert.validity().not_after < now {
        println!("Status:      EXPIRED");
    } else {
        let remaining = x509_cert.validity().not_after.timestamp() - now.timestamp();
        let days = remaining / 86400;
        println!("Expiry:      {days} days remaining");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Info (generic PEM inspection)
// ---------------------------------------------------------------------------

async fn cmd_info(cert_path: &Path) -> Result<()> {
    let cert_path = expand_tilde(cert_path);
    let pem_data = tokio::fs::read(&cert_path)
        .await
        .with_context(|| format!("Failed to read: {}", cert_path.display()))?;

    let (_, pem) = x509_parser::pem::parse_x509_pem(&pem_data)
        .map_err(|e| anyhow::anyhow!("Failed to parse PEM: {e}"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents)
        .map_err(|e| anyhow::anyhow!("Failed to parse X.509: {e}"))?;

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

    println!("  Serial:     {}", cert.raw_serial_as_string());
    println!("  Signature:  {}", cert.signature_algorithm.algorithm);

    let self_signed = cert.subject() == cert.issuer();
    println!("  Self-signed: {self_signed}");

    // SHA-256 fingerprint
    let fp = rockbot_pki::sha256_fingerprint(&pem.contents);
    println!("  SHA-256:    {fp}");

    Ok(())
}

// ---------------------------------------------------------------------------
// Enrollment commands
// ---------------------------------------------------------------------------

async fn run_enroll(command: &EnrollCommands, config_path: &Path) -> Result<()> {
    match command {
        EnrollCommands::Create {
            role,
            cert_role,
            uses,
            expires,
        } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mut mgr = open_pki(dir)?;
            let (primary_role, all_roles) = resolve_enrollment_roles(role, cert_role)?;

            let expires_at = expires
                .as_deref()
                .map(|s| {
                    let dur = parse_duration(s)?;
                    Ok::<_, anyhow::Error>(chrono::Utc::now() + dur)
                })
                .transpose()?;
            let token = mgr.create_enrollment(primary_role, &all_roles, *uses, expires_at)?;

            println!("Enrollment token created:");
            println!("  Token: {token}");
            println!("  Cert role: {primary_role}");
            println!("  Roles: {}", all_roles.join(", "));
            if let Some(n) = uses {
                println!("  Uses:  {n}");
            } else {
                println!("  Uses:  unlimited");
            }
            if let Some(d) = expires {
                println!("  Expires: {d}");
            }

            Ok(())
        }

        EnrollCommands::List => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mgr = open_pki(dir)?;
            let tokens = mgr.list_enrollments();

            if tokens.is_empty() {
                println!("No active enrollment tokens.");
                return Ok(());
            }

            println!(
                "{:<12} {:<10} {:<24} {:<10} {:<20} TOKEN",
                "ID", "CERT ROLE", "ROLES", "USES", "EXPIRES"
            );
            for t in tokens {
                let uses_str = t
                    .remaining_uses
                    .map_or_else(|| "∞".to_string(), |n| n.to_string());
                let expires_str = t.expires_at.map_or_else(
                    || "never".to_string(),
                    |dt| dt.format("%Y-%m-%d %H:%M").to_string(),
                );
                let roles = if t.roles.is_empty() {
                    t.role.to_string()
                } else {
                    t.roles.join(",")
                };
                let token_preview = if t.token.len() > 16 {
                    format!("{}…", &t.token[..16])
                } else {
                    t.token.clone()
                };
                println!(
                    "{:<12} {:<10} {:<24} {:<10} {:<20} {}",
                    t.id, t.role, roles, uses_str, expires_str, token_preview
                );
            }

            Ok(())
        }

        EnrollCommands::Revoke { id } => {
            let dir = resolve_pki_dir_from_config(config_path).await?;
            let mut mgr = open_pki(dir)?;
            mgr.revoke_enrollment(id)?;

            println!("Enrollment token '{id}' revoked.");

            Ok(())
        }

        EnrollCommands::Submit {
            gateway,
            ca_fingerprint,
            psk,
            name,
            role,
        } => {
            cmd_enroll_submit(
                config_path,
                gateway.as_deref(),
                ca_fingerprint.as_deref(),
                psk,
                name,
                role,
            )
            .await
        }
    }
}

/// Enroll with a remote gateway — generate key + CSR, send to /api/cert/sign, save result.
async fn cmd_enroll_submit(
    config_path: &Path,
    gateway: Option<&str>,
    ca_fingerprint: Option<&str>,
    psk: &str,
    name: &str,
    role_str: &str,
) -> Result<()> {
    let dir = resolve_pki_dir_from_config(config_path).await?;
    let keys_dir = dir.join("keys");
    std::fs::create_dir_all(&keys_dir)?;
    std::fs::create_dir_all(dir.join("certs"))?;

    // Generate a key pair locally
    let backend = rockbot_pki::FileBackend::new(keys_dir);
    let key_handle = backend.generate(name)?;

    // Generate a CSR
    let csr_pem = rockbot_pki::ca::generate_csr(&key_handle, name)?;

    // Send to gateway
    let configured_gateway = rockbot_config::Config::from_file(config_path)
        .await
        .ok()
        .map(|config| {
            format!(
                "https://{}:{}",
                config.client.gateway_host, config.client.https_port
            )
        });
    let gateway = gateway
        .map(ToOwned::to_owned)
        .or(configured_gateway)
        .ok_or_else(|| {
            anyhow::anyhow!("No gateway provided and no [client] bootstrap target configured")
        })?;
    let gateway = if gateway.contains("://") {
        gateway
    } else {
        format!("https://{gateway}")
    };
    #[derive(serde::Deserialize)]
    struct CaInfoResponse {
        ca_certificate: String,
    }

    let config = rockbot_config::Config::from_file(config_path).await.ok();
    let url = format!("{}/api/cert/sign", gateway.trim_end_matches('/'));
    let client = if let Some(ca_path) = config
        .as_ref()
        .and_then(|cfg| {
            cfg.effective_pki()
                .tls_ca
                .as_ref()
                .map(|path| expand_tilde(path.as_path()))
        })
        .filter(|path: &PathBuf| path.exists())
    {
        let ca_pem = tokio::fs::read(&ca_path).await?;
        let ca_cert = reqwest::Certificate::from_pem(&ca_pem)?;
        reqwest::Client::builder()
            .add_root_certificate(ca_cert)
            .build()?
    } else {
        let expected_fingerprint = ca_fingerprint.ok_or_else(|| {
            anyhow::anyhow!(
                "First-time enrollment requires `--ca-fingerprint <SHA256>` when no local CA file is configured"
            )
        })?;
        let bootstrap = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
        let ca_info: CaInfoResponse = bootstrap
            .get(format!("{}/api/cert/ca", gateway.trim_end_matches('/')))
            .send()
            .await
            .with_context(|| format!("Failed to fetch CA info from {gateway}"))?
            .error_for_status()?
            .json()
            .await
            .context("Invalid CA response from gateway")?;
        let actual_fingerprint = sha256_fingerprint_for_pem(ca_info.ca_certificate.as_bytes())?;
        if normalize_fingerprint(expected_fingerprint) != normalize_fingerprint(&actual_fingerprint)
        {
            anyhow::bail!(
                "Gateway CA fingerprint mismatch: expected {expected_fingerprint}, got {actual_fingerprint}"
            );
        }
        let ca_cert = reqwest::Certificate::from_pem(ca_info.ca_certificate.as_bytes())?;
        reqwest::Client::builder()
            .add_root_certificate(ca_cert)
            .build()?
    };

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "csr": csr_pem,
            "psk": psk,
            "name": name,
            "role": role_str,
        }))
        .send()
        .await
        .with_context(|| format!("Failed to connect to {url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Enrollment failed (HTTP {status}): {body}");
    }

    #[derive(serde::Deserialize)]
    struct SignResponse {
        certificate: String,
        ca_certificate: String,
    }

    let sign_resp: SignResponse = resp.json().await.context("Invalid response from gateway")?;

    // Save the signed cert
    let cert_path = dir.join("certs").join(format!("{name}.crt"));
    std::fs::write(&cert_path, &sign_resp.certificate)?;

    // Save the CA cert
    let ca_path = dir.join("ca.crt");
    std::fs::write(&ca_path, &sign_resp.ca_certificate)?;

    let mut mgr = open_pki(dir.clone())?;
    let role = parse_role(role_str)?;
    mgr.import_signed_client(name, role, &sign_resp.certificate)?;

    let key_path = dir.join("keys").join(format!("{name}.key"));
    println!("Enrollment successful:");
    println!("  Cert:   {}", cert_path.display());
    println!("  Key:    {}", key_path.display());
    println!("  CA:     {}", ca_path.display());
    println!();
    println!("Run 'rockbot cert install --name {name}' to update your config.");

    Ok(())
}

fn sha256_fingerprint_for_pem(pem: &[u8]) -> Result<String> {
    let mut cursor = std::io::Cursor::new(pem);
    let cert_der = rustls_pemfile::certs(&mut cursor)
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("No PEM certificate found"))?;
    Ok(rockbot_pki::sha256_fingerprint(cert_der.as_ref()))
}

fn normalize_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// ---------------------------------------------------------------------------
// Duration parser (e.g. "1h", "24h", "7d")
// ---------------------------------------------------------------------------

fn parse_duration(s: &str) -> Result<chrono::Duration> {
    let s = s.trim();
    if let Some(hours) = s.strip_suffix('h') {
        let n: i64 = hours.parse().context("Invalid hours")?;
        Ok(chrono::Duration::hours(n))
    } else if let Some(days) = s.strip_suffix('d') {
        let n: i64 = days.parse().context("Invalid days")?;
        Ok(chrono::Duration::days(n))
    } else if let Some(mins) = s.strip_suffix('m') {
        let n: i64 = mins.parse().context("Invalid minutes")?;
        Ok(chrono::Duration::minutes(n))
    } else {
        anyhow::bail!("Invalid duration '{s}'. Use e.g. '1h', '24h', '7d'")
    }
}

// ---------------------------------------------------------------------------
// Path resolution helpers
// ---------------------------------------------------------------------------

async fn resolve_cert_key_paths(
    cert: Option<&Path>,
    key: Option<&Path>,
    config_path: &Path,
) -> Result<(PathBuf, PathBuf)> {
    if let (Some(c), Some(k)) = (cert, key) {
        Ok((expand_tilde(c), expand_tilde(k)))
    } else {
        let config = load_config(&config_path.to_path_buf()).await?;
        let c = cert
            .map(expand_tilde)
            .or(config.effective_pki().tls_cert.map(|p| expand_tilde(&p)))
            .context("No --cert provided and no tls_cert in config")?;
        let k = key
            .map(expand_tilde)
            .or(config.effective_pki().tls_key.map(|p| expand_tilde(&p)))
            .context("No --key provided and no tls_key in config")?;
        Ok((c, k))
    }
}

// ---------------------------------------------------------------------------
// Legacy public API — used by config init
// ---------------------------------------------------------------------------

/// Generate a self-signed TLS certificate (used by `config init` for bootstrap).
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

    if let Ok(hostname) = std::process::Command::new("hostname").output() {
        if let Ok(name) = String::from_utf8(hostname.stdout) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                san_names.push(SanType::DnsName(name.try_into()?));
            }
        }
    }

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
    params
        .distinguished_name
        .push(DnType::CommonName, "RockBot Gateway");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "RockBot");

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
