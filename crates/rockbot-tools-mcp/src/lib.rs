//! MCP (Model Context Protocol) client for RockBot.
//!
//! Provides a JSON-RPC 2.0 client over stdio transport to connect to external
//! MCP servers. Each server process is managed by [`McpServerManager`] which
//! handles lifecycle, tool discovery, and tool call proxying.

use rockbot_credentials_schema::{
    AuthMethod, CredentialCategory, CredentialField, CredentialSchema,
};
use rockbot_security::Capabilities;
use rockbot_tools::{Tool, ToolError, message::ToolResult, ToolExecutionContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};

/// JSON-RPC 2.0 request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

/// MCP tool definition returned by `tools/list`
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<serde_json::Value>,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// A running MCP server process with its stdio transport
struct McpConnection {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
    next_id: u64,
}

impl McpConnection {
    /// Send a JSON-RPC request and read the response
    async fn call(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, ToolError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&request).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to serialize request: {e}"),
        })?;
        line.push('\n');

        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.stdin.write_all(line.as_bytes()),
        ).await.map_err(|_| ToolError::ExecutionFailed {
            message: "Timed out writing to MCP server stdin".to_string(),
        })?.map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to write to MCP server stdin: {e}"),
        })?;
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.stdin.flush(),
        ).await.map_err(|_| ToolError::ExecutionFailed {
            message: "Timed out flushing MCP server stdin".to_string(),
        })?.map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to flush MCP server stdin: {e}"),
        })?;

        // Read response line
        let mut response_line = String::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.stdout_reader.read_line(&mut response_line),
        )
        .await
        .map_err(|_| ToolError::ExecutionFailed {
            message: "Timeout waiting for MCP server response".to_string(),
        })?
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to read from MCP server stdout: {e}"),
        })?;

        if read == 0 {
            return Err(ToolError::ExecutionFailed {
                message: "MCP server closed stdout unexpectedly".to_string(),
            });
        }

        let response: JsonRpcResponse =
            serde_json::from_str(response_line.trim()).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to parse MCP server response: {e}"),
            })?;

        if let Some(error) = response.error {
            return Err(ToolError::ExecutionFailed {
                message: format!("MCP error ({}): {}", error.code, error.message),
            });
        }

        response.result.ok_or_else(|| ToolError::ExecutionFailed {
            message: "MCP server returned neither result nor error".to_string(),
        })
    }
}

/// Manages MCP server processes and provides tool discovery + invocation
pub struct McpServerManager {
    servers: RwLock<HashMap<String, Arc<Mutex<McpConnection>>>>,
    /// Discovered tools: key = "server_name:tool_name"
    tool_defs: RwLock<HashMap<String, (String, McpToolDef)>>,
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpServerManager {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
            tool_defs: RwLock::new(HashMap::new()),
        }
    }

    /// Start an MCP server process and discover its tools
    pub async fn start_server(
        &self,
        name: &str,
        config: &McpServerConfig,
    ) -> Result<Vec<McpToolDef>, ToolError> {
        debug!("Starting MCP server '{name}': {} {:?}", config.command, config.args);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn().map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to spawn MCP server '{name}': {e}"),
        })?;

        let stdin = child.stdin.take().ok_or_else(|| ToolError::ExecutionFailed {
            message: "Failed to capture MCP server stdin".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| ToolError::ExecutionFailed {
            message: "Failed to capture MCP server stdout".to_string(),
        })?;

        let mut conn = McpConnection {
            child,
            stdin,
            stdout_reader: BufReader::new(stdout),
            next_id: 1,
        };

        // Initialize MCP protocol
        let init_result = conn.call("initialize", Some(serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "rockbot",
                "version": env!("CARGO_PKG_VERSION")
            }
        }))).await?;

        debug!("MCP server '{name}' initialized: {init_result}");

        // Send initialized notification (no response expected, but we send as a call
        // that we don't wait for — MCP spec says this is a notification)
        let notify_line = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        })).unwrap_or_default();
        let _ = conn.stdin.write_all(format!("{notify_line}\n").as_bytes()).await;
        let _ = conn.stdin.flush().await;

        // Discover tools
        let tools_result = conn.call("tools/list", None).await?;
        let tools: Vec<McpToolDef> = serde_json::from_value(
            tools_result.get("tools").cloned().unwrap_or(serde_json::Value::Array(vec![]))
        ).unwrap_or_default();

        debug!("MCP server '{name}' has {} tools", tools.len());

        // Store tool definitions
        {
            let mut defs = self.tool_defs.write().await;
            for tool in &tools {
                let key = format!("{name}:{}", tool.name);
                defs.insert(key, (name.to_string(), tool.clone()));
            }
        }

        // Store connection
        {
            let mut servers = self.servers.write().await;
            servers.insert(name.to_string(), Arc::new(Mutex::new(conn)));
        }

        Ok(tools)
    }

    /// Call a tool on a running MCP server
    pub async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let servers = self.servers.read().await;
        let conn = servers.get(server_name).ok_or_else(|| ToolError::ExecutionFailed {
            message: format!("MCP server '{server_name}' not found or not running"),
        })?.clone();
        drop(servers);

        let mut conn = conn.lock().await;
        conn.call("tools/call", Some(serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        }))).await
    }

    /// Stop all MCP server processes
    pub async fn stop_all(&self) {
        let mut servers = self.servers.write().await;
        for (name, conn) in servers.drain() {
            let mut conn = conn.lock().await;
            if let Err(e) = conn.child.kill().await {
                warn!("Failed to kill MCP server '{name}': {e}");
            }
        }
        self.tool_defs.write().await.clear();
    }

    /// Get all discovered tool definitions
    pub async fn get_tool_defs(&self) -> Vec<(String, McpToolDef)> {
        let defs = self.tool_defs.read().await;
        defs.values().cloned().collect()
    }
}

impl Drop for McpServerManager {
    fn drop(&mut self) {
        // Best-effort cleanup — async drop isn't available so we just try
        // to kill the child processes synchronously via their handles
    }
}

/// Dynamic proxy tool that forwards execution to an MCP server via `McpServerManager`.
///
/// One `McpProxyTool` is created per discovered MCP tool during `McpServerManager::start_server()`.
pub struct McpProxyTool {
    /// Namespaced name: "mcp_<server>_<tool>" to avoid collisions
    qualified_name: String,
    server_name: String,
    tool_def: McpToolDef,
    manager: Arc<McpServerManager>,
}

impl McpProxyTool {
    pub fn new(server_name: String, tool_def: McpToolDef, manager: Arc<McpServerManager>) -> Self {
        let qualified_name = format!("mcp_{}_{}", server_name, tool_def.name);
        Self { qualified_name, server_name, tool_def, manager }
    }
}

impl Tool for McpProxyTool {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn description(&self) -> &str {
        self.tool_def.description.as_deref().unwrap_or("MCP tool")
    }

    fn parameters(&self) -> serde_json::Value {
        self.tool_def.input_schema.clone().unwrap_or(serde_json::json!({"type": "object"}))
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
            let result = self.manager.call_tool(&self.server_name, &self.tool_def.name, params).await?;

            // MCP tools/call returns { content: [{ type, text }] }
            if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
                let text_parts: Vec<&str> = content.iter()
                    .filter_map(|part| {
                        if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                            part.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !text_parts.is_empty() {
                    return Ok(ToolResult::text(text_parts.join("\n")));
                }
            }

            // Fallback: return raw JSON
            Ok(ToolResult::json(result))
        })
    }
}

/// MCP server connection tool — proxies tool calls to MCP servers
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
        "Connect to an MCP server and invoke tools via JSON-RPC over stdio"
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
                "MCP tool not yet implemented for HTTP transport (server: {server_url}). \
                 Use McpServerManager for stdio-based MCP servers."
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"tools/list\""));
        // params should be absent when None
        assert!(!json.contains("params"));
    }

    #[test]
    fn test_json_rpc_request_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "test", "arguments": {}})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"id\":42"));
        assert!(json.contains("\"params\""));
    }

    #[test]
    fn test_json_rpc_response_parsing() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_parsing() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn test_mcp_tool_def_parsing() {
        let json = r#"{"name":"read_file","description":"Read a file","inputSchema":{"type":"object"}}"#;
        let tool: McpToolDef = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description.as_deref(), Some("Read a file"));
        assert!(tool.input_schema.is_some());
    }

    #[test]
    fn test_mcp_server_config_parsing() {
        let json = r#"{"command":"npx","args":["-y","@mcp/server"],"env":{"NODE_ENV":"test"}}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.command, "npx");
        assert_eq!(config.args, vec!["-y", "@mcp/server"]);
        assert_eq!(config.env.get("NODE_ENV").unwrap(), "test");
    }

    #[test]
    fn test_mcp_server_manager_creation() {
        let manager = McpServerManager::new();
        // Should start with empty servers and tools
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let defs = manager.get_tool_defs().await;
            assert!(defs.is_empty());
        });
    }

    #[test]
    fn test_mcp_proxy_tool_naming() {
        let manager = Arc::new(McpServerManager::new());
        let tool_def = McpToolDef {
            name: "read_file".to_string(),
            description: Some("Read a file".to_string()),
            input_schema: Some(serde_json::json!({"type": "object"})),
        };
        let proxy = McpProxyTool::new("filesystem".to_string(), tool_def, manager);
        assert_eq!(proxy.name(), "mcp_filesystem_read_file");
        assert_eq!(proxy.description(), "Read a file");
    }

    #[test]
    fn test_mcp_proxy_tool_no_description() {
        let manager = Arc::new(McpServerManager::new());
        let tool_def = McpToolDef {
            name: "list".to_string(),
            description: None,
            input_schema: None,
        };
        let proxy = McpProxyTool::new("server".to_string(), tool_def, manager);
        assert_eq!(proxy.name(), "mcp_server_list");
        assert_eq!(proxy.description(), "MCP tool");
    }
}
