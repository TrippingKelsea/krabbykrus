//! Agent Client Protocol (ACP) implementation.
//!
//! ACP enables IDE integration by exposing agent capabilities over a
//! JSON-RPC 2.0 protocol transported over stdio (stdin/stdout).
//! This is compatible with editors like JetBrains, Zed, and VS Code.
//!
//! # Protocol
//!
//! The IDE spawns the RockBot process with `--acp` flag. Communication
//! happens via newline-delimited JSON-RPC messages over stdin/stdout.
//!
//! ## Methods
//!
//! - `initialize` — Handshake with capabilities exchange
//! - `agent/message` — Send a message to an agent and get a response
//! - `agent/list` — List available agents
//! - `agent/capabilities` — Get agent capabilities (tools, skills)
//! - `shutdown` — Graceful shutdown
//!
//! ## Notifications (server -> client)
//!
//! - `agent/progress` — Streaming progress updates
//! - `agent/toolCall` — Tool call notifications

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// ACP protocol version.
pub const ACP_VERSION: &str = "0.1.0";

/// JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification (no id, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

// Standard JSON-RPC error codes
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// Server capabilities advertised during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Protocol version.
    pub protocol_version: String,
    /// Server name.
    pub server_name: String,
    /// Server version.
    pub server_version: String,
    /// Supported capabilities.
    pub capabilities: CapabilitySet,
}

/// Set of supported capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    /// Whether streaming responses are supported.
    pub streaming: bool,
    /// Whether tool calling is supported.
    pub tool_use: bool,
    /// Whether vision/image input is supported.
    pub vision: bool,
    /// Available agent IDs.
    pub agents: Vec<String>,
}

/// Initialize request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    /// Client name.
    #[serde(default)]
    pub client_name: String,
    /// Client version.
    #[serde(default)]
    pub client_version: String,
    /// Workspace root path.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

/// Agent message request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageParams {
    /// Target agent ID.
    pub agent_id: String,
    /// Message text.
    pub message: String,
    /// Optional session ID (creates new session if not provided).
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Agent message response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageResult {
    /// Agent's response text.
    pub response: String,
    /// Session ID used.
    pub session_id: String,
    /// Tools called during processing.
    pub tool_calls: Vec<ToolCallInfo>,
}

/// Info about a tool call made during processing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub tool_name: String,
    pub success: bool,
}

/// Agent list entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub model: Option<String>,
    pub description: Option<String>,
}

/// ACP dispatcher that processes JSON-RPC messages.
pub struct AcpDispatcher {
    /// Available agent IDs.
    agent_ids: Vec<String>,
    /// Server name.
    server_name: String,
    /// Initialized flag.
    initialized: bool,
}

impl AcpDispatcher {
    /// Create a new ACP dispatcher.
    pub fn new(agent_ids: Vec<String>) -> Self {
        Self {
            agent_ids,
            server_name: "rockbot".to_string(),
            initialized: false,
        }
    }

    /// Process a JSON-RPC request and return a response.
    pub async fn dispatch(&mut self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(&request.params),
            "agent/list" => self.handle_agent_list(),
            "agent/capabilities" => self.handle_capabilities(&request.params),
            "shutdown" => self.handle_shutdown(),
            "agent/message" => {
                // This would need an agent invoker to actually process - return placeholder
                self.handle_agent_message_placeholder(&request.params)
            }
            _ => Err(JsonRpcError {
                code: METHOD_NOT_FOUND,
                message: format!("Method not found: {}", request.method),
                data: None,
            }),
        };

        match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: Some(value),
                error: None,
            },
            Err(error) => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: request.id.clone(),
                result: None,
                error: Some(error),
            },
        }
    }

    fn handle_initialize(
        &mut self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, JsonRpcError> {
        let init_params: InitializeParams =
            serde_json::from_value(params.clone()).unwrap_or(InitializeParams {
                client_name: String::new(),
                client_version: String::new(),
                workspace_root: None,
            });

        info!(
            "ACP initialize: client={} v{}",
            init_params.client_name, init_params.client_version
        );

        self.initialized = true;

        let capabilities = ServerCapabilities {
            protocol_version: ACP_VERSION.to_string(),
            server_name: self.server_name.clone(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: CapabilitySet {
                streaming: true,
                tool_use: true,
                vision: true,
                agents: self.agent_ids.clone(),
            },
        };

        serde_json::to_value(capabilities).map_err(|e| JsonRpcError {
            code: INTERNAL_ERROR,
            message: e.to_string(),
            data: None,
        })
    }

    fn handle_agent_list(&self) -> Result<serde_json::Value, JsonRpcError> {
        let agents: Vec<AgentInfo> = self
            .agent_ids
            .iter()
            .map(|id| AgentInfo {
                id: id.clone(),
                model: None,
                description: None,
            })
            .collect();

        serde_json::to_value(agents).map_err(|e| JsonRpcError {
            code: INTERNAL_ERROR,
            message: e.to_string(),
            data: None,
        })
    }

    fn handle_capabilities(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, JsonRpcError> {
        let agent_id = params
            .get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !self.agent_ids.contains(&agent_id.to_string()) && !agent_id.is_empty() {
            return Err(JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Unknown agent: {agent_id}"),
                data: None,
            });
        }

        let caps = serde_json::json!({
            "agent_id": agent_id,
            "tools": true,
            "streaming": true,
            "vision": true,
        });

        Ok(caps)
    }

    fn handle_shutdown(&mut self) -> Result<serde_json::Value, JsonRpcError> {
        info!("ACP shutdown requested");
        self.initialized = false;
        Ok(serde_json::Value::Null)
    }

    fn handle_agent_message_placeholder(
        &self,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, JsonRpcError> {
        if !self.initialized {
            return Err(JsonRpcError {
                code: INVALID_REQUEST,
                message: "Server not initialized. Send 'initialize' first.".to_string(),
                data: None,
            });
        }

        let msg_params: AgentMessageParams =
            serde_json::from_value(params.clone()).map_err(|e| JsonRpcError {
                code: INVALID_PARAMS,
                message: e.to_string(),
                data: None,
            })?;

        debug!(
            "ACP agent/message: agent={}, msg={}",
            msg_params.agent_id, msg_params.message
        );

        if !self.agent_ids.contains(&msg_params.agent_id) {
            return Err(JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Unknown agent: {}", msg_params.agent_id),
                data: None,
            });
        }

        // Placeholder response — actual agent invocation requires gateway integration
        let result = AgentMessageResult {
            response: format!("ACP message received by agent '{}'. Full agent invocation requires gateway connection.", msg_params.agent_id),
            session_id: msg_params.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            tool_calls: vec![],
        };

        serde_json::to_value(result).map_err(|e| JsonRpcError {
            code: INTERNAL_ERROR,
            message: e.to_string(),
            data: None,
        })
    }

    /// Parse a JSON-RPC message from a line of text.
    pub fn parse_request(line: &str) -> Result<JsonRpcRequest, JsonRpcError> {
        serde_json::from_str(line).map_err(|e| JsonRpcError {
            code: PARSE_ERROR,
            message: format!("Invalid JSON: {e}"),
            data: None,
        })
    }

    /// Serialize a response to a JSON string (newline-terminated).
    pub fn serialize_response(response: &JsonRpcResponse) -> String {
        let mut json = serde_json::to_string(response).unwrap_or_else(|_| {
            r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Serialization error"}}"#
                .to_string()
        });
        json.push('\n');
        json
    }

    /// Serialize a notification to a JSON string (newline-terminated).
    pub fn serialize_notification(notification: &JsonRpcNotification) -> String {
        let mut json = serde_json::to_string(notification).unwrap_or_else(|_| String::new());
        json.push('\n');
        json
    }

    /// Create a progress notification.
    pub fn progress_notification(agent_id: &str, text: &str) -> JsonRpcNotification {
        JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "agent/progress".to_string(),
            params: serde_json::json!({
                "agent_id": agent_id,
                "text": text,
            }),
        }
    }

    /// Create a tool call notification.
    pub fn tool_call_notification(
        agent_id: &str,
        tool_name: &str,
        status: &str,
    ) -> JsonRpcNotification {
        JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "agent/toolCall".to_string(),
            params: serde_json::json!({
                "agent_id": agent_id,
                "tool_name": tool_name,
                "status": status,
            }),
        }
    }
}

/// Run the ACP stdio server loop.
///
/// Reads JSON-RPC requests from stdin, dispatches them, and writes
/// responses to stdout. This is the entry point when `--acp` is passed.
pub async fn run_acp_server(agent_ids: Vec<String>) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    info!("Starting ACP server (protocol v{})", ACP_VERSION);

    let mut dispatcher = AcpDispatcher::new(agent_ids);
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                debug!("ACP: stdin closed, shutting down");
                break;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match AcpDispatcher::parse_request(trimmed) {
                    Ok(request) => {
                        let response = dispatcher.dispatch(&request).await;
                        let output = AcpDispatcher::serialize_response(&response);
                        if let Err(e) = stdout.write_all(output.as_bytes()).await {
                            warn!("ACP: failed to write response: {e}");
                            break;
                        }
                        if let Err(e) = stdout.flush().await {
                            warn!("ACP: failed to flush stdout: {e}");
                            break;
                        }

                        // Exit after shutdown
                        if request.method == "shutdown" {
                            break;
                        }
                    }
                    Err(error) => {
                        let response = JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id: None,
                            result: None,
                            error: Some(error),
                        };
                        let output = AcpDispatcher::serialize_response(&response);
                        let _ = stdout.write_all(output.as_bytes()).await;
                        let _ = stdout.flush().await;
                    }
                }
            }
            Err(e) => {
                warn!("ACP: stdin read error: {e}");
                break;
            }
        }
    }

    info!("ACP server shut down");
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_parse_valid_request() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"client_name":"test"}}"#;
        let req = AcpDispatcher::parse_request(json).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = AcpDispatcher::parse_request("not json");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, PARSE_ERROR);
    }

    #[tokio::test]
    async fn test_initialize() {
        let mut dispatcher = AcpDispatcher::new(vec!["agent-1".to_string()]);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({"client_name": "test-ide", "client_version": "1.0"}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocol_version"], ACP_VERSION);
        assert!(result["capabilities"]["streaming"].as_bool().unwrap());
        assert!(dispatcher.initialized);
    }

    #[tokio::test]
    async fn test_agent_list() {
        let mut dispatcher = AcpDispatcher::new(vec!["a1".to_string(), "a2".to_string()]);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: "agent/list".to_string(),
            params: serde_json::json!({}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_none());
        let agents = resp.result.unwrap();
        let arr = agents.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], "a1");
    }

    #[tokio::test]
    async fn test_method_not_found() {
        let mut dispatcher = AcpDispatcher::new(vec![]);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(3)),
            method: "nonexistent/method".to_string(),
            params: serde_json::json!({}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_agent_message_before_init() {
        let mut dispatcher = AcpDispatcher::new(vec!["agent-1".to_string()]);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(4)),
            method: "agent/message".to_string(),
            params: serde_json::json!({"agent_id": "agent-1", "message": "hello"}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, INVALID_REQUEST);
    }

    #[tokio::test]
    async fn test_agent_message_after_init() {
        let mut dispatcher = AcpDispatcher::new(vec!["agent-1".to_string()]);

        // Initialize first
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "initialize".to_string(),
            params: serde_json::json!({}),
        };
        dispatcher.dispatch(&init_req).await;

        let msg_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: "agent/message".to_string(),
            params: serde_json::json!({"agent_id": "agent-1", "message": "hello"}),
        };

        let resp = dispatcher.dispatch(&msg_req).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["response"].as_str().unwrap().contains("agent-1"));
        assert!(!result["session_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_agent_message_unknown_agent() {
        let mut dispatcher = AcpDispatcher::new(vec!["agent-1".to_string()]);
        dispatcher.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(5)),
            method: "agent/message".to_string(),
            params: serde_json::json!({"agent_id": "nonexistent", "message": "hello"}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn test_shutdown() {
        let mut dispatcher = AcpDispatcher::new(vec![]);
        dispatcher.initialized = true;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(99)),
            method: "shutdown".to_string(),
            params: serde_json::json!({}),
        };

        let resp = dispatcher.dispatch(&req).await;
        assert!(resp.error.is_none());
        assert!(!dispatcher.initialized);
    }

    #[test]
    fn test_serialize_response() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let json = AcpDispatcher::serialize_response(&resp);
        assert!(json.ends_with('\n'));
        assert!(json.contains("\"ok\":true"));
    }

    #[test]
    fn test_progress_notification() {
        let notif = AcpDispatcher::progress_notification("agent-1", "Processing...");
        assert_eq!(notif.method, "agent/progress");
        assert_eq!(notif.params["agent_id"], "agent-1");
    }

    #[test]
    fn test_tool_call_notification() {
        let notif = AcpDispatcher::tool_call_notification("agent-1", "read", "started");
        assert_eq!(notif.method, "agent/toolCall");
        assert_eq!(notif.params["tool_name"], "read");
    }
}
