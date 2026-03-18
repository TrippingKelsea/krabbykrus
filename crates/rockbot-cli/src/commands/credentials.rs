//! Credential management CLI commands

use anyhow::Result;
use rockbot_config::Config;
use rockbot_credentials::{
    AuditLog, CredentialManager, CredentialType, CredentialVault, EndpointType, PathPermission,
    PermissionLevel,
};
use std::io::{self, Write};
use std::path::PathBuf;
use uuid::Uuid;

use crate::{load_config, CredentialsCommands, PermissionsCommands};

/// Run credentials commands
pub async fn run(command: &CredentialsCommands, config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path).await?;

    match command {
        CredentialsCommands::Init {
            force,
            password,
            keyfile,
            age,
            ssh_key,
        } => {
            init_vault(
                &config,
                *force,
                *password,
                keyfile.as_ref(),
                age.as_deref(),
                ssh_key.as_ref(),
            )
            .await
        }
        CredentialsCommands::Add {
            name,
            endpoint_type,
            url,
            secret,
            credential_type,
        } => {
            add_credential(
                &config,
                name,
                endpoint_type,
                url,
                secret.as_deref(),
                credential_type,
            )
            .await
        }
        CredentialsCommands::List => list_endpoints(&config).await,
        CredentialsCommands::Remove { endpoint } => remove_endpoint(&config, endpoint).await,
        CredentialsCommands::Permissions { command } => handle_permissions(&config, command).await,
        CredentialsCommands::Audit { verify, limit } => view_audit(&config, *verify, *limit).await,
        CredentialsCommands::Status => show_status(&config).await,
        CredentialsCommands::Unlock {
            password,
            keyfile,
            age,
            ssh_key,
            ssh_passphrase,
        } => {
            unlock_vault(
                &config,
                password.as_deref(),
                keyfile.as_ref(),
                age.as_deref(),
                ssh_key.as_ref(),
                ssh_passphrase.as_deref(),
            )
            .await
        }
        CredentialsCommands::Lock => lock_vault(&config).await,
        CredentialsCommands::Ui => {
            anyhow::bail!(
                "The standalone credentials UI was removed. Use `rockbot tui` and open the Credentials view."
            )
        }
    }
}

/// Initialize the credential vault
async fn init_vault(
    config: &Config,
    force: bool,
    use_password: bool,
    keyfile: Option<&PathBuf>,
    age_pubkey: Option<&str>,
    ssh_pubkey: Option<&PathBuf>,
) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let vault_path = &config.credentials.vault_path;

    // Check if vault already exists
    if CredentialVault::exists(vault_path) {
        if !force {
            anyhow::bail!(
                "Vault already exists at {}. Use --force to re-initialize (WARNING: destroys existing credentials).",
                vault_path.display()
            );
        }

        // Confirm destruction
        print!("⚠️  This will DESTROY all existing credentials. Type 'yes' to confirm: ");
        io::stdout().flush()?;
        let mut confirm = String::new();
        io::stdin().read_line(&mut confirm)?;
        if confirm.trim() != "yes" {
            println!("Aborted.");
            return Ok(());
        }

        // Remove existing vault
        std::fs::remove_dir_all(vault_path)?;
    }

    println!(
        "🔐 Initializing credential vault at {}",
        vault_path.display()
    );
    println!();

    // Determine unlock method - priority: Age > SSH > password > keyfile (default)
    if let Some(pubkey) = age_pubkey {
        // Age encryption
        println!("🔐 Using Age encryption");
        let _vault = CredentialVault::init_with_age(vault_path, pubkey)?;
        println!();
        println!("✅ Vault initialized with Age encryption!");
        println!("   Unlock with: rockbot credentials unlock --age <identity>");
    } else if let Some(pubkey_path) = ssh_pubkey {
        // SSH key encryption
        if !pubkey_path.exists() {
            anyhow::bail!("SSH public key not found: {}", pubkey_path.display());
        }
        println!("🔐 Using SSH key: {}", pubkey_path.display());
        let _vault = CredentialVault::init_with_ssh(vault_path, pubkey_path)?;
        println!();
        println!("✅ Vault initialized with SSH key!");
        println!("   Unlock with: rockbot credentials unlock --ssh-key <private_key_path>");
    } else if use_password {
        // Password-based encryption
        let password = prompt_password_hidden("Enter vault password: ")?;
        let confirm = prompt_password_hidden("Confirm password: ")?;

        if password != confirm {
            anyhow::bail!("Passwords do not match");
        }

        if password.len() < 8 {
            anyhow::bail!("Password must be at least 8 characters");
        }

        let _vault = CredentialVault::init_with_password(vault_path, &password)?;
        println!();
        println!("✅ Vault initialized with password!");
    } else {
        // Keyfile (default) - generate or use specified
        let kf_path = keyfile.cloned().unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("rockbot")
                .join("vault.key")
        });

        // Create parent directory if needed
        if let Some(parent) = kf_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if kf_path.exists() {
            // Use existing keyfile
            println!("🔑 Using existing key file: {}", kf_path.display());
        } else {
            // Generate new keyfile
            println!("🔑 Generating new key file: {}", kf_path.display());

            let mut key_bytes = [0u8; 32];
            getrandom::getrandom(&mut key_bytes)?;

            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&kf_path)?;

            use std::io::Write;
            file.write_all(&key_bytes)?;

            println!("   ⚠️  Store this file securely! Without it, you cannot access your vault.");
        }

        let _vault = CredentialVault::init_with_keyfile(vault_path, &kf_path)?;
        println!();
        println!("✅ Vault initialized with key file!");
    }

    println!();
    println!("Next steps:");
    println!("  • Add credentials: rockbot credentials add <name> -t <type> -u <url>");
    println!("  • View status:     rockbot credentials status");

    Ok(())
}

/// Add a new credential endpoint
async fn add_credential(
    config: &Config,
    name: &str,
    endpoint_type: &str,
    url: &str,
    secret: Option<&str>,
    credential_type: &str,
) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    // Check if vault exists
    if !CredentialVault::exists(&config.credentials.vault_path) {
        anyhow::bail!("Vault not initialized. Run 'rockbot credentials init' first.");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;

    // Prompt for password to unlock
    let password = prompt_password_hidden("Enter vault password: ")?;
    manager.unlock_with_password(&password).await?;

    // Parse endpoint type
    let ep_type = match endpoint_type {
        "home_assistant" => EndpointType::HomeAssistant,
        "gmail" => EndpointType::Gmail,
        "spotify" => EndpointType::Spotify,
        "generic_rest" => EndpointType::GenericRest,
        "generic_oauth2" => EndpointType::GenericOAuth2,
        _ => anyhow::bail!("Unknown endpoint type: {endpoint_type}"),
    };

    // Create endpoint
    let endpoint = manager
        .create_endpoint(name.to_string(), ep_type, url.to_string())
        .await?;
    println!("✅ Created endpoint: {} ({})", endpoint.name, endpoint.id);

    // Store credential if provided
    if let Some(secret_value) = secret {
        let cred_type = match credential_type {
            "bearer_token" => CredentialType::BearerToken,
            "basic_auth" => CredentialType::BasicAuth {
                username: String::new(),
            },
            "api_key" => CredentialType::ApiKey {
                header_name: "X-API-Key".to_string(),
            },
            _ => anyhow::bail!("Unknown credential type: {credential_type}"),
        };

        manager
            .store_credential(endpoint.id, cred_type, secret_value.as_bytes())
            .await?;
        println!("✅ Stored credential for endpoint");
    } else {
        println!(
            "ℹ️  No secret provided. Use 'rockbot credentials add' with --secret to add later."
        );
    }

    Ok(())
}

/// List all configured endpoints
async fn list_endpoints(config: &Config) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;
    let endpoints = manager.list_endpoints().await;

    if endpoints.is_empty() {
        println!("No endpoints configured.");
        return Ok(());
    }

    println!("{:<36} {:<20} {:<15} {:<30}", "ID", "NAME", "TYPE", "URL");
    println!("{}", "-".repeat(100));

    for endpoint in endpoints {
        println!(
            "{:<36} {:<20} {:<15} {:<30}",
            endpoint.id,
            truncate(&endpoint.name, 18),
            endpoint.endpoint_type.as_str(),
            truncate(&endpoint.base_url, 28),
        );
    }

    Ok(())
}

/// Remove an endpoint
async fn remove_endpoint(config: &Config, endpoint: &str) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;
    let endpoint_id = Uuid::parse_str(endpoint)
        .map_err(|_| anyhow::anyhow!("Endpoint must be a UUID, got '{endpoint}'"))?;
    manager.delete_endpoint(endpoint_id).await?;
    println!("✅ Removed endpoint: {endpoint_id}");

    Ok(())
}

/// Handle permission subcommands
async fn handle_permissions(config: &Config, command: &PermissionsCommands) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;

    match command {
        PermissionsCommands::Add {
            pattern,
            level,
            description,
        } => {
            let perm_level = match level.as_str() {
                "allow" => PermissionLevel::Allow,
                "allow_hil" => PermissionLevel::AllowHil,
                "allow_hil_2fa" => PermissionLevel::AllowHil2fa,
                "deny" => PermissionLevel::Deny,
                _ => anyhow::bail!("Unknown permission level: {level}"),
            };

            let permission = PathPermission {
                id: Uuid::new_v4(),
                path_pattern: pattern.clone(),
                level: perm_level,
                description: description.clone(),
            };

            manager.add_permission(permission.clone()).await;
            println!("✅ Added permission rule: {pattern} -> {level}");
            println!("   Rule ID: {}", permission.id);
        }
        PermissionsCommands::List => {
            let permissions = manager.list_path_permissions().await;
            if permissions.is_empty() {
                println!("No permission rules configured.");
                return Ok(());
            }

            println!(
                "{:<36} {:<20} {:<14} DESCRIPTION",
                "RULE ID", "PATTERN", "LEVEL"
            );
            println!("{}", "-".repeat(96));
            for permission in permissions {
                println!(
                    "{:<36} {:<20} {:<14} {}",
                    permission.id,
                    truncate(&permission.path_pattern, 18),
                    format!("{:?}", permission.level),
                    permission.description.unwrap_or_default(),
                );
            }
        }
        PermissionsCommands::Remove { rule_id } => {
            let permission_id = Uuid::parse_str(rule_id)
                .map_err(|_| anyhow::anyhow!("Permission rule id must be a UUID, got '{rule_id}'"))?;
            if manager.remove_permission(permission_id).await {
                println!("✅ Removed permission rule: {permission_id}");
            } else {
                println!("⚠ Permission rule not found: {permission_id}");
            }
        }
    }

    Ok(())
}

/// View or verify audit log
async fn view_audit(config: &Config, verify: bool, _limit: usize) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let audit_path = config.credentials.vault_path.join("audit.log");

    if verify {
        println!("Verifying audit log integrity...");
        let log = AuditLog::open(&audit_path)?;
        let result = log.verify()?;

        if result.valid {
            println!("✅ Audit log is valid");
            println!("   Entries verified: {}", result.entries_checked);
            println!("   Last sequence: {}", result.last_sequence);
        } else {
            println!("❌ Audit log verification FAILED");
            if let Some(error) = result.error {
                println!("   Error: {error}");
            }
        }
    } else {
        // Just show recent entries count
        let log = AuditLog::open(&audit_path)?;
        println!("Audit log: {}", audit_path.display());
        println!("Next sequence: {}", log.next_sequence());
        println!("\nTo verify integrity: rockbot credentials audit --verify");
        println!("\nView raw log: cat {}", audit_path.display());
    }

    Ok(())
}

/// Show vault status
async fn show_status(config: &Config) -> Result<()> {
    println!("Credentials Configuration:");
    println!("  Enabled: {}", config.credentials.enabled);
    println!("  Vault path: {}", config.credentials.vault_path.display());
    println!("  Unlock method: {}", config.credentials.unlock_method);
    println!(
        "  Default permission: {}",
        config.credentials.default_permission
    );

    if config.credentials.enabled {
        let manager = CredentialManager::new(&config.credentials.vault_path)?;
        let locked = manager.is_locked().await;
        let endpoints = manager.list_endpoints().await;

        println!("\nVault Status:");
        println!("  Locked: {locked}");
        println!("  Endpoints: {}", endpoints.len());
    }

    Ok(())
}

/// Unlock the vault
async fn unlock_vault(
    config: &Config,
    password: Option<&str>,
    keyfile: Option<&PathBuf>,
    age_identity: Option<&str>,
    ssh_privkey: Option<&PathBuf>,
    ssh_passphrase: Option<&str>,
) -> Result<()> {
    use rockbot_credentials::UnlockMethod;

    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    // Check if vault exists
    if !CredentialVault::exists(&config.credentials.vault_path) {
        anyhow::bail!("Vault not initialized. Run 'rockbot credentials init' first.");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;

    // Get the vault's unlock method
    let vault = CredentialVault::open(&config.credentials.vault_path)?;
    let unlock_method = vault
        .unlock_method()
        .ok_or_else(|| anyhow::anyhow!("Failed to read vault unlock method"))?;

    match unlock_method {
        UnlockMethod::Password { .. } => {
            let password = match password {
                Some(p) => p.to_string(),
                None => prompt_password_hidden("Enter vault password: ")?,
            };
            manager.unlock_with_password(&password).await?;
        }
        UnlockMethod::Keyfile { path_hint } => {
            let kf_path = if let Some(p) = keyfile {
                p.clone()
            } else {
                // Try default path first
                let default_path = dirs::config_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                    .join("rockbot")
                    .join("vault.key");

                if default_path.exists() {
                    default_path
                } else if let Some(hint) = path_hint {
                    let hint_path = PathBuf::from(hint);
                    if hint_path.exists() {
                        println!("Using key file: {hint}");
                        hint_path
                    } else {
                        anyhow::bail!(
                            "Key file not found. Use --keyfile <path>\n\
                             (Previously used: {hint})"
                        );
                    }
                } else {
                    anyhow::bail!("Key file not found. Use --keyfile <path>");
                }
            };
            manager.unlock_with_keyfile(&kf_path).await?;
        }
        UnlockMethod::Age { public_key, .. } => {
            let identity = match age_identity {
                Some(id) => id.to_string(),
                None => {
                    anyhow::bail!(
                        "This vault uses Age encryption.\n\
                         Unlock with: --age AGE-SECRET-KEY-...\n\
                         Public key: {}",
                        &public_key[..40.min(public_key.len())]
                    );
                }
            };
            manager.unlock_with_age(&identity).await?;
        }
        UnlockMethod::SshKey {
            public_key_path, ..
        } => {
            let privkey_path = if let Some(p) = ssh_privkey {
                p.clone()
            } else {
                // Try to find corresponding private key
                let pubkey = PathBuf::from(public_key_path);
                let privkey = pubkey.with_extension(""); // Remove .pub extension
                if privkey.exists() {
                    privkey
                } else {
                    anyhow::bail!(
                        "This vault uses SSH key encryption.\n\
                         Unlock with: --ssh-key <private_key_path>\n\
                         Public key used: {public_key_path}"
                    );
                }
            };
            manager
                .unlock_with_ssh(&privkey_path, ssh_passphrase)
                .await?;
        }
    }

    println!("✅ Vault unlocked");
    println!("Note: Vault will lock when this process exits.");

    Ok(())
}

/// Lock the vault
async fn lock_vault(config: &Config) -> Result<()> {
    if !config.credentials.enabled {
        anyhow::bail!("Credential management is not enabled in configuration");
    }

    let manager = CredentialManager::new(&config.credentials.vault_path)?;
    manager.lock().await?;

    println!("🔒 Vault locked");

    Ok(())
}

/// Prompt for password with hidden input
fn prompt_password_hidden(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;

    let password = rpassword::read_password()?;
    Ok(password)
}

/// Truncate a string to a maximum length
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}
