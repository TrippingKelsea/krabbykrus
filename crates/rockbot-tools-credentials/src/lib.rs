//! Credential vault access tool for RockBot agents.
//!
//! Allows agents to securely access credentials stored in the vault.
//! No external credentials needed — this tool accesses the local vault.

use rockbot_credentials_schema::{CredentialCategory, CredentialSchema};
use rockbot_security::Capabilities;
use rockbot_tools::{Tool, ToolError, message::ToolResult, ToolExecutionContext};
use std::future::Future;
use std::pin::Pin;

/// Credential vault access tool
pub struct CredentialVaultTool;

impl CredentialVaultTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CredentialVaultTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for CredentialVaultTool {
    fn name(&self) -> &str {
        "credential_vault"
    }

    fn description(&self) -> &str {
        "Access credentials from the secure vault"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Credential path (e.g. 'saggyclaw://service/key')"
                }
            },
            "required": ["path"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::new()
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            let path = params.get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidParameters {
                    message: "Missing 'path' parameter".to_string(),
                })?;

            if let Some(accessor) = &context.credential_accessor {
                match accessor.get_credential(path, &context.agent_id).await {
                    Ok(result) => Ok(ToolResult::json(serde_json::json!({
                        "status": "ok",
                        "path": path,
                    }))),
                    Err(e) => Ok(ToolResult::error(format!("Credential access failed: {}", e))),
                }
            } else {
                Ok(ToolResult::error("No credential accessor available"))
            }
        })
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        // No external credentials needed — this accesses the local vault
        None
    }
}
