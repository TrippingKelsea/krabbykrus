//! Remote tool execution over Noise Protocol encrypted channels.
//!
//! When a TUI or other client connects to a remote gateway, it can advertise
//! tool execution capabilities (e.g., filesystem access, browser control).
//! The gateway routes tool calls to the most appropriate executor:
//!
//! 1. Check connected clients for matching capabilities
//! 2. Fall back to local (gateway) execution if no remote executor matches
//!
//! All remote tool dispatch uses Noise Protocol (XX pattern) for mutual
//! authentication and encrypted transport over the existing WebSocket.
//!
//! # Capability Model
//!
//! Clients advertise capability categories:
//! - `filesystem` — read, write, edit, glob, grep, patch
//! - `shell` — exec, test, lint
//! - `browser` — browser tool, web_fetch
//! - `network` — web_search, web_fetch
//!
//! The gateway matches tool names to categories and dispatches accordingly.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, warn};

// Re-export snow types so downstream crates (rockbot-cli) can use them
// without taking a direct dependency on the `snow` crate.
pub use snow::{HandshakeState, Keypair};

// ---------------------------------------------------------------------------
// Capability categories
// ---------------------------------------------------------------------------

/// Tool execution capability categories that a client can advertise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCapability {
    /// File read/write/edit/glob/grep/patch
    Filesystem,
    /// Shell execution (exec, test, lint)
    Shell,
    /// Browser automation
    Browser,
    /// Network access (web_fetch, web_search)
    Network,
    /// Agent delegation (invoke_agent, handoff)
    Agent,
    /// Memory operations
    Memory,
    /// All capabilities
    Full,
}

impl ToolCapability {
    /// Map a tool name to its required capability category.
    pub fn for_tool(tool_name: &str) -> Option<Self> {
        match tool_name {
            "read" | "write" | "edit" | "glob" | "grep" | "patch" => Some(Self::Filesystem),
            "exec" | "test" | "lint" => Some(Self::Shell),
            "browser" => Some(Self::Browser),
            "web_fetch" | "web_search" => Some(Self::Network),
            "invoke_agent" | "handoff" => Some(Self::Agent),
            "memory_get" | "memory_search" => Some(Self::Memory),
            "clarify" => None, // Always handled by the client presenting the UI
            _ => None,         // Unknown tools execute on gateway
        }
    }
}

/// Client capability advertisement sent during the Noise handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Capability categories this client supports.
    pub capabilities: HashSet<ToolCapability>,
    /// Client type identifier (e.g., "tui", "browser", "daemon").
    pub client_type: String,
    /// Human-readable client name.
    pub client_name: Option<String>,
    /// Working directory on the client (for filesystem tools).
    pub working_dir: Option<String>,
}

impl ClientCapabilities {
    /// Create capabilities for a TUI client (full local access).
    pub fn tui() -> Self {
        let mut capabilities = HashSet::new();
        capabilities.insert(ToolCapability::Filesystem);
        capabilities.insert(ToolCapability::Shell);
        capabilities.insert(ToolCapability::Network);
        Self {
            capabilities,
            client_type: "tui".to_string(),
            client_name: None,
            working_dir: None,
        }
    }

    /// Create capabilities for a browser client.
    pub fn browser() -> Self {
        let mut capabilities = HashSet::new();
        capabilities.insert(ToolCapability::Browser);
        Self {
            capabilities,
            client_type: "browser".to_string(),
            client_name: None,
            working_dir: None,
        }
    }

    /// Check if this client can execute a given tool.
    pub fn can_execute(&self, tool_name: &str) -> bool {
        if self.capabilities.contains(&ToolCapability::Full) {
            return true;
        }
        match ToolCapability::for_tool(tool_name) {
            Some(cap) => self.capabilities.contains(&cap),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Noise Protocol handshake
// ---------------------------------------------------------------------------

/// Noise Protocol handshake state for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseState {
    /// Initial state — no handshake started.
    Pending,
    /// Handshake in progress (messages exchanged).
    Handshaking,
    /// Handshake complete — transport encryption active.
    Established,
    /// Handshake failed.
    Failed,
}

/// A Noise-secured remote execution session.
///
/// Wraps the `snow` transport state and tracks client capabilities.
pub struct NoiseSession {
    /// Connection ID (matches WS connection ID).
    pub conn_id: String,
    /// Noise transport state for encrypt/decrypt.
    pub transport: snow::TransportState,
    /// Client capabilities (set after handshake).
    pub capabilities: ClientCapabilities,
    /// Channel to send tool execution requests to this client.
    pub tool_tx: mpsc::UnboundedSender<RemoteToolRequest>,
}

impl NoiseSession {
    /// Encrypt a message for sending to the client.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, snow::Error> {
        let mut buf = vec![0u8; plaintext.len() + 64]; // 16 bytes AEAD tag + overhead
        let len = self.transport.write_message(plaintext, &mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }

    /// Decrypt a message received from the client.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, snow::Error> {
        let mut buf = vec![0u8; ciphertext.len()];
        let len = self.transport.read_message(ciphertext, &mut buf)?;
        buf.truncate(len);
        Ok(buf)
    }
}

impl std::fmt::Debug for NoiseSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoiseSession")
            .field("conn_id", &self.conn_id)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Remote tool dispatch messages
// ---------------------------------------------------------------------------

/// A tool execution request sent to a remote client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteToolRequest {
    /// Unique request ID for correlating responses.
    pub request_id: String,
    /// Tool name to execute.
    pub tool_name: String,
    /// Tool parameters (JSON string).
    pub params: String,
    /// Agent ID requesting the tool.
    pub agent_id: String,
    /// Session ID.
    pub session_id: String,
    /// Working directory for the tool.
    pub workspace_path: String,
}

/// A tool execution response from a remote client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteToolResponse {
    /// Matching request ID.
    pub request_id: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// Tool output (text result or error message).
    pub output: String,
    /// Execution time in milliseconds.
    pub execution_time_ms: u64,
}

// ---------------------------------------------------------------------------
// WS message types for remote execution
// ---------------------------------------------------------------------------

/// Client -> Server messages for the remote execution protocol.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RemoteExecClientMsg {
    /// Noise handshake message (initiator -> responder).
    #[serde(rename = "noise_handshake")]
    NoiseHandshake {
        /// Base64-encoded handshake payload.
        payload: String,
    },
    /// Client advertises capabilities after Noise handshake completes.
    #[serde(rename = "capabilities")]
    Capabilities(ClientCapabilities),
    /// Tool execution result from the client.
    #[serde(rename = "tool_response")]
    ToolResponse(RemoteToolResponse),
}

/// Server -> Client messages for the remote execution protocol.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RemoteExecServerMsg {
    /// Noise handshake message (responder -> initiator).
    #[serde(rename = "noise_handshake")]
    NoiseHandshake {
        /// Base64-encoded handshake payload.
        payload: String,
    },
    /// Server acknowledges capabilities.
    #[serde(rename = "capabilities_ack")]
    CapabilitiesAck {
        /// Whether the server accepts this client as a remote executor.
        accepted: bool,
    },
    /// Tool execution request dispatched to the client.
    #[serde(rename = "tool_request")]
    ToolRequest(RemoteToolRequest),
}

// ---------------------------------------------------------------------------
// Remote executor registry
// ---------------------------------------------------------------------------

/// Tracks Noise sessions and routes tool calls to capable clients.
pub struct RemoteExecutorRegistry {
    /// Active Noise sessions keyed by connection ID.
    sessions: RwLock<HashMap<String, Arc<tokio::sync::Mutex<NoiseSession>>>>,
    /// Pending response channels keyed by request ID.
    pending: RwLock<HashMap<String, oneshot::Sender<RemoteToolResponse>>>,
}

impl RemoteExecutorRegistry {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
        }
    }

    /// Register a completed Noise session.
    pub async fn register(&self, session: NoiseSession) {
        let conn_id = session.conn_id.clone();
        info!(
            "Registered remote executor: {} ({} capabilities)",
            conn_id,
            session.capabilities.capabilities.len()
        );
        self.sessions
            .write()
            .await
            .insert(conn_id, Arc::new(tokio::sync::Mutex::new(session)));
    }

    /// Remove a session (on disconnect).
    pub async fn remove(&self, conn_id: &str) {
        self.sessions.write().await.remove(conn_id);
        debug!("Removed remote executor: {conn_id}");
    }

    /// Find a session that can execute the given tool.
    ///
    /// Returns the connection ID of the best match, preferring:
    /// 1. TUI clients (full local access)
    /// 2. Clients with matching capability
    pub async fn find_executor(&self, tool_name: &str) -> Option<String> {
        let sessions = self.sessions.read().await;
        let mut best: Option<(String, &str)> = None;

        for (conn_id, session) in sessions.iter() {
            let session = session.lock().await;
            if session.capabilities.can_execute(tool_name) {
                // Prefer TUI clients
                if session.capabilities.client_type == "tui" {
                    return Some(conn_id.clone());
                }
                if best.is_none() {
                    best = Some((conn_id.clone(), "other"));
                }
            }
        }

        best.map(|(id, _)| id)
    }

    /// Dispatch a tool call to a remote client and wait for the response.
    ///
    /// Returns `None` if no capable client is found or the client disconnects.
    pub async fn dispatch(
        &self,
        tool_name: &str,
        params: &str,
        agent_id: &str,
        session_id: &str,
        workspace_path: &str,
        timeout: std::time::Duration,
    ) -> Option<RemoteToolResponse> {
        let conn_id = self.find_executor(tool_name).await?;
        let sessions = self.sessions.read().await;
        let session = sessions.get(&conn_id)?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let request = RemoteToolRequest {
            request_id: request_id.clone(),
            tool_name: tool_name.to_string(),
            params: params.to_string(),
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            workspace_path: workspace_path.to_string(),
        };

        // Set up response channel
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending
            .write()
            .await
            .insert(request_id.clone(), resp_tx);

        // Send the request to the client
        {
            let session = session.lock().await;
            let _ = session.tool_tx.send(request);
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(timeout, resp_rx).await;

        // Clean up pending entry
        self.pending.write().await.remove(&request_id);

        match result {
            Ok(Ok(response)) => Some(response),
            Ok(Err(_)) => {
                warn!("Remote executor {conn_id} dropped response channel");
                None
            }
            Err(_) => {
                warn!("Remote tool call to {conn_id} timed out");
                None
            }
        }
    }

    /// Deliver a response from a client to the waiting dispatch call.
    pub async fn deliver_response(&self, response: RemoteToolResponse) {
        if let Some(tx) = self.pending.write().await.remove(&response.request_id) {
            let _ = tx.send(response);
        } else {
            warn!(
                "Received tool response for unknown request: {}",
                response.request_id
            );
        }
    }

    /// Number of active remote executors.
    pub async fn executor_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// List connected executors with their capabilities.
    pub async fn list_executors(&self) -> Vec<(String, ClientCapabilities)> {
        let sessions = self.sessions.read().await;
        let mut result = Vec::new();
        for (conn_id, session) in sessions.iter() {
            let session = session.lock().await;
            result.push((conn_id.clone(), session.capabilities.clone()));
        }
        result
    }
}

impl Default for RemoteExecutorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Noise Protocol helpers
// ---------------------------------------------------------------------------

/// Noise Protocol pattern used for the handshake.
///
/// XX pattern: mutual authentication, no pre-shared keys.
/// Both sides prove identity during the 3-message handshake.
pub const NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";

/// Create a Noise responder (gateway side) for a new connection.
pub fn create_responder(static_key: &snow::Keypair) -> Result<snow::HandshakeState, snow::Error> {
    snow::Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(&static_key.private)
        .build_responder()
}

/// Create a Noise initiator (client side) for connecting to a gateway.
pub fn create_initiator(static_key: &snow::Keypair) -> Result<snow::HandshakeState, snow::Error> {
    snow::Builder::new(NOISE_PATTERN.parse()?)
        .local_private_key(&static_key.private)
        .build_initiator()
}

/// Generate a new static keypair for Noise Protocol.
pub fn generate_keypair() -> Result<snow::Keypair, snow::Error> {
    snow::Builder::new(NOISE_PATTERN.parse()?).generate_keypair()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_tool_capability_mapping() {
        assert_eq!(
            ToolCapability::for_tool("read"),
            Some(ToolCapability::Filesystem)
        );
        assert_eq!(
            ToolCapability::for_tool("exec"),
            Some(ToolCapability::Shell)
        );
        assert_eq!(
            ToolCapability::for_tool("browser"),
            Some(ToolCapability::Browser)
        );
        assert_eq!(
            ToolCapability::for_tool("web_fetch"),
            Some(ToolCapability::Network)
        );
        assert_eq!(ToolCapability::for_tool("clarify"), None);
        assert_eq!(ToolCapability::for_tool("unknown_tool"), None);
    }

    #[test]
    fn test_client_capabilities_tui() {
        let caps = ClientCapabilities::tui();
        assert!(caps.can_execute("read"));
        assert!(caps.can_execute("write"));
        assert!(caps.can_execute("exec"));
        assert!(caps.can_execute("web_fetch"));
        assert!(!caps.can_execute("browser"));
        assert!(!caps.can_execute("invoke_agent"));
    }

    #[test]
    fn test_client_capabilities_browser() {
        let caps = ClientCapabilities::browser();
        assert!(caps.can_execute("browser"));
        assert!(!caps.can_execute("read"));
        assert!(!caps.can_execute("exec"));
    }

    #[test]
    fn test_capabilities_serde_roundtrip() {
        let caps = ClientCapabilities::tui();
        let json = serde_json::to_string(&caps).unwrap();
        let parsed: ClientCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.client_type, "tui");
        assert!(parsed.can_execute("read"));
    }

    #[test]
    fn test_noise_keypair_generation() {
        let kp = generate_keypair().unwrap();
        assert!(!kp.private.is_empty());
        assert!(!kp.public.is_empty());
    }

    #[test]
    fn test_noise_handshake() {
        let server_key = generate_keypair().unwrap();
        let client_key = generate_keypair().unwrap();

        let mut server = create_responder(&server_key).unwrap();
        let mut client = create_initiator(&client_key).unwrap();

        let mut buf = vec![0u8; 65535];

        // Message 1: client -> server (e)
        let len = client.write_message(&[], &mut buf).unwrap();
        let msg1 = buf[..len].to_vec();

        // Message 2: server -> client (e, ee, s, es)
        server.read_message(&msg1, &mut buf).unwrap();
        let len = server.write_message(&[], &mut buf).unwrap();
        let msg2 = buf[..len].to_vec();

        // Message 3: client -> server (s, se)
        client.read_message(&msg2, &mut buf).unwrap();
        let len = client.write_message(&[], &mut buf).unwrap();
        let msg3 = buf[..len].to_vec();

        server.read_message(&msg3, &mut buf).unwrap();

        // Both sides should now be ready for transport
        assert!(client.is_handshake_finished());
        assert!(server.is_handshake_finished());

        let mut client_transport = client.into_transport_mode().unwrap();
        let mut server_transport = server.into_transport_mode().unwrap();

        // Test encrypt/decrypt
        let plaintext = b"hello remote executor";
        let len = client_transport.write_message(plaintext, &mut buf).unwrap();
        let ciphertext = &buf[..len];

        let mut out = vec![0u8; 65535];
        let len = server_transport.read_message(ciphertext, &mut out).unwrap();
        assert_eq!(&out[..len], plaintext);
    }

    #[tokio::test]
    async fn test_registry_find_executor() {
        let registry = RemoteExecutorRegistry::new();
        assert!(registry.find_executor("read").await.is_none());
    }

    #[test]
    fn test_remote_tool_request_serde() {
        let req = RemoteToolRequest {
            request_id: "req-1".to_string(),
            tool_name: "read".to_string(),
            params: r#"{"path": "/tmp/test.txt"}"#.to_string(),
            agent_id: "main".to_string(),
            session_id: "sess-1".to_string(),
            workspace_path: "/home/user".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: RemoteToolRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, "read");
    }
}
