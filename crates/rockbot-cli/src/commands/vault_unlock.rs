//! Vault unlock flow for gateway startup
//!
//! Handles interactive and non-interactive vault unlocking to retrieve
//! credentials needed for LLM providers and other services.

use anyhow::{anyhow, Result};
use rockbot_credentials::{CredentialManager, UnlockMethod};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Result of vault unlock attempt
pub struct VaultUnlockResult {
    /// The unlocked credential manager
    pub manager: Arc<CredentialManager>,
    /// Retrieved LLM API keys (provider_name -> api_key)
    pub llm_credentials: std::collections::HashMap<String, String>,
}

/// Known LLM provider endpoint names in the vault
const LLM_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("openai", "OPENAI_API_KEY"),
    ("google", "GOOGLE_API_KEY"),
    ("mistral", "MISTRAL_API_KEY"),
    ("cohere", "COHERE_API_KEY"),
];

/// Initialize and unlock the credential vault
///
/// This function:
/// 1. Opens the vault (creates if needed)
/// 2. Determines unlock method (password, keyfile, etc.)
/// 3. Prompts for password if needed (interactive only)
/// 4. Unlocks the vault
/// 5. Retrieves LLM API keys
///
/// # Arguments
///
/// * `vault_path` - Path to the vault directory
/// * `interactive` - Whether to prompt for password if needed
/// * `password_env` - Environment variable name for password (optional)
///
/// # Returns
///
/// `VaultUnlockResult` with the manager and retrieved credentials
pub async fn unlock_vault_for_gateway(
    vault_path: &Path,
    interactive: bool,
    password_env: Option<&str>,
) -> Result<VaultUnlockResult> {
    info!("Initializing credential vault at {:?}", vault_path);

    // Create vault directory if needed
    if !vault_path.exists() {
        tokio::fs::create_dir_all(vault_path).await?;
    }

    // Open or create the vault
    let manager = CredentialManager::new(vault_path)?;
    let manager = Arc::new(manager);

    // Check if vault is already unlocked (shouldn't be, but check anyway)
    if !manager.is_locked().await {
        info!("Vault already unlocked");
        let llm_credentials = retrieve_llm_credentials(&manager).await?;
        return Ok(VaultUnlockResult {
            manager,
            llm_credentials,
        });
    }

    // Determine unlock method
    let unlock_method = match manager.get_unlock_method().await {
        Some(method) => method,
        None => {
            // Vault not initialized
            return Err(anyhow!(
                "Vault not initialized. Run 'rockbot credentials init' first."
            ));
        }
    };
    info!("Vault unlock method: {:?}", unlock_method);

    match unlock_method {
        UnlockMethod::Keyfile { path_hint } => {
            // Auto-unlock with keyfile
            let keyfile_path = path_hint
                .as_ref()
                .map(|p| std::path::PathBuf::from(p))
                .unwrap_or_else(|| vault_path.join("keyfile"));

            if keyfile_path.exists() {
                info!("Unlocking vault with keyfile: {:?}", keyfile_path);
                manager.unlock_with_keyfile(&keyfile_path).await?;
            } else {
                return Err(anyhow!(
                    "Keyfile not found at {:?}. Create it or change unlock method.",
                    keyfile_path
                ));
            }
        }

        UnlockMethod::Password { .. } => {
            // Try environment variable first
            let password = if let Some(env_var) = password_env {
                std::env::var(env_var).ok()
            } else {
                // Default env var
                std::env::var("ROCKBOT_VAULT_PASSWORD").ok()
            };

            if let Some(password) = password {
                info!("Unlocking vault with password from environment");
                manager.unlock_with_password(&password).await?;
            } else if interactive {
                // Prompt for password
                let password = prompt_password("Enter vault password: ")?;
                manager.unlock_with_password(&password).await?;
            } else {
                return Err(anyhow!(
                    "Vault requires password but running non-interactively. \
                     Set ROCKBOT_VAULT_PASSWORD or use --interactive"
                ));
            }
        }

        UnlockMethod::SshKey { public_key_path, .. } => {
            // For SSH unlock, we need the private key (not the public key in the vault)
            // The public_key_path tells us which key pair to use
            let private_key_path = std::path::PathBuf::from(&public_key_path)
                .with_extension(""); // Remove .pub extension
            
            let key_path = if private_key_path.exists() {
                private_key_path
            } else {
                // Try default SSH key location
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(".ssh")
                    .join("id_ed25519")
            };

            // Check if key requires passphrase
            let passphrase = if interactive {
                // Try without passphrase first, prompt if needed
                match manager.unlock_with_ssh(&key_path, None).await {
                    Ok(_) => None,
                    Err(_) => {
                        let pass = prompt_password(&format!(
                            "Enter passphrase for {:?}: ",
                            key_path
                        ))?;
                        Some(pass)
                    }
                }
            } else {
                // Try without passphrase in non-interactive mode
                None
            };

            if passphrase.is_some() {
                manager
                    .unlock_with_ssh(&key_path, passphrase.as_deref())
                    .await?;
            }
        }

        UnlockMethod::Age { public_key, .. } => {
            // Age identity from environment or prompt
            let identity = std::env::var("AGE_IDENTITY").ok();

            if let Some(identity) = identity {
                manager.unlock_with_age(&identity).await?;
            } else if interactive {
                let identity = prompt_password("Enter age identity: ")?;
                manager.unlock_with_age(&identity).await?;
            } else {
                return Err(anyhow!(
                    "Vault requires age identity but running non-interactively. \
                     Set AGE_IDENTITY environment variable."
                ));
            }
        }
    }

    // Verify unlock succeeded
    if manager.is_locked().await {
        return Err(anyhow!("Failed to unlock vault"));
    }

    info!("Vault unlocked successfully");

    // Retrieve LLM credentials
    let llm_credentials = retrieve_llm_credentials(&manager).await?;

    Ok(VaultUnlockResult {
        manager,
        llm_credentials,
    })
}

/// Retrieve LLM API keys from the vault
async fn retrieve_llm_credentials(
    manager: &Arc<CredentialManager>,
) -> Result<std::collections::HashMap<String, String>> {
    let mut credentials = std::collections::HashMap::new();

    // Get all endpoints
    let endpoints = manager.list_endpoints().await;

    for (provider_name, env_var_name) in LLM_PROVIDERS {
        // Look for endpoint with matching name
        if let Some(endpoint) = endpoints.iter().find(|e| {
            e.name.to_lowercase() == *provider_name
                || e.name.to_lowercase().contains(provider_name)
        }) {
            // Try to decrypt the credential
            match manager.request_credential(
                &format!("saggyclaw://{}/api_key", provider_name),
                "gateway",
                "Gateway startup credential retrieval",
            ).await {
                Ok(result) => {
                    if let Some(secret) = result.credential {
                        if let Ok(api_key) = String::from_utf8(secret) {
                            debug!("Retrieved {} API key from vault", provider_name);
                            credentials.insert(provider_name.to_string(), api_key.clone());
                            
                            // Also set environment variable for providers that use it
                            std::env::set_var(env_var_name, &api_key);
                        }
                    }
                }
                Err(e) => {
                    debug!("Could not retrieve {} credential: {}", provider_name, e);
                }
            }
        }
    }

    // Also check environment for credentials not in vault
    for (provider_name, env_var_name) in LLM_PROVIDERS {
        if !credentials.contains_key(*provider_name) {
            if let Ok(api_key) = std::env::var(env_var_name) {
                debug!("{} API key found in environment", provider_name);
                credentials.insert(provider_name.to_string(), api_key);
            }
        }
    }

    if credentials.is_empty() {
        warn!(
            "No LLM API keys found in vault or environment. \
             Add credentials with 'rockbot credentials add anthropic' or set environment variables."
        );
    } else {
        info!(
            "Retrieved {} LLM credential(s): {:?}",
            credentials.len(),
            credentials.keys().collect::<Vec<_>>()
        );
    }

    Ok(credentials)
}

/// Prompt for password (hidden input)
fn prompt_password(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;

    // Use rpassword for hidden input
    let password = rpassword::read_password()?;
    Ok(password)
}

/// Prompt for password with confirmation
fn prompt_password_confirm(prompt: &str) -> Result<String> {
    loop {
        let password = prompt_password(prompt)?;
        let confirm = prompt_password("Confirm password: ")?;

        if password == confirm {
            return Ok(password);
        }

        println!("Passwords don't match. Try again.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_providers_list() {
        assert!(LLM_PROVIDERS.iter().any(|(name, _)| *name == "anthropic"));
        assert!(LLM_PROVIDERS.iter().any(|(name, _)| *name == "openai"));
    }
}
