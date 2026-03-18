//! Bridge between tools and credential vault
//!
//! This module provides the `VaultCredentialAccessor` which implements
//! `rockbot_tools::CredentialAccessor` and bridges tool credential
//! requests to the `rockbot_credentials::CredentialManager`.

use rockbot_credentials::CredentialManager;
use rockbot_tools::{CredentialAccessor, CredentialApplicationType, CredentialResult, Result};
use std::sync::Arc;
use tracing::{debug, warn};

/// Credential accessor that bridges to the vault
pub struct VaultCredentialAccessor {
    manager: Arc<CredentialManager>,
}

impl VaultCredentialAccessor {
    /// Create a new vault credential accessor
    pub fn new(manager: Arc<CredentialManager>) -> Self {
        Self { manager }
    }
}

#[async_trait::async_trait]
impl CredentialAccessor for VaultCredentialAccessor {
    async fn get_credential(&self, path: &str, agent_id: &str) -> Result<CredentialResult> {
        debug!(
            "Tool requesting credential for path: {} (agent: {})",
            path, agent_id
        );

        // Check if vault is unlocked
        if self.manager.is_locked().await {
            return Ok(CredentialResult::Denied {
                reason: "Credential vault is locked".to_string(),
            });
        }

        // Request credential - this handles permission checking and HIL internally
        match self
            .manager
            .request_credential(path, agent_id, &format!("Tool access to {path}"))
            .await
        {
            Ok(result) => {
                if let Some(credential) = result.credential {
                    // Credential was granted
                    let cred_type = self.infer_credential_type(path);
                    Ok(CredentialResult::Granted {
                        secret: credential,
                        credential_type: cred_type,
                    })
                } else if let Some(reason) = result.reason {
                    // Denied with reason
                    Ok(CredentialResult::Denied { reason })
                } else {
                    // No credential found
                    Ok(CredentialResult::NotFound {
                        path: path.to_string(),
                    })
                }
            }
            Err(e) => {
                warn!("Credential request failed: {}", e);
                Ok(CredentialResult::Denied {
                    reason: e.to_string(),
                })
            }
        }
    }

    async fn has_credential(&self, path: &str) -> Result<bool> {
        // Parse the path to extract endpoint info
        // Path format: saggyclaw://endpoint_name/...
        if let Some(endpoint_name) = path.strip_prefix("saggyclaw://") {
            let endpoint_name = endpoint_name.split('/').next().unwrap_or("");

            // Check if endpoint exists
            let endpoints = self.manager.list_endpoints().await;
            Ok(endpoints.iter().any(|e| e.name == endpoint_name))
        } else {
            Ok(false)
        }
    }
}

impl VaultCredentialAccessor {
    /// Infer credential application type from path patterns
    #[allow(clippy::unused_self)]
    fn infer_credential_type(&self, path: &str) -> CredentialApplicationType {
        // Default to bearer token
        // TODO: Look up the endpoint and get the actual credential type
        if path.contains("api_key") || path.contains("apikey") {
            CredentialApplicationType::ApiKey {
                header_name: "X-API-Key".to_string(),
            }
        } else if path.contains("basic") {
            CredentialApplicationType::BasicAuth {
                username: "user".to_string(), // Would need to get from credential
            }
        } else {
            CredentialApplicationType::BearerToken
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    #[test]
    #[ignore = "Requires proper CredentialManager setup"]
    fn test_infer_credential_type() {
        // This test requires a proper CredentialManager instance
        // which needs proper setup with vault files, etc.
        // Skipping for now until we have proper test infrastructure
    }
}
