//! MCP (Model Context Protocol) server connection tool for RockBot.
//!
//! Allows agents to connect to external MCP servers and use their tools.

use rockbot_credentials_schema::{
    AuthMethod, CredentialCategory, CredentialField, CredentialSchema,
};
use rockbot_security::Capabilities;
use rockbot_tools::{Tool, ToolError, message::ToolResult, ToolExecutionContext};
use std::future::Future;
use std::pin::Pin;

/// MCP server connection tool
pub struct McpTool;

impl McpTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for McpTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "Connect to an MCP server and invoke tools"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server_url": {
                    "type": "string",
                    "description": "URL of the MCP server"
                },
                "tool_name": {
                    "type": "string",
                    "description": "Name of the tool to invoke"
                },
                "arguments": {
                    "type": "object",
                    "description": "Arguments to pass to the tool"
                }
            },
            "required": ["server_url", "tool_name"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::new()
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            let server_url = params.get("server_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidParameters {
                    message: "Missing 'server_url' parameter".to_string(),
                })?;

            Ok(ToolResult::error(format!(
                "MCP tool not yet implemented (server: {})",
                server_url
            )))
        })
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "mcp".to_string(),
            provider_name: "MCP Server".to_string(),
            category: CredentialCategory::Tool,
            auth_methods: vec![AuthMethod {
                id: "server_auth".to_string(),
                label: "Server Authentication".to_string(),
                fields: vec![
                    CredentialField {
                        id: "server_url".to_string(),
                        label: "Server URL".to_string(),
                        secret: false,
                        default: None,
                        placeholder: Some("http://localhost:3000".to_string()),
                        required: true,
                        env_var: Some("MCP_SERVER_URL".to_string()),
                    },
                    CredentialField {
                        id: "auth_token".to_string(),
                        label: "Auth Token".to_string(),
                        secret: true,
                        default: None,
                        placeholder: None,
                        required: false,
                        env_var: Some("MCP_AUTH_TOKEN".to_string()),
                    },
                ],
                hint: None,
                docs_url: None,
            }],
        })
    }
}
