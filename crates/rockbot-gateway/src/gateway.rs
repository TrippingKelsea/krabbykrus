//! Gateway server for RockBot
//!
//! This module provides the main gateway server that handles WebSocket connections,
//! HTTP API endpoints, and coordinates agent execution.
#![allow(
    clippy::expect_used,
    clippy::single_match_else,
    clippy::redundant_closure_for_method_calls,
    clippy::unnecessary_map_or,
    clippy::uninlined_format_args,
    clippy::manual_let_else,
    clippy::let_and_return
)]

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use rockbot_agent::agent::{Agent, AgentResponse, ProcessMessageOptions};
use rockbot_config::{Config, CredentialsConfig, GatewayConfig, PkiConfig};
use rockbot_credentials::CredentialManager;
use rockbot_pki::PkiManager;

fn tool_locality_label(locality: &rockbot_agent::agent::ToolExecutionLocality) -> String {
    match locality {
        rockbot_agent::agent::ToolExecutionLocality::Gateway => "gateway".to_string(),
        rockbot_agent::agent::ToolExecutionLocality::ActiveClient => "active_client".to_string(),
        rockbot_agent::agent::ToolExecutionLocality::RemoteClient(target) => {
            format!("remote:{target}")
        }
    }
}

use crate::error::{GatewayError, Result};
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Incoming as IncomingBody, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rockbot_config::message::{Message, MessageRole};
use rockbot_session::SessionManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

/// Response body type supporting both full and SSE streaming responses.
type GatewayBody = http_body_util::Either<
    Full<hyper::body::Bytes>,
    StreamBody<
        tokio_stream::wrappers::ReceiverStream<
            std::result::Result<Frame<hyper::body::Bytes>, std::convert::Infallible>,
        >,
    >,
>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ListenerKind {
    Public,
    Client,
}

/// Shared transport state map for Noise Protocol handshakes (remote-exec feature).
///
/// After the 3-step Noise handshake completes, the transport is stored here keyed by
/// connection ID. When the client sends its `remote_capabilities` message, the transport
/// is consumed from this map and moved into the `NoiseSession`.
#[cfg(feature = "remote-exec")]
static NOISE_TRANSPORT_STATES: std::sync::OnceLock<
    tokio::sync::Mutex<HashMap<String, snow::TransportState>>,
> = std::sync::OnceLock::new();
#[cfg(feature = "remote-exec")]
static NOISE_HANDSHAKE_STATES: std::sync::OnceLock<
    tokio::sync::Mutex<HashMap<String, snow::HandshakeState>>,
> = std::sync::OnceLock::new();

/// Pending agent info (for agents that couldn't be created due to missing credentials)
#[derive(Debug, Clone)]
pub struct PendingAgent {
    pub config: rockbot_config::AgentInstance,
    pub reason: String,
}

/// Agent factory callback for creating agents
pub type AgentFactory = Arc<
    dyn Fn(
            rockbot_config::AgentInstance,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Arc<Agent>>> + Send>>
        + Send
        + Sync,
>;

/// Registered provider info exposed via the API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    /// Provider identifier (e.g. "bedrock", "anthropic", "openai", "mock")
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Whether the provider is available (credentials valid, etc.)
    pub available: bool,
    /// Authentication type used (e.g. "aws_credentials", "oauth", "api_key", "none")
    pub auth_type: String,
    /// Available models from this provider
    pub models: Vec<ProviderModelInfo>,
    /// Provider capabilities
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    /// Credential schema (how to configure this provider)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_schema: Option<rockbot_credentials_schema::CredentialSchema>,
}

/// Session export payload returned by the export API
#[derive(Debug, Serialize)]
struct SessionExportPayload {
    agent_id: String,
    session_id: String,
    created_at: String,
    updated_at: String,
    messages: Vec<SessionExportMessage>,
    stats: SessionExportStats,
}

/// A single message in a session export
#[derive(Debug, Serialize)]
struct SessionExportMessage {
    role: String,
    content: String,
    timestamp: String,
}

/// Token usage statistics included in a session export
#[derive(Debug, Serialize)]
struct SessionExportStats {
    total_messages: usize,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

/// Model info returned by the provider API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: Option<String>,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
}

/// Main gateway server
pub struct Gateway {
    /// Gateway configuration
    config: GatewayConfig,
    /// Effective PKI/TLS configuration.
    pki: PkiConfig,
    /// Credentials configuration
    pub(crate) credentials_config: CredentialsConfig,
    /// Path to the TOML config file (for persisting agent changes)
    config_path: Option<std::path::PathBuf>,
    /// Vault store for persisting agents (preferred over TOML when available)
    store: Option<Arc<rockbot_storage::Store>>,
    /// Agent configurations from the config file (source of truth for declared agents)
    agents_config: Arc<RwLock<Vec<rockbot_config::AgentInstance>>>,
    /// Registered agents
    pub(crate) agents: Arc<RwLock<HashMap<String, Arc<Agent>>>>,
    /// Pending agents (couldn't be created, e.g., missing API keys)
    pending_agents: Arc<RwLock<Vec<PendingAgent>>>,
    /// Agent factory for creating new agents
    agent_factory: Option<AgentFactory>,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Credential manager (optional, if credentials are enabled)
    pub(crate) credential_manager: Option<Arc<CredentialManager>>,
    /// LLM provider registry — single source of truth for provider state
    llm_registry: Arc<RwLock<Option<Arc<rockbot_llm::LlmProviderRegistry>>>>,
    /// Cached provider availability (provider_id -> is_configured). Refreshed on registry set.
    provider_configured: Arc<RwLock<HashMap<String, bool>>>,
    /// Channel registry — collects credential schemas from channel plugins
    channel_registry: Arc<rockbot_channels::ChannelRegistry>,
    /// Tool provider registry — collects credential schemas from tool plugins
    tool_provider_registry: Arc<rockbot_tools::ToolProviderRegistry>,
    /// Active WebSocket connections
    ws_connections: Arc<RwLock<HashMap<String, WsConnection>>>,
    /// A2A task store for agent-to-agent protocol
    a2a_task_store: Arc<crate::a2a::TaskStore>,
    /// Recent certificate signing attempts keyed by source IP for rate limiting.
    cert_sign_attempts: Arc<tokio::sync::Mutex<HashMap<std::net::IpAddr, Vec<std::time::Instant>>>>,
    /// Shared blackboard for swarm coordination
    blackboard: Arc<rockbot_agent::orchestration::SwarmBlackboard>,
    /// Cron scheduler for timed job execution
    cron_scheduler: Arc<crate::cron::CronScheduler>,
    /// Embedded overseer for agent behavior monitoring
    #[cfg(feature = "overseer")]
    overseer: Option<Arc<rockbot_overseer::Overseer>>,
    /// Stored error from overseer initialization failure (if configured but init failed)
    #[cfg(feature = "overseer")]
    overseer_init_error: Option<String>,
    /// Embedded butler companion agent
    #[cfg(feature = "butler")]
    butler: Option<Arc<rockbot_butler::Butler>>,
    /// Remote executor registry for Noise-encrypted tool dispatch
    #[cfg(feature = "remote-exec")]
    pub(crate) remote_exec_registry: Arc<rockbot_client::remote_exec::RemoteExecutorRegistry>,
    /// Noise Protocol static keypair (gateway side)
    #[cfg(feature = "remote-exec")]
    noise_keypair: Arc<snow::Keypair>,
    /// S3 CA certificate distributor
    #[cfg(feature = "bedrock-deploy")]
    deploy_distributor: Option<Arc<rockbot_deploy::CaDistributor>>,
    /// Route53 DNS provisioner
    #[cfg(feature = "bedrock-deploy")]
    deploy_dns: Option<Arc<rockbot_deploy::DnsProvisioner>>,
    /// Deploy config (parsed from config.deploy)
    #[cfg(feature = "bedrock-deploy")]
    deploy_config: Option<rockbot_deploy::DeployConfig>,
    /// Stored error from deploy initialization failure
    #[cfg(feature = "bedrock-deploy")]
    deploy_init_error: Option<String>,
    /// Shutdown broadcast channel
    shutdown_tx: broadcast::Sender<()>,
    /// Gateway process start time.
    started_at: std::time::Instant,
}

/// Stable identity for a connected WebSocket client.
///
/// `client_uuid` is assigned by the gateway for the lifetime of the current
/// connection. `hostname` is the machine's self-reported hostname (human
/// readable). `label` is an optional user-chosen alias (e.g. "laptop-1").
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientIdentity {
    /// Connection-scoped client ID assigned by the gateway.
    client_uuid: String,
    /// Machine hostname (self-reported by the client).
    hostname: String,
    /// Optional human-readable label (e.g. "laptop-1", "server-prod").
    label: Option<String>,
}

type WsOutboundSender = tokio::sync::mpsc::Sender<WsMessage>;
const MAX_WS_OUTBOUND_QUEUE: usize = 256;

fn enqueue_ws_message(sender: &WsOutboundSender, message: WsMessage) -> bool {
    match sender.try_send(message) {
        Ok(()) => true,
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            warn!("Dropping websocket message because outbound queue is full");
            false
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
    }
}

/// WebSocket connection information
struct WsConnection {
    sender: WsOutboundSender,
    /// Client identity for targeted cron dispatch and human-readable display.
    /// Set by the client sending a `client_identify` WS message after connecting.
    identity: Option<ClientIdentity>,
    listener_kind: ListenerKind,
    browser_auth: BrowserAuthState,
    connected_at: std::time::Instant,
}

#[derive(Debug, Default)]
struct BrowserAuthState {
    authenticated: bool,
    pending_cert_pem: Option<String>,
    pending_challenge: Option<Vec<u8>>,
    cert_name: Option<String>,
    cert_role: Option<String>,
}

const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MAX_WS_API_BODY_BYTES: usize = 256 * 1024;
const MAX_TOOL_OUTPUT_CHARS: usize = 2000;
const MAX_WS_INFLIGHT_MESSAGES_PER_CONNECTION: usize = 32;

/// WebSocket message types (client -> server)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WsMessageType {
    #[serde(rename = "agent_message")]
    AgentMessage {
        agent_id: String,
        session_key: String,
        message: String,
        workspace: Option<String>,
        executor_target: Option<String>,
        allow_active_client_tools: Option<bool>,
    },
    #[serde(rename = "health_check")]
    HealthCheck,
    #[serde(rename = "ping")]
    Ping,
    /// Client sends this after connecting to declare its identity.
    /// Used for targeted cron job dispatch and human-readable client lists.
    #[serde(rename = "client_identify")]
    ClientIdentify {
        /// Globally unique client UUID (v4). If omitted, the server assigns one.
        #[serde(default)]
        client_uuid: Option<String>,
        /// Machine hostname (self-reported).
        hostname: String,
        /// Optional human-readable label (e.g. "laptop-1", "server-prod")
        #[serde(default)]
        label: Option<String>,
    },
    /// Client sends this in response to a `cron_dispatch` to report the result.
    #[serde(rename = "cron_result")]
    CronResult {
        job_id: String,
        success: bool,
        error: Option<String>,
        output: Option<String>,
    },
    /// Noise Protocol handshake message from the client.
    #[serde(rename = "noise_handshake")]
    NoiseHandshake {
        /// Base64-encoded handshake payload.
        payload: String,
        /// Handshake step (1 or 3 for the XX pattern).
        step: u8,
    },
    /// Client advertises tool execution capabilities after Noise handshake.
    #[serde(rename = "remote_capabilities")]
    RemoteCapabilities {
        /// Capability categories this client supports.
        capabilities: Vec<String>,
        /// Client type (e.g. "tui", "browser").
        client_type: String,
        /// Working directory on the client.
        working_dir: Option<String>,
    },
    /// Tool execution result from a remote client.
    #[serde(rename = "remote_tool_response")]
    RemoteToolResponse {
        request_id: String,
        success: bool,
        output: String,
        execution_time_ms: u64,
    },
    #[serde(rename = "remote_tool_output")]
    RemoteToolOutput {
        request_id: String,
        output: String,
        #[serde(default)]
        stream: Option<String>,
    },
    #[serde(rename = "api_request")]
    ApiRequest {
        request_id: String,
        method: String,
        path: String,
        #[serde(default)]
        body: Option<serde_json::Value>,
    },
    #[serde(rename = "web_auth_begin")]
    WebAuthBegin { certificate_pem: String },
    #[serde(rename = "web_auth_complete")]
    WebAuthComplete { signature: String },
}

/// WebSocket response types (server -> client)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WsResponseType {
    #[serde(rename = "stream_chunk")]
    StreamChunk { session_key: String, delta: String },
    #[serde(rename = "tool_call")]
    ToolCall {
        session_key: String,
        tool_name: String,
        arguments: String,
        locality: Option<String>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        session_key: String,
        tool_name: String,
        result: String,
        success: bool,
        duration_ms: u64,
        locality: Option<String>,
    },
    #[serde(rename = "tool_output")]
    ToolOutput {
        session_key: String,
        tool_name: String,
        output: String,
        locality: Option<String>,
    },
    #[serde(rename = "agent_response")]
    AgentResponseMsg {
        session_key: String,
        content: String,
        tool_calls: Vec<WsToolCallInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tokens_used: Option<WsTokenUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        processing_time_ms: Option<u64>,
    },
    #[serde(rename = "agent_error")]
    AgentError { session_key: String, error: String },
    #[serde(rename = "health_status")]
    HealthStatus { status: GatewayHealth },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { message: String },
    /// Sent to the client after a successful `client_identify` handshake.
    /// Contains the assigned (or confirmed) UUID so the client can persist it.
    #[serde(rename = "client_identity_assigned")]
    ClientIdentityAssigned {
        client_uuid: String,
        hostname: String,
        label: Option<String>,
    },
    /// Structured token usage update (separate from stream_chunk).
    #[serde(rename = "token_usage")]
    TokenUsageMsg {
        session_key: String,
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        cumulative_total: u64,
    },
    /// Thinking/processing status update with phase information.
    #[serde(rename = "thinking_status")]
    ThinkingStatus {
        session_key: String,
        phase: String, // "llm", "tool", "planning", etc.
        tool_name: Option<String>,
        iteration: Option<usize>,
    },
    /// Dispatched to a specific client to execute a cron job remotely.
    /// The client should process the job and reply with `cron_result`.
    #[serde(rename = "cron_dispatch")]
    CronDispatch {
        job_id: String,
        job_name: String,
        agent_id: Option<String>,
        payload: crate::cron::CronPayload,
    },
    /// Noise Protocol handshake response from the server.
    #[serde(rename = "noise_handshake")]
    NoiseHandshake {
        /// Base64-encoded handshake payload.
        payload: String,
        /// Handshake step (2 for the XX pattern).
        step: u8,
    },
    /// Server acknowledges remote execution capabilities.
    #[serde(rename = "remote_capabilities_ack")]
    RemoteCapabilitiesAck { accepted: bool, message: String },
    /// Tool execution request dispatched to the remote client.
    #[serde(rename = "remote_tool_request")]
    RemoteToolRequest {
        request_id: String,
        tool_name: String,
        params: String,
        agent_id: String,
        session_id: String,
        workspace_path: String,
    },
    #[serde(rename = "api_response")]
    ApiResponse {
        request_id: String,
        status: u16,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    #[serde(rename = "web_auth_challenge")]
    WebAuthChallenge { challenge: String },
    #[serde(rename = "web_auth_result")]
    WebAuthResult {
        authenticated: bool,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cert_name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cert_role: Option<String>,
    },
}

/// Tool call info sent over WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WsToolCallInfo {
    tool_name: String,
    result: String,
    success: bool,
    duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    locality: Option<String>,
}

/// Token usage sent over WebSocket
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WsTokenUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

fn truncate_utf8(input: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    let mut iter = input.chars();
    for _ in 0..max_chars {
        let Some(ch) = iter.next() else {
            return input.to_string();
        };
        truncated.push(ch);
    }
    if iter.next().is_some() {
        format!(
            "{}... ({} chars truncated)",
            truncated,
            input.chars().count().saturating_sub(max_chars)
        )
    } else {
        input.to_string()
    }
}

fn split_utf8_chunks(input: &str, max_chars: usize) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = input.chars().collect();
    chars
        .chunks(max_chars)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn json_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|e| {
        error!("Failed to serialize JSON response: {e}");
        "{}".to_string()
    })
}

/// Gateway health status
#[derive(Debug, Serialize, Deserialize)]
pub struct GatewayHealth {
    pub version: String,
    pub uptime_seconds: u64,
    pub active_connections: usize,
    pub active_sessions: usize,
    pub pending_agents: usize,
    pub agents: Vec<rockbot_agent::agent::AgentHealthStatus>,
    pub memory_usage: MemoryUsage,
}

/// Memory usage statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub allocated_bytes: u64,
    pub heap_size_bytes: u64,
}

// Conversion helpers — these live here (not in config.rs) because they reference
// rockbot_tools / rockbot_security types which config should not depend on.

pub fn convert_tool_config(config: rockbot_config::ToolConfig) -> rockbot_tools::ToolConfig {
    rockbot_tools::ToolConfig {
        profile: config.profile,
        deny: config.deny,
        configs: config.configs,
    }
}

pub fn convert_security_config(
    config: rockbot_config::SecurityConfig,
) -> rockbot_security::SecurityConfig {
    rockbot_security::SecurityConfig {
        sandbox: rockbot_security::SandboxConfig {
            mode: config.sandbox.mode,
            scope: config.sandbox.scope,
            image: config.sandbox.image,
        },
        capabilities: rockbot_security::CapabilityConfig {
            filesystem: config.capabilities.filesystem.map(|fs| {
                rockbot_security::FilesystemCapabilities {
                    read_paths: fs.read_paths,
                    write_paths: fs.write_paths,
                    forbidden_paths: fs.forbidden_paths,
                }
            }),
            network: config
                .capabilities
                .network
                .map(|net| rockbot_security::NetworkCapabilities {
                    allowed_domains: net.allowed_domains,
                    blocked_domains: net.blocked_domains,
                    max_request_size: net.max_request_size,
                }),
            process: config.capabilities.process.map(|proc| {
                rockbot_security::ProcessCapabilities {
                    allowed_commands: proc.allowed_commands,
                    blocked_commands: proc.blocked_commands,
                    max_execution_time: proc.max_execution_time,
                }
            }),
        },
    }
}

impl Gateway {
    /// Create a new gateway with the given configuration
    pub async fn new(
        config: Config,
        session_manager: Arc<SessionManager>,
        credential_manager_override: Option<Arc<CredentialManager>>,
    ) -> Result<Self> {
        let (shutdown_tx, _) = broadcast::channel(1);

        // Initialize credential manager if enabled
        let credential_manager = if let Some(manager) = credential_manager_override {
            Some(manager)
        } else if config.credentials.enabled {
            // Check if vault exists first
            if !rockbot_credentials::CredentialVault::exists(&config.credentials.vault_path) {
                info!(
                    "Credential vault not initialized at {}. Use 'rockbot credentials init' or the TUI to set up.",
                    config.credentials.vault_path.display()
                );
                None
            } else {
                match Self::init_credential_manager(&config.credentials).await {
                    Ok(mgr) => {
                        info!("Credential manager initialized");
                        Some(Arc::new(mgr))
                    }
                    Err(e) => {
                        error!("Failed to initialize credential manager: {}", e);
                        None
                    }
                }
            }
        } else {
            debug!("Credential management disabled");
            None
        };

        // Build channel registry from feature-flagged channel crates
        let mut channel_registry = rockbot_channels::ChannelRegistry::new();
        #[cfg(feature = "discord")]
        {
            channel_registry.register(std::sync::Arc::new(
                rockbot_channels_discord::DiscordChannel::default_schema(),
            ));
        }
        #[cfg(feature = "telegram")]
        {
            channel_registry.register(std::sync::Arc::new(
                rockbot_channels_telegram::TelegramChannel::default_schema(),
            ));
        }
        #[cfg(feature = "signal")]
        {
            channel_registry.register(std::sync::Arc::new(
                rockbot_channels_signal::SignalChannel::new(),
            ));
        }

        // Build tool provider registry from feature-flagged tool crates
        let mut tool_provider_registry = rockbot_tools::ToolProviderRegistry::new();
        #[cfg(feature = "tools-credentials")]
        {
            tool_provider_registry.register(std::sync::Arc::new(
                rockbot_tools_credentials::CredentialVaultTool::new(),
            ));
        }
        #[cfg(feature = "tools-mcp")]
        {
            tool_provider_registry.register(std::sync::Arc::new(rockbot_tools_mcp::McpTool::new()));
        }
        #[cfg(feature = "tools-markdown")]
        {
            tool_provider_registry.register(std::sync::Arc::new(
                rockbot_tools_markdown::MarkdownTool::new(),
            ));
        }

        // Initialize overseer if configured and feature-enabled
        #[cfg(feature = "overseer")]
        let (overseer, overseer_init_error) = if let Some(ref overseer_value) = config.overseer {
            match serde_json::from_value::<rockbot_overseer::OverseerConfig>(overseer_value.clone())
            {
                Ok(overseer_config) => {
                    match rockbot_overseer::Overseer::init(overseer_config).await {
                        Ok(o) => {
                            info!("Overseer initialized");
                            (Some(Arc::new(o)), None)
                        }
                        Err(e) => {
                            error!("Failed to initialize overseer: {e}");
                            (None, Some(format!("{e}")))
                        }
                    }
                }
                Err(e) => {
                    error!("Invalid overseer config: {e}");
                    (None, Some(format!("Invalid config: {e}")))
                }
            }
        } else {
            debug!("Overseer not configured");
            (None, None)
        };

        // Initialize S3 CA distributor + DNS provisioner if configured and feature-enabled
        #[cfg(feature = "bedrock-deploy")]
        let (deploy_distributor, deploy_dns, deploy_config_parsed, deploy_init_error) =
            if let Some(ref deploy_value) = config.deploy {
                match serde_json::from_value::<rockbot_deploy::DeployConfig>(deploy_value.clone()) {
                    Ok(deploy_cfg) => {
                        let mut dist = None;
                        let mut dns = None;
                        let mut err = None;

                        match rockbot_deploy::CaDistributor::new(deploy_cfg.clone()).await {
                            Ok(d) => {
                                info!("Deploy: S3 CA distributor initialized");
                                dist = Some(Arc::new(d));
                            }
                            Err(e) => {
                                error!("Failed to initialize S3 CA distributor: {e}");
                                err = Some(format!("S3: {e}"));
                            }
                        }

                        match rockbot_deploy::DnsProvisioner::new(deploy_cfg.clone()).await {
                            Ok(d) => {
                                info!("Deploy: DNS provisioner initialized");
                                dns = Some(Arc::new(d));
                            }
                            Err(e) => {
                                error!("Failed to initialize DNS provisioner: {e}");
                                let msg = format!("DNS: {e}");
                                err = Some(err.map(|prev| format!("{prev}; {msg}")).unwrap_or(msg));
                            }
                        }

                        (dist, dns, Some(deploy_cfg), err)
                    }
                    Err(e) => {
                        error!("Invalid deploy config: {e}");
                        (None, None, None, Some(format!("Invalid config: {e}")))
                    }
                }
            } else {
                debug!("Deploy not configured");
                (None, None, None, None)
            };

        let effective_pki = config.effective_pki();

        let gateway_config = config.gateway.clone();
        let credentials_config = config.credentials.clone();
        let storage_root = config
            .credentials
            .vault_path
            .parent()
            .map_or_else(
                || {
                dirs::config_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                    .join("rockbot")
                },
                std::path::Path::to_path_buf,
            );

        Ok(Self {
            config: gateway_config,
            pki: effective_pki,
            credentials_config,
            config_path: None,
            store: None,
            agents_config: Arc::new(RwLock::new(config.agents.list.clone())),
            agents: Arc::new(RwLock::new(HashMap::new())),
            pending_agents: Arc::new(RwLock::new(Vec::new())),
            agent_factory: None,
            session_manager,
            credential_manager,
            llm_registry: Arc::new(RwLock::new(None)),
            provider_configured: Arc::new(RwLock::new(HashMap::new())),
            channel_registry: Arc::new(channel_registry),
            tool_provider_registry: Arc::new(tool_provider_registry),
            ws_connections: Arc::new(RwLock::new(HashMap::new())),
            a2a_task_store: Arc::new(crate::a2a::TaskStore::new()),
            cert_sign_attempts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            blackboard: Arc::new(rockbot_agent::orchestration::SwarmBlackboard::new()),
            cron_scheduler: {
                let _ = std::fs::create_dir_all(&storage_root);
                let cron_store = match rockbot_storage_runtime::StorageRuntime::new_with_root(
                    &config,
                    storage_root.clone(),
                )
                .await
                {
                    Ok(runtime) => runtime
                        .open_cron_store()
                        .await
                        .map(|opened| (opened.store, opened.descriptor))
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to open cron store: {e}"),
                        }),
                    Err(e) => Err(GatewayError::InvalidRequest {
                        message: format!("Failed to initialize storage runtime: {e}"),
                    }),
                };
                match cron_store {
                    Ok((store, descriptor)) => match crate::cron::CronScheduler::new_with_store(
                        store,
                        &descriptor,
                    )
                    .await
                    {
                        Ok(scheduler) => {
                            info!("Cron scheduler initialized");
                            Arc::new(scheduler)
                        }
                        Err(e) => {
                            error!("Failed to initialize cron scheduler: {}", e);
                            Arc::new(
                                crate::cron::CronScheduler::new(":memory:")
                                    .await
                                    .expect("in-memory cron scheduler should never fail"),
                            )
                        }
                    },
                    Err(e) => {
                        error!("Failed to initialize cron store: {}", e);
                        // Create an in-memory fallback so the gateway can still start
                        Arc::new(
                            crate::cron::CronScheduler::new(":memory:")
                                .await
                                .expect("in-memory cron scheduler should never fail"),
                        )
                    }
                }
            },
            #[cfg(feature = "overseer")]
            overseer,
            #[cfg(feature = "overseer")]
            overseer_init_error,
            #[cfg(feature = "butler")]
            butler: None,
            #[cfg(feature = "remote-exec")]
            remote_exec_registry: Arc::new(
                rockbot_client::remote_exec::RemoteExecutorRegistry::new(),
            ),
            #[cfg(feature = "remote-exec")]
            noise_keypair: Arc::new(
                rockbot_client::remote_exec::generate_keypair()
                    .expect("Noise keypair generation should not fail"),
            ),
            #[cfg(feature = "bedrock-deploy")]
            deploy_distributor,
            #[cfg(feature = "bedrock-deploy")]
            deploy_dns,
            #[cfg(feature = "bedrock-deploy")]
            deploy_config: deploy_config_parsed,
            #[cfg(feature = "bedrock-deploy")]
            deploy_init_error,
            shutdown_tx,
            started_at: std::time::Instant::now(),
        })
    }

    /// Initialize the credential manager based on configuration
    async fn init_credential_manager(config: &CredentialsConfig) -> Result<CredentialManager> {
        let manager = CredentialManager::new(&config.vault_path).map_err(|e| {
            GatewayError::InvalidRequest {
                message: format!("Failed to open credential vault: {e}"),
            }
        })?;

        // Auto-unlock if configured
        match config.unlock_method.as_str() {
            "env" => {
                if let Ok(password) = std::env::var(&config.password_env_var) {
                    manager.unlock_with_password(&password).await.map_err(|e| {
                        GatewayError::InvalidRequest {
                            message: format!("Failed to unlock vault: {e}"),
                        }
                    })?;
                    info!("Vault unlocked via environment variable");
                } else {
                    debug!("Vault password env var not set, vault remains locked");
                }
            }
            "password" => {
                // Vault starts locked, unlock manually via CLI or API
                debug!("Vault configured for password unlock, remains locked until manual unlock");
            }
            "keyring" => {
                warn!("Vault unlock method 'keyring' is configured but not implemented; vault remains locked");
            }
            other => {
                debug!("Unknown unlock method '{}', vault remains locked", other);
            }
        }

        Ok(manager)
    }

    /// Get the credential manager if enabled
    pub fn credential_manager(&self) -> Option<&Arc<CredentialManager>> {
        self.credential_manager.as_ref()
    }

    /// Get a reference to the overseer, if enabled.
    #[cfg(feature = "overseer")]
    pub fn overseer(&self) -> Option<&Arc<rockbot_overseer::Overseer>> {
        self.overseer.as_ref()
    }

    /// Set the butler instance.
    #[cfg(feature = "butler")]
    pub fn set_butler(&mut self, butler: Arc<rockbot_butler::Butler>) {
        self.butler = Some(butler);
    }

    /// Get a reference to the butler, if enabled.
    #[cfg(feature = "butler")]
    pub fn butler(&self) -> Option<&Arc<rockbot_butler::Butler>> {
        self.butler.as_ref()
    }

    /// Publish the CA certificate to S3 and register DNS records.
    ///
    /// Called from gateway startup and from `rockbot cert ca publish` CLI.
    /// Silently skips if deploy is not configured or initialization failed.
    #[cfg(feature = "bedrock-deploy")]
    pub async fn publish_ca_to_s3(&self) {
        let deploy_config = match &self.deploy_config {
            Some(cfg) if cfg.upload_on_startup => cfg.clone(),
            Some(_) => {
                debug!("Deploy: upload_on_startup disabled, skipping");
                return;
            }
            None => {
                if let Some(ref err) = self.deploy_init_error {
                    warn!("Deploy not available: {err}");
                }
                return;
            }
        };

        // Read CA cert from pki_dir
        let pki_dir = self.pki.pki_dir.clone().unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("rockbot")
                .join("pki")
        });

        let ca_cert_path = pki_dir.join("ca.crt");
        let ca_pem = match tokio::fs::read_to_string(&ca_cert_path).await {
            Ok(pem) => pem,
            Err(e) => {
                warn!(
                    "Deploy: CA cert not found at {}: {e}",
                    ca_cert_path.display()
                );
                return;
            }
        };

        // Auto-import AWS credentials if vault is available
        if let Some(ref cred_mgr) = self.credential_manager {
            let importer = rockbot_deploy::AwsCredentialImporter::new(cred_mgr.clone());
            let client_uuid = uuid::Uuid::new_v4().to_string();
            match importer.import_or_prompt(&client_uuid).await {
                Ok(rockbot_deploy::credentials::ImportResult::Imported) => {
                    info!("Deploy: auto-imported AWS credentials into vault");
                }
                Ok(rockbot_deploy::credentials::ImportResult::Conflict {
                    existing_endpoint_name,
                    ..
                }) => {
                    warn!(
                        "Deploy: found different AWS keys vs vault ({}). Use 'rockbot cert ca publish' to resolve.",
                        existing_endpoint_name
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("Deploy: credential import failed: {e}");
                }
            }
        }

        // Provision S3
        if let Some(ref distributor) = self.deploy_distributor {
            match distributor.provision(&ca_pem).await {
                Ok(()) => {
                    info!("Deploy: CA cert published to {}", distributor.ca_cert_url());
                }
                Err(e) => {
                    error!("Deploy: S3 provisioning failed: {e}");
                    return;
                }
            }
        } else {
            warn!("Deploy: S3 distributor not initialized");
            return;
        }

        // Register DNS records
        if let Some(ref dns) = self.deploy_dns {
            let cluster_uuid = uuid::Uuid::new_v4().to_string();
            let s3_endpoint = format!(
                "{}.s3.{}.amazonaws.com",
                deploy_config.bucket, deploy_config.region
            );

            if let Err(e) = dns
                .register_cluster(
                    &cluster_uuid,
                    deploy_config.cluster_name.as_deref(),
                    &s3_endpoint,
                )
                .await
            {
                warn!("Deploy: DNS registration failed: {e}");
            }
        }
    }

    /// Register an agent with the gateway
    pub async fn register_agent(&self, #[allow(unused_mut)] mut agent: Arc<Agent>) {
        #[cfg(feature = "remote-exec")]
        if let Some(a) = Arc::get_mut(&mut agent) {
            a.set_remote_exec_registry(self.remote_exec_registry.clone());
        }
        let agent_id = agent.id().to_string();
        let mut agents = self.agents.write().await;
        agents.insert(agent_id.clone(), agent);
        info!("Registered agent: {}", agent_id);
    }

    /// Create an `AgentInvoker` backed by this gateway's agent map.
    ///
    /// Agents created with this invoker can delegate work to sibling agents
    /// via the `invoke_agent` tool.
    pub fn agent_invoker(&self) -> Arc<dyn rockbot_tools::AgentInvoker> {
        Arc::new(GatewayInvoker::new(Arc::clone(&self.agents)))
    }

    /// Get the shared blackboard for swarm coordination.
    pub fn blackboard(&self) -> Arc<rockbot_agent::orchestration::SwarmBlackboard> {
        Arc::clone(&self.blackboard)
    }

    /// Get the cron scheduler.
    pub fn cron_scheduler(&self) -> &Arc<crate::cron::CronScheduler> {
        &self.cron_scheduler
    }

    /// Start the cron scheduler background loop. Call this after agent registration
    /// so the executor can find agents to invoke.
    pub async fn start_cron_scheduler(&self) {
        let executor = Arc::new(GatewayCronExecutor {
            agents: Arc::clone(&self.agents),
            ws_connections: Arc::clone(&self.ws_connections),
        });
        self.cron_scheduler.start(executor).await;
        info!("Cron scheduler started (jobs will execute via GatewayCronExecutor)");
    }

    /// Register agent-as-tool entries for agents that have `expose_as_tool` configured.
    ///
    /// For each agent with `expose_as_tool`, an `AgentTool` is registered in the
    /// tool registries of all *other* agents so they can call it like any tool.
    /// Call this after all agents have been registered.
    pub async fn register_agent_tools(&self) {
        let agents = self.agents.read().await;
        // Collect agents that expose themselves as tools
        let exposures: Vec<(String, String, String)> = agents
            .values()
            .filter_map(|agent| {
                agent.config.expose_as_tool.as_ref().map(|cfg| {
                    (
                        agent.config.id.clone(),
                        cfg.tool_name.clone(),
                        cfg.description.clone(),
                    )
                })
            })
            .collect();

        if exposures.is_empty() {
            return;
        }

        info!("Registering {} agent-as-tool(s)", exposures.len());

        for (source_agent_id, tool_name, description) in &exposures {
            // Register in every other agent's tool registry
            for (target_id, target_agent) in agents.iter() {
                if target_id == source_agent_id {
                    continue; // Don't register self-tool
                }
                let agent_tool = Arc::new(rockbot_tools::AgentTool::new(
                    source_agent_id.clone(),
                    tool_name.clone(),
                    description.clone(),
                ));
                target_agent.tool_registry().register_tool(agent_tool).await;
                debug!(
                    "Registered agent-tool '{}' (→ agent '{}') in agent '{}'",
                    tool_name, source_agent_id, target_id
                );
            }
        }
    }

    /// Set the agent factory for creating new agents
    pub fn set_agent_factory(&mut self, factory: AgentFactory) {
        self.agent_factory = Some(factory);
    }

    /// Set the config file path used for bootstrap loading and diagnostics.
    pub fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path);
    }

    /// Set the vault store for agent persistence.
    ///
    /// When a store is configured, agents are loaded from the vault `AGENTS`
    /// table instead of the TOML `[[agents.list]]` section. On first startup
    /// with a non-empty `agents.list` and an empty vault, agents are
    /// auto-migrated.
    pub fn set_store(&mut self, store: Arc<rockbot_storage::Store>) {
        self.store = Some(store);
    }

    /// Load agents from the vault store. Returns empty vec if store is not set.
    pub fn load_agents_from_store(&self) -> Vec<rockbot_config::AgentInstance> {
        let Some(ref store) = self.store else {
            return Vec::new();
        };
        match store.list_agents() {
            Ok(agents) => agents,
            Err(e) => {
                warn!("Failed to load agents from vault: {e}");
                Vec::new()
            }
        }
    }

    /// Persist a single agent to the authoritative store if available.
    fn persist_agent_to_store(&self, agent: &rockbot_config::AgentInstance) {
        if let Some(ref store) = self.store {
            if let Err(e) = store.store_agent(&agent.id, agent) {
                warn!("Failed to persist agent '{}' to vault: {e}", agent.id);
            }
        } else {
            warn!(
                "No authoritative agent store is available; agent '{}' change is runtime-only",
                agent.id
            );
        }
    }

    /// Delete an agent from the vault store.
    fn delete_agent_from_store(&self, agent_id: &str) {
        if let Some(ref store) = self.store {
            if let Err(e) = store.delete_agent(agent_id) {
                warn!("Failed to delete agent '{agent_id}' from vault: {e}");
            }
        }
    }

    /// Auto-migrate agents from TOML config to vault store.
    ///
    /// Called during agent registration when the vault is empty but TOML
    /// `agents.list` is non-empty. Each agent is written to the vault.
    pub async fn auto_migrate_agents_to_store(&self) {
        let Some(ref store) = self.store else {
            return;
        };
        let agents_config = self.agents_config.read().await;
        if agents_config.is_empty() {
            return;
        }
        // Only migrate if vault is empty
        match store.list_agents() {
            Ok(existing) if !existing.is_empty() => return,
            Err(_) => return,
            _ => {}
        }
        info!(
            "Migrating {} agent(s) from TOML config to vault store",
            agents_config.len()
        );
        for agent in agents_config.iter() {
            if let Err(e) = store.store_agent(&agent.id, agent) {
                warn!("Failed to migrate agent '{}' to vault: {e}", agent.id);
            } else {
                info!("Migrated agent '{}' to vault", agent.id);
            }
        }
    }

    /// Set the LLM provider registry (single source of truth for provider state).
    /// Probes each provider's `is_configured()` and caches the result.
    pub async fn set_llm_registry(&self, registry: Arc<rockbot_llm::LlmProviderRegistry>) {
        // Probe and cache availability for each provider (with timeout to prevent hangs)
        let mut cache = HashMap::new();
        for provider_id in registry.list_providers() {
            if let Some(provider) = registry.get_provider(&provider_id) {
                let configured = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    provider.is_configured(),
                )
                .await
                .unwrap_or(false);
                info!("Provider '{}': configured={}", provider_id, configured);
                cache.insert(provider_id, configured);
            }
        }
        {
            let mut configured = self.provider_configured.write().await;
            *configured = cache;
        }

        let mut lock = self.llm_registry.write().await;
        *lock = Some(registry);
    }

    /// Refresh the cached provider availability status
    pub async fn refresh_provider_status(&self) {
        let registry = self.llm_registry.read().await;
        if let Some(reg) = registry.as_ref() {
            let mut cache = HashMap::new();
            for provider_id in reg.list_providers() {
                if let Some(provider) = reg.get_provider(&provider_id) {
                    let configured = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        provider.is_configured(),
                    )
                    .await
                    .unwrap_or(false);
                    cache.insert(provider_id, configured);
                }
            }
            let mut configured = self.provider_configured.write().await;
            *configured = cache;
        }
    }

    /// Add a pending agent (couldn't be created, e.g., missing API key)
    pub async fn add_pending_agent(&self, config: rockbot_config::AgentInstance, reason: String) {
        let mut pending = self.pending_agents.write().await;
        // Don't add duplicates
        if !pending.iter().any(|p| p.config.id == config.id) {
            pending.push(PendingAgent { config, reason });
        }
    }

    /// Get list of pending agents
    pub async fn list_pending_agents(&self) -> Vec<PendingAgent> {
        self.pending_agents.read().await.clone()
    }

    /// Attempt to reload/create pending agents
    /// Returns (created_count, still_pending_count)
    pub async fn reload_agents(&self) -> Result<(usize, usize)> {
        let factory = match &self.agent_factory {
            Some(f) => f.clone(),
            None => {
                return Err(GatewayError::InvalidRequest {
                    message: "Agent factory not configured".to_string(),
                }
                .into())
            }
        };

        let mut pending = self.pending_agents.write().await;
        let mut still_pending = Vec::new();
        let mut created = 0;

        for pending_agent in pending.drain(..) {
            let config = pending_agent.config.clone();
            let agent_id = config.id.clone();

            match factory(config).await {
                Ok(mut agent) => {
                    if let Some(a) = Arc::get_mut(&mut agent) {
                        a.set_agent_invoker(self.agent_invoker());
                        a.set_blackboard(self.blackboard());
                        #[cfg(feature = "remote-exec")]
                        a.set_remote_exec_registry(self.remote_exec_registry.clone());
                    }
                    let mut agents = self.agents.write().await;
                    agents.insert(agent_id.clone(), agent);
                    info!("Created agent '{}' on reload", agent_id);
                    created += 1;
                }
                Err(e) => {
                    debug!("Agent '{}' still pending: {}", agent_id, e);
                    still_pending.push(PendingAgent {
                        config: pending_agent.config,
                        reason: e.to_string(),
                    });
                }
            }
        }

        let still_pending_count = still_pending.len();
        *pending = still_pending;

        Ok((created, still_pending_count))
    }

    /// Expand a leading `~` or `~/` to the user's home directory.
    fn expand_tilde(path: &std::path::Path) -> std::path::PathBuf {
        let s = path.to_string_lossy();
        if s == "~" || s.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(s.strip_prefix("~/").unwrap_or(""));
            }
        }
        path.to_path_buf()
    }

    /// Load a TLS acceptor from the configured cert/key paths.
    /// Supports optional mTLS when `tls_ca` is configured.
    fn load_tls_acceptor(
        &self,
        listener_kind: ListenerKind,
    ) -> Result<Option<tokio_rustls::TlsAcceptor>> {
        let (cert_path, key_path) = match (&self.pki.tls_cert, &self.pki.tls_key) {
            (Some(c), Some(k)) => (Self::expand_tilde(c), Self::expand_tilde(k)),
            _ => return Ok(None),
        };

        let tls_config_err = |msg: String| -> crate::error::RockBotError {
            crate::error::RockBotError::Config(rockbot_config::ConfigError::Invalid {
                message: msg,
            })
        };

        let cert_pem = std::fs::read(&cert_path).map_err(|e| {
            tls_config_err(format!(
                "Failed to read TLS cert {}: {e}",
                cert_path.display()
            ))
        })?;
        let key_pem = std::fs::read(&key_path).map_err(|e| {
            tls_config_err(format!(
                "Failed to read TLS key {}: {e}",
                key_path.display()
            ))
        })?;

        let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut &cert_pem[..])
                .filter_map(|r| r.ok())
                .collect();
        if certs.is_empty() {
            return Err(tls_config_err(
                "No valid certificates found in TLS cert file".into(),
            ));
        }

        let key = rustls_pemfile::private_key(&mut &key_pem[..])
            .map_err(|e| tls_config_err(format!("Invalid TLS key: {e}")))?
            .ok_or_else(|| tls_config_err("No private key found in TLS key file".into()))?;

        // Build client auth configuration
        let tls_config = if let Some(ca_path) = &self.pki.tls_ca {
            let ca_path = Self::expand_tilde(ca_path);
            let ca_pem = std::fs::read(&ca_path).map_err(|e| {
                tls_config_err(format!("Failed to read CA cert {}: {e}", ca_path.display()))
            })?;
            let ca_certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls_pemfile::certs(&mut &ca_pem[..])
                    .filter_map(|r| r.ok())
                    .collect();
            if ca_certs.is_empty() {
                return Err(tls_config_err(
                    "No valid certificates found in CA cert file".into(),
                ));
            }

            let mut root_store = rustls::RootCertStore::empty();
            for ca_cert in ca_certs {
                root_store
                    .add(ca_cert)
                    .map_err(|e| tls_config_err(format!("Invalid CA certificate: {e}")))?;
            }

            let crls = if self.pki.require_client_cert || matches!(listener_kind, ListenerKind::Client)
            {
                let crl_path = ca_path.with_file_name("crl.pem");
                if crl_path.exists() {
                    let crl_pem = std::fs::read(&crl_path).map_err(|e| {
                        tls_config_err(format!("Failed to read CRL {}: {e}", crl_path.display()))
                    })?;
                    let crls: Vec<rustls::pki_types::CertificateRevocationListDer<'static>> =
                        rustls_pemfile::crls(&mut &crl_pem[..])
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .map_err(|e| tls_config_err(format!("Invalid CRL PEM: {e}")))?;
                    Some(crls)
                } else {
                    None
                }
            } else {
                None
            };

            let verifier_builder = rustls::server::WebPkiClientVerifier::builder(Arc::new(
                root_store,
            ));
            let verifier_builder = if let Some(crls) = crls {
                verifier_builder.with_crls(crls)
            } else {
                verifier_builder
            };

            let verifier = match listener_kind {
                ListenerKind::Client if self.pki.require_client_cert => {
                    info!(
                        "Client listener mTLS enabled: requiring client certificates (CA: {})",
                        ca_path.display()
                    );
                    verifier_builder
                        .build()
                        .map_err(|e| tls_config_err(format!("Client cert verifier error: {e}")))?
                }
                ListenerKind::Client => {
                    info!(
                        "Client listener TLS enabled: accepting optional client certificates (CA: {})",
                        ca_path.display()
                    );
                    verifier_builder
                        .allow_unauthenticated()
                        .build()
                        .map_err(|e| tls_config_err(format!("Client cert verifier error: {e}")))?
                }
                ListenerKind::Public => {
                    info!(
                        "Public listener TLS enabled: server-auth only (client certs not required) (CA: {})",
                        ca_path.display()
                    );
                    verifier_builder
                        .allow_unauthenticated()
                        .build()
                        .map_err(|e| tls_config_err(format!("Client cert verifier error: {e}")))?
                }
            };

            rustls::ServerConfig::builder()
                .with_client_cert_verifier(verifier)
                .with_single_cert(certs, key)
                .map_err(|e| tls_config_err(format!("TLS config error: {e}")))?
        } else {
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .map_err(|e| tls_config_err(format!("TLS config error: {e}")))?
        };

        Ok(Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls_config))))
    }

    /// Start the gateway server
    pub async fn start(&self) -> Result<()> {
        let listener_hosts = self.config.listener_hosts();
        let mut public_listeners = Vec::new();
        let mut client_listeners = Vec::new();
        for host in &listener_hosts {
            let public_addr = format!("{host}:{}", self.config.port);
            public_listeners.push((
                public_addr.clone(),
                TcpListener::bind(&public_addr)
                    .await
                    .map_err(|_| GatewayError::BindFailed {
                        host: host.clone(),
                        port: self.config.port,
                    })?,
            ));

            let client_addr = format!("{host}:{}", self.config.client_port);
            client_listeners.push((
                client_addr.clone(),
                TcpListener::bind(&client_addr)
                    .await
                    .map_err(|_| GatewayError::BindFailed {
                        host: host.clone(),
                        port: self.config.client_port,
                    })?,
            ));
        }

        let public_tls_acceptor = self.load_tls_acceptor(ListenerKind::Public)?;
        let client_tls_acceptor = self.load_tls_acceptor(ListenerKind::Client)?;

        match &public_tls_acceptor {
            Some(_) => {
                for (addr, _) in &public_listeners {
                    info!("Gateway public listener on {addr} (TLS)");
                }
            }
            None => {
                #[cfg(not(feature = "http-insecure"))]
                {
                    warn!(
                        "No TLS cert configured and http-insecure not enabled. \
                           Run `rockbot config init gateway` to generate a self-signed cert, \
                           or set tls_cert/tls_key in config."
                    );
                }
                for (addr, _) in &public_listeners {
                    info!("Gateway public listener on {addr} (plain HTTP)");
                }
            }
        }
        match &client_tls_acceptor {
            Some(_) => {
                for (addr, _) in &client_listeners {
                    info!("Gateway client listener on {addr} (TLS/mTLS)");
                }
            }
            None => {
                for (addr, _) in &client_listeners {
                    info!("Gateway client listener on {addr} (plain HTTP)");
                }
            }
        }

        for (_, listener) in public_listeners {
            let gateway = self.clone();
            let tls_acceptor = public_tls_acceptor.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                gateway
                    .accept_loop(
                        listener,
                        tls_acceptor,
                        ListenerKind::Public,
                        &mut shutdown_rx,
                    )
                    .await;
            });
        }
        for (_, listener) in client_listeners {
            let gateway = self.clone();
            let tls_acceptor = client_tls_acceptor.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            tokio::spawn(async move {
                gateway
                    .accept_loop(
                        listener,
                        tls_acceptor,
                        ListenerKind::Client,
                        &mut shutdown_rx,
                    )
                    .await;
            });
        }

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let _ = shutdown_rx.recv().await;
        info!("Gateway shutdown requested");

        Ok(())
    }

    async fn accept_loop(
        &self,
        listener: TcpListener,
        tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
        listener_kind: ListenerKind,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let gateway = self.clone();
                            if let Some(ref acceptor) = tls_acceptor {
                                let acceptor = acceptor.clone();
                                tokio::spawn(async move {
                                    match acceptor.accept(stream).await {
                                        Ok(tls_stream) => {
                                            if let Err(e) = gateway.handle_tls_connection(tls_stream, addr, listener_kind).await {
                                                error!("TLS connection error: {e}");
                                            }
                                        }
                                        Err(e) => {
                                            debug!("TLS handshake failed from {addr}: {e}");
                                        }
                                    }
                                });
                            } else {
                                tokio::spawn(async move {
                                    if let Err(e) = gateway.handle_connection(stream, addr, listener_kind).await {
                                        error!("Connection error: {e}");
                                    }
                                });
                            }
                        }
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    }

    /// Handle a TLS-wrapped TCP connection
    async fn handle_tls_connection(
        &self,
        stream: tokio_rustls::server::TlsStream<TcpStream>,
        addr: SocketAddr,
        listener_kind: ListenerKind,
    ) -> Result<()> {
        debug!("New TLS connection from {addr}");

        let io = TokioIo::new(stream);

        let gateway = self.clone();
        let service = service_fn(move |req| {
            let gateway = gateway.clone();
            let mut req = req;
            req.extensions_mut().insert(addr);
            async move { gateway.handle_request(req, listener_kind).await }
        });

        if let Err(err) = http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await
        {
            let msg = format!("{err:?}");
            if msg.contains("IncompleteMessage") {
                debug!("TLS client disconnected early from {addr}: {err}");
            } else {
                error!("Error serving TLS connection from {addr}: {err:?}");
            }
        }

        Ok(())
    }

    /// Handle a new TCP connection
    async fn handle_connection(
        &self,
        stream: TcpStream,
        addr: SocketAddr,
        listener_kind: ListenerKind,
    ) -> Result<()> {
        debug!("New connection from {}", addr);

        let io = TokioIo::new(stream);

        let gateway = self.clone();
        let service = service_fn(move |req| {
            let gateway = gateway.clone();
            let mut req = req;
            req.extensions_mut().insert(addr);
            async move { gateway.handle_request(req, listener_kind).await }
        });

        if let Err(err) = http1::Builder::new()
            .serve_connection(io, service)
            .with_upgrades()
            .await
        {
            // IncompleteMessage is normal — client disconnected before sending a full request
            // (e.g. TUI polling with short timeouts). Only log real errors.
            let msg = format!("{err:?}");
            if msg.contains("IncompleteMessage") {
                debug!("Client disconnected early from {}: {}", addr, err);
            } else {
                error!("Error serving connection from {}: {:?}", addr, err);
            }
        }

        Ok(())
    }

    /// Handle HTTP request (which may be upgraded to WebSocket)
    async fn handle_request(
        &self,
        req: Request<IncomingBody>,
        listener_kind: ListenerKind,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        if listener_kind == ListenerKind::Public {
            return self.handle_public_request(req).await;
        }

        let path = req.uri().path().to_string();

        match (req.method(), path.as_str()) {
            // Web UI
            (&Method::GET, "/") | (&Method::GET, "/index.html") => self.handle_web_ui().await,
            (&Method::GET, "/ws") => {
                self.handle_websocket_upgrade(req, ListenerKind::Client)
                    .await
            }
            // A2A Protocol
            (&Method::GET, "/.well-known/agent.json") => self.handle_agent_card().await,
            (&Method::POST, "/a2a") => self.handle_a2a_request(req).await,
            (&Method::GET, "/health") | (&Method::GET, "/api/status") => {
                self.handle_health_check().await
            }
            (&Method::GET, "/api/metrics") => self.handle_metrics().await,
            (&Method::GET, "/api/topology") => Ok(self.handle_get_topology().await),
            (&Method::GET, "/api/agents") => self.handle_list_agents().await,
            (&Method::POST, "/api/agents") => self.handle_create_agent(req).await,
            // Agent context files API (must precede generic agent PUT/DELETE)
            (&Method::GET, p)
                if p.starts_with("/api/agents/")
                    && p.contains("/files")
                    && !p.ends_with("/files") =>
            {
                Ok(self.handle_get_agent_file(&path).await)
            }
            (&Method::GET, p) if p.starts_with("/api/agents/") && p.ends_with("/files") => {
                Ok(self.handle_list_agent_files(&path).await)
            }
            (&Method::GET, p) if p.starts_with("/api/agents/") && p.ends_with("/objects") => {
                Ok(self.handle_list_agent_objects(&path).await)
            }
            (&Method::PUT, p) if p.starts_with("/api/agents/") && p.contains("/files/") => {
                Ok(self.handle_put_agent_file(&path, req).await)
            }
            (&Method::PUT, p) if p.starts_with("/api/agents/") && p.contains("/objects/") => {
                Ok(self.handle_put_agent_object(&path, req).await)
            }
            (&Method::DELETE, p) if p.starts_with("/api/agents/") && p.contains("/files/") => {
                Ok(self.handle_delete_agent_file(&path).await)
            }
            (&Method::PUT, p) if p.starts_with("/api/agents/") && !p.contains("/message") => {
                self.handle_update_agent(req).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/agents/") && !p.contains("/message") => {
                self.handle_delete_agent(&path).await
            }
            // Credentials API endpoints
            (&Method::GET, "/api/credentials") | (&Method::GET, "/api/credentials/endpoints") => {
                self.handle_list_endpoints().await
            }
            (&Method::POST, "/api/credentials") | (&Method::POST, "/api/credentials/endpoints") => {
                self.handle_create_endpoint(req).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/credentials/endpoints/") => {
                self.handle_delete_endpoint(&path).await
            }
            (&Method::DELETE, p)
                if p.starts_with("/api/credentials/")
                    && !p.contains("/permissions/")
                    && !p.contains("/approvals/") =>
            {
                // DELETE /api/credentials/{id} - alternative to /api/credentials/endpoints/{id}
                let id = path.strip_prefix("/api/credentials/").unwrap_or("");
                let endpoint_path = format!("/api/credentials/endpoints/{id}");
                self.handle_delete_endpoint(&endpoint_path).await
            }
            (&Method::POST, p)
                if p.starts_with("/api/credentials/endpoints/") && p.ends_with("/credential") =>
            {
                self.handle_store_credential(req, &path).await
            }
            // Permissions API
            (&Method::GET, "/api/credentials/permissions") => self.handle_list_permissions().await,
            (&Method::POST, "/api/credentials/permissions") => {
                self.handle_add_permission(req).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/credentials/permissions/") => {
                self.handle_delete_permission(&path).await
            }
            // Audit API
            (&Method::GET, "/api/credentials/audit") => self.handle_get_audit_log(req).await,
            // Approvals API
            (&Method::GET, "/api/credentials/approvals") => self.handle_list_approvals().await,
            (&Method::POST, p)
                if p.starts_with("/api/credentials/approvals/") && p.ends_with("/approve") =>
            {
                self.handle_approve_request(&path, req).await
            }
            (&Method::POST, p)
                if p.starts_with("/api/credentials/approvals/") && p.ends_with("/deny") =>
            {
                self.handle_deny_request(&path, req).await
            }
            (&Method::POST, "/api/credentials/approvals/respond") => {
                self.handle_approval_response(req).await
            }
            (&Method::GET, "/api/credentials/status") => self.handle_credentials_status().await,
            (&Method::POST, "/api/credentials/unlock") => self.handle_unlock_vault(req).await,
            (&Method::POST, "/api/credentials/lock") => self.handle_lock_vault().await,
            (&Method::POST, "/api/credentials/init") => self.handle_init_vault(req).await,
            // Provider API endpoints
            (&Method::GET, "/api/providers") => self.handle_list_providers().await,
            (&Method::GET, p) if p.starts_with("/api/providers/") && !p.contains("/test") => {
                self.handle_get_provider(&path).await
            }
            (&Method::POST, p) if p.starts_with("/api/providers/") && p.ends_with("/test") => {
                self.handle_test_provider(&path).await
            }
            (&Method::POST, "/api/chat") => self.handle_chat(req).await,
            (&Method::GET, "/api/credentials/schemas") => self.handle_credential_schemas().await,
            // Sessions API
            (&Method::GET, "/api/sessions") => self.handle_list_sessions(req).await,
            (&Method::POST, "/api/sessions") => self.handle_create_session(req).await,
            (&Method::GET, p) if p.starts_with("/api/sessions/") && p.ends_with("/messages") => {
                self.handle_get_session_messages(&path).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/sessions/") => {
                self.handle_delete_session(&path).await
            }
            // Gateway management
            (&Method::POST, "/api/gateway/reload") => self.handle_reload_agents().await,
            (&Method::GET, "/api/gateway/pending") => self.handle_list_pending_agents().await,
            (&Method::GET, p)
                if p.starts_with("/api/agents/")
                    && p.contains("/sessions/")
                    && p.ends_with("/export") =>
            {
                self.handle_session_export(req).await
            }
            (&Method::POST, p) if p.starts_with("/api/agents/") && p.ends_with("/stream") => {
                self.handle_agent_message_stream(req).await
            }
            (&Method::POST, p) if p.starts_with("/api/agents/") && p.ends_with("/approve") => {
                self.handle_tool_approval(req).await
            }
            (&Method::POST, p) if p.starts_with("/api/agents/") => {
                self.handle_agent_message(req).await
            }
            // Cron API
            (&Method::GET, "/api/cron/jobs") => self.handle_list_cron_jobs().await,
            (&Method::POST, "/api/cron/jobs") => self.handle_create_cron_job(req).await,
            (&Method::GET, p) if p.starts_with("/api/cron/jobs/") && !p.ends_with("/trigger") => {
                self.handle_get_cron_job(&path).await
            }
            (&Method::PUT, p) if p.starts_with("/api/cron/jobs/") => {
                self.handle_update_cron_job(req, &path).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/cron/jobs/") => {
                self.handle_delete_cron_job(&path).await
            }
            (&Method::POST, p) if p.starts_with("/api/cron/jobs/") && p.ends_with("/trigger") => {
                self.handle_trigger_cron_job(&path).await
            }
            (&Method::GET, "/api/cron/clients") => self.handle_list_cron_clients().await,
            (&Method::GET, "/api/executors") => self.handle_list_executors().await,
            // Certificate API (PSK-authenticated CSR signing)
            (&Method::POST, "/api/cert/sign") => self.handle_cert_sign(req).await,
            (&Method::GET, "/api/cert/ca") => self.handle_cert_ca_info().await,
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(GatewayBody::Left(Full::new("Not Found".into())))
                .unwrap()),
        }
    }

    async fn handle_public_request(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();

        match (req.method(), path.as_str()) {
            (&Method::GET, "/health") => self
                .handle_health_check()
                .await
                .map(Self::with_public_security_headers),
            (&Method::GET, "/") | (&Method::GET, "/index.html")
                if self.config.public.serve_webapp =>
            {
                self.handle_web_ui()
                    .await
                    .map(Self::with_public_security_headers)
            }
            (&Method::GET, p) if p.starts_with("/static/") && self.config.public.serve_webapp => {
                self.handle_web_ui_asset(p)
                    .await
                    .map(Self::with_public_security_headers)
            }
            (&Method::GET, "/ws") => {
                self.handle_websocket_upgrade(req, ListenerKind::Public)
                    .await
            }
            (&Method::GET, "/api/cert/ca") if self.config.public.serve_ca => self
                .handle_cert_ca_info()
                .await
                .map(Self::with_public_security_headers),
            (&Method::POST, "/api/cert/sign") if self.config.public.enrollment_enabled => self
                .handle_cert_sign(req)
                .await
                .map(Self::with_public_security_headers),
            _ => Ok(Self::with_public_security_headers(
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(GatewayBody::Left(Full::new("Not Found".into())))
                    .unwrap(),
            )),
        }
    }

    // ==================== Credentials API Handlers ====================

    /// Handle list endpoints
    async fn handle_list_endpoints(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let endpoints = manager.list_endpoints().await;
        // Don't include credential data in the list
        let endpoint_list: Vec<_> = endpoints
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "name": e.name,
                    "endpoint_type": e.endpoint_type,
                    "base_url": e.base_url,
                    "created_at": e.created_at,
                    "updated_at": e.updated_at,
                })
            })
            .collect();

        let body = json_string(&endpoint_list);
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle create endpoint
    async fn handle_create_endpoint<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct CreateEndpointRequest {
            name: String,
            endpoint_type: String,
            base_url: String,
        }

        let request: CreateEndpointRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        let endpoint_type = match request.endpoint_type.as_str() {
            "home_assistant" => rockbot_credentials::EndpointType::HomeAssistant,
            "gmail" => rockbot_credentials::EndpointType::Gmail,
            "spotify" => rockbot_credentials::EndpointType::Spotify,
            "generic_rest" => rockbot_credentials::EndpointType::GenericRest,
            "generic_oauth2" => rockbot_credentials::EndpointType::GenericOAuth2,
            "api_key_service" => rockbot_credentials::EndpointType::ApiKeyService,
            "basic_auth_service" => rockbot_credentials::EndpointType::BasicAuthService,
            "bearer_token" => rockbot_credentials::EndpointType::BearerToken,
            _ => {
                return Ok(Self::json_error(
                    "Invalid endpoint type",
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        match manager
            .create_endpoint(request.name, endpoint_type, request.base_url)
            .await
        {
            Ok(endpoint) => {
                let body = json_string(&endpoint);
                Ok(Self::json_response(&body, StatusCode::CREATED))
            }
            Err(e) => Ok(Self::json_error(
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle delete endpoint
    async fn handle_delete_endpoint(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let endpoint_id = path
            .strip_prefix("/api/credentials/endpoints/")
            .unwrap_or("");
        let Ok(uuid) = uuid::Uuid::parse_str(endpoint_id) else {
            return Ok(Self::json_error(
                "Invalid endpoint ID",
                StatusCode::BAD_REQUEST,
            ));
        };

        match manager.delete_endpoint(uuid).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::NOT_FOUND)),
        }
    }

    /// Handle store credential
    async fn handle_store_credential<B>(
        &self,
        req: Request<B>,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        // Parse endpoint ID from path
        let endpoint_id = path
            .strip_prefix("/api/credentials/endpoints/")
            .and_then(|s| s.strip_suffix("/credential"))
            .unwrap_or("");
        let Ok(endpoint_uuid) = uuid::Uuid::parse_str(endpoint_id) else {
            return Ok(Self::json_error(
                "Invalid endpoint ID",
                StatusCode::BAD_REQUEST,
            ));
        };

        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct StoreCredentialRequest {
            credential_type: String,
            secret: String, // Base64 encoded
        }

        let request: StoreCredentialRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        let credential_type = match request.credential_type.as_str() {
            "bearer_token" => rockbot_credentials::CredentialType::BearerToken,
            "api_key" => rockbot_credentials::CredentialType::ApiKey {
                header_name: "Authorization".to_string(),
            },
            "basic_auth" => rockbot_credentials::CredentialType::BasicAuth {
                username: String::new(),
            },
            _ => {
                return Ok(Self::json_error(
                    "Invalid credential type",
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        // Decode base64 secret
        let Ok(secret) = BASE64_STANDARD.decode(&request.secret) else {
            return Ok(Self::json_error(
                "Invalid base64 secret",
                StatusCode::BAD_REQUEST,
            ));
        };

        match manager
            .store_credential(endpoint_uuid, credential_type, &secret)
            .await
        {
            Ok(()) => {
                // Refresh provider availability cache after credential change
                self.refresh_provider_status().await;
                Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK))
            }
            Err(e) => Ok(Self::json_error(
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle list pending approvals
    async fn handle_list_approvals(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let approvals = manager.list_pending_approvals().await;
        let body = json_string(&approvals);
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle approval response
    async fn handle_approval_response(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        let response: rockbot_credentials::HilApprovalResponse = match serde_json::from_slice(&body)
        {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        match manager.respond_to_approval(response).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
        }
    }

    /// Handle credentials status
    async fn handle_credentials_status(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let vault_exists =
            rockbot_credentials::CredentialVault::exists(&self.credentials_config.vault_path);

        let Some(manager) = &self.credential_manager else {
            // No manager - either disabled or vault doesn't exist
            let body = serde_json::json!({
                "enabled": self.credentials_config.enabled,
                "initialized": vault_exists,
                "locked": true,
                "endpoint_count": 0,
                "pending_approvals": 0,
                "vault_path": self.credentials_config.vault_path.display().to_string(),
            });
            return Ok(Self::json_response(&body.to_string(), StatusCode::OK));
        };

        let locked = manager.is_locked().await;
        let endpoints = manager.list_endpoints().await;
        let approvals = manager.list_pending_approvals().await;

        let body = serde_json::json!({
            "enabled": true,
            "initialized": true,
            "locked": locked,
            "endpoint_count": endpoints.len(),
            "pending_approvals": approvals.len(),
            "vault_path": self.credentials_config.vault_path.display().to_string(),
        });
        Ok(Self::json_response(&body.to_string(), StatusCode::OK))
    }

    /// Handle unlock vault
    async fn handle_unlock_vault(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct UnlockRequest {
            password: String,
        }

        let request: UnlockRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        match manager.unlock_with_password(&request.password).await {
            Ok(()) => Ok(Self::json_response(
                r#"{"status":"unlocked"}"#,
                StatusCode::OK,
            )),
            Err(e) => Ok(Self::json_error(
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle lock vault
    async fn handle_lock_vault(&self) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        match manager.lock().await {
            Ok(()) => Ok(Self::json_response(
                r#"{"status":"locked"}"#,
                StatusCode::OK,
            )),
            Err(e) => Ok(Self::json_error(
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle init vault - creates a new vault if one doesn't exist
    async fn handle_init_vault(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        // Check if credentials are enabled in config
        if !self.credentials_config.enabled {
            return Ok(Self::json_error(
                "Credential management not enabled in config",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }

        // Check if vault already exists
        if rockbot_credentials::CredentialVault::exists(&self.credentials_config.vault_path) {
            return Ok(Self::json_error(
                "Vault already exists. Use unlock instead.",
                StatusCode::CONFLICT,
            ));
        }

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct InitRequest {
            /// Unlock method: "password" or "keyfile"
            method: Option<String>,
            /// Password (required for password method)
            password: Option<String>,
            /// Keyfile path (optional for keyfile method - auto-generates if not provided)
            keyfile_path: Option<String>,
        }

        let request: InitRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        let method = request.method.as_deref().unwrap_or("password");

        match method {
            "password" => {
                let password = match &request.password {
                    Some(p) if p.len() >= 8 => p.clone(),
                    Some(_) => {
                        return Ok(Self::json_error(
                            "Password must be at least 8 characters",
                            StatusCode::BAD_REQUEST,
                        ))
                    }
                    None => {
                        return Ok(Self::json_error(
                            "Password is required for password method",
                            StatusCode::BAD_REQUEST,
                        ))
                    }
                };

                match rockbot_credentials::CredentialVault::init_with_password(
                    &self.credentials_config.vault_path,
                    &password,
                ) {
                    Ok(_) => {
                        info!(
                            "Vault initialized with password at {}",
                            self.credentials_config.vault_path.display()
                        );
                        Ok(Self::json_response(
                            r#"{"status":"initialized","method":"password"}"#,
                            StatusCode::CREATED,
                        ))
                    }
                    Err(e) => Ok(Self::json_error(
                        &format!("Failed to initialize vault: {e}"),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )),
                }
            }
            "keyfile" => {
                use std::os::unix::fs::OpenOptionsExt;

                let keyfile_path = match self.resolve_keyfile_path(request.keyfile_path.as_deref())
                {
                    Ok(path) => path,
                    Err(e) => return Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
                };

                // Create parent directory if needed
                if let Some(parent) = keyfile_path.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return Ok(Self::json_error(
                            &format!("Failed to prepare keyfile directory: {e}"),
                            StatusCode::INTERNAL_SERVER_ERROR,
                        ));
                    }
                }

                // Generate keyfile if it doesn't exist
                if !keyfile_path.exists() {
                    // Generate 32 random bytes for the key using the existing crypto infrastructure
                    let key_bytes = rockbot_credentials::crypto::generate_salt(); // 32-byte salt works as a key

                    match std::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .mode(0o600)
                        .open(&keyfile_path)
                    {
                        Ok(mut file) => {
                            use std::io::Write;
                            if let Err(e) = file.write_all(&key_bytes) {
                                return Ok(Self::json_error(
                                    &format!("Failed to write keyfile: {e}"),
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                ));
                            }
                        }
                        Err(e) => {
                            return Ok(Self::json_error(
                                &format!("Failed to create keyfile: {e}"),
                                StatusCode::INTERNAL_SERVER_ERROR,
                            ))
                        }
                    }
                }

                match rockbot_credentials::CredentialVault::init_with_keyfile(
                    &self.credentials_config.vault_path,
                    &keyfile_path,
                ) {
                    Ok(_) => {
                        info!(
                            "Vault initialized with keyfile at {}",
                            self.credentials_config.vault_path.display()
                        );
                        let body = serde_json::json!({
                            "status": "initialized",
                            "method": "keyfile",
                            "keyfile_path": keyfile_path.display().to_string(),
                        });
                        Ok(Self::json_response(&body.to_string(), StatusCode::CREATED))
                    }
                    Err(e) => Ok(Self::json_error(
                        &format!("Failed to initialize vault: {e}"),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )),
                }
            }
            _ => Ok(Self::json_error(
                "Invalid method. Use 'password' or 'keyfile'",
                StatusCode::BAD_REQUEST,
            )),
        }
    }

    /// Handle list permissions
    async fn handle_list_permissions(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let permissions = manager.list_path_permissions().await;
        let body = json_string(&permissions);
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle add permission
    async fn handle_add_permission(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct AddPermissionRequest {
            path_pattern: String,
            level: String,
            description: Option<String>,
        }

        let request: AddPermissionRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        let level = match request.level.as_str() {
            "allow" => rockbot_credentials::PermissionLevel::Allow,
            "allow_hil" | "hil" => rockbot_credentials::PermissionLevel::AllowHil,
            "allow_hil_2fa" | "hil_2fa" => rockbot_credentials::PermissionLevel::AllowHil2fa,
            "deny" => rockbot_credentials::PermissionLevel::Deny,
            _ => {
                return Ok(Self::json_error(
                    "Invalid permission level. Use: allow, allow_hil, allow_hil_2fa, deny",
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        let permission = rockbot_credentials::PathPermission {
            id: uuid::Uuid::new_v4(),
            path_pattern: request.path_pattern,
            level,
            description: request.description,
        };

        let id = permission.id;
        manager.add_permission(permission).await;

        let body = serde_json::json!({
            "status": "ok",
            "id": id.to_string(),
        });
        Ok(Self::json_response(&body.to_string(), StatusCode::CREATED))
    }

    /// Handle delete permission
    async fn handle_delete_permission(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        let permission_id = path
            .strip_prefix("/api/credentials/permissions/")
            .unwrap_or("");
        let Ok(uuid) = uuid::Uuid::parse_str(permission_id) else {
            return Ok(Self::json_error(
                "Invalid permission ID",
                StatusCode::BAD_REQUEST,
            ));
        };

        if manager.remove_permission(uuid).await {
            Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK))
        } else {
            Ok(Self::json_error(
                "Permission not found",
                StatusCode::NOT_FOUND,
            ))
        }
    }

    /// Handle get audit log
    async fn handle_get_audit_log(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        // Parse limit from query string
        let limit = req
            .uri()
            .query()
            .and_then(|q| {
                q.split('&').find_map(|pair| {
                    let mut parts = pair.split('=');
                    if parts.next() == Some("limit") {
                        parts.next().and_then(|v| v.parse::<usize>().ok())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(100); // Default to 100 entries

        let entries = manager.get_audit_entries(limit);
        let body = json_string(&entries);
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle approve HIL request
    async fn handle_approve_request(
        &self,
        path: &str,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/approve
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/approve"))
            .unwrap_or("");

        let Ok(uuid) = uuid::Uuid::parse_str(request_id) else {
            return Ok(Self::json_error(
                "Invalid request ID",
                StatusCode::BAD_REQUEST,
            ));
        };

        // Parse optional body for resolved_by
        let resolved_by =
            if let Ok(body) = Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
                if !body.is_empty() {
                    #[derive(Deserialize)]
                    struct ApproveBody {
                        resolved_by: Option<String>,
                    }
                    serde_json::from_slice::<ApproveBody>(&body)
                        .ok()
                        .and_then(|b| b.resolved_by)
                        .unwrap_or_else(|| "api".to_string())
                } else {
                    "api".to_string()
                }
            } else {
                "api".to_string()
            };

        let response = rockbot_credentials::HilApprovalResponse {
            request_id: uuid,
            approved: true,
            resolved_by,
            denial_reason: None,
        };

        match manager.respond_to_approval(response).await {
            Ok(()) => Ok(Self::json_response(
                r#"{"status":"approved"}"#,
                StatusCode::OK,
            )),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
        }
    }

    /// Handle deny HIL request
    async fn handle_deny_request(
        &self,
        path: &str,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error(
                "Credential management not enabled",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/deny
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/deny"))
            .unwrap_or("");

        let Ok(uuid) = uuid::Uuid::parse_str(request_id) else {
            return Ok(Self::json_error(
                "Invalid request ID",
                StatusCode::BAD_REQUEST,
            ));
        };

        // Parse body for resolved_by and denial_reason
        let (resolved_by, denial_reason) =
            if let Ok(body) = Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
                if !body.is_empty() {
                    #[derive(Deserialize)]
                    struct DenyBody {
                        resolved_by: Option<String>,
                        reason: Option<String>,
                    }
                    let parsed = serde_json::from_slice::<DenyBody>(&body).ok();
                    (
                        parsed
                            .as_ref()
                            .and_then(|b| b.resolved_by.clone())
                            .unwrap_or_else(|| "api".to_string()),
                        parsed.and_then(|b| b.reason),
                    )
                } else {
                    ("api".to_string(), None)
                }
            } else {
                ("api".to_string(), None)
            };

        let response = rockbot_credentials::HilApprovalResponse {
            request_id: uuid,
            approved: false,
            resolved_by,
            denial_reason,
        };

        match manager.respond_to_approval(response).await {
            Ok(()) => Ok(Self::json_response(
                r#"{"status":"denied"}"#,
                StatusCode::OK,
            )),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
        }
    }

    // ==================== Web UI ====================

    /// Serve the web UI dashboard
    async fn handle_web_ui(&self) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let html = rockbot_webui::get_dashboard_html();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(GatewayBody::Left(Full::new(html.into())))
            .unwrap())
    }

    /// Serve embedded static web assets.
    async fn handle_web_ui_asset(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        match rockbot_webui::get_static_asset(path) {
            Some((content_type, body)) => Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", content_type)
                .body(GatewayBody::Left(Full::new(body.into())))
                .unwrap()),
            None => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(GatewayBody::Left(Full::new("Not Found".into())))
                .unwrap()),
        }
    }

    // ==================== Provider API Handlers ====================

    /// List all registered providers and their status
    async fn handle_list_providers(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let registry = self.llm_registry.read().await;
        let providers = match registry.as_ref() {
            Some(reg) => self.build_provider_status_list(reg).await,
            None => vec![],
        };

        let json = serde_json::json!({ "providers": providers });
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json_string(&json).into())))
            .unwrap())
    }

    /// Get details for a specific provider
    async fn handle_get_provider(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let provider_id = path.strip_prefix("/api/providers/").unwrap_or("");

        let registry = self.llm_registry.read().await;
        let Some(reg) = registry.as_ref() else {
            return Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    r#"{"error":"LLM registry not initialized"}"#.into(),
                )))
                .unwrap());
        };

        if !reg.has_provider(provider_id) {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    serde_json::json!({"error": format!("Provider '{}' not found", provider_id)})
                        .to_string()
                        .into(),
                )))
                .unwrap());
        }

        let providers = self.build_provider_status_list(reg).await;
        let provider = providers.into_iter().find(|p| p.id == provider_id);

        match provider {
            Some(p) => Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(json_string(&p).into())))
                .unwrap()),
            None => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    serde_json::json!({"error": "Provider not found"})
                        .to_string()
                        .into(),
                )))
                .unwrap()),
        }
    }

    /// Test a provider by sending a simple completion request
    async fn handle_test_provider(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let provider_id = path
            .strip_prefix("/api/providers/")
            .and_then(|p| p.strip_suffix("/test"))
            .unwrap_or("");

        let registry = self.llm_registry.read().await;
        let Some(reg) = registry.as_ref() else {
            return Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    r#"{"error":"LLM registry not initialized"}"#.into(),
                )))
                .unwrap());
        };

        // Test provider: check credentials and list models (with timeout)
        let result = if let Some(provider) = reg.get_provider(provider_id) {
            let configured =
                tokio::time::timeout(std::time::Duration::from_secs(10), provider.is_configured())
                    .await
                    .unwrap_or(false);
            let models = match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                provider.list_models(),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Ok(Vec::new()), // Timeout — treat as empty
            };
            let model_count = models.as_ref().map_or(0, |m| m.len());

            // Update the cached availability
            {
                let mut cache = self.provider_configured.write().await;
                cache.insert(provider_id.to_string(), configured);
            }

            if configured {
                serde_json::json!({
                    "status": "ok",
                    "provider": provider_id,
                    "configured": true,
                    "models_found": model_count,
                })
            } else {
                serde_json::json!({
                    "status": "error",
                    "provider": provider_id,
                    "configured": false,
                    "models_found": model_count,
                    "error": "Provider credentials not configured",
                })
            }
        } else {
            serde_json::json!({
                "status": "error",
                "provider": provider_id,
                "error": "Provider not registered",
            })
        };

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(result.to_string().into())))
            .unwrap())
    }

    /// Handle a chat completion request routed through the gateway
    async fn handle_chat(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        // Parse as raw JSON first to extract agent_id (not part of ChatCompletionRequest)
        let raw_json: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ));
            }
        };
        let agent_id = raw_json
            .get("agent_id")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut chat_req: rockbot_llm::ChatCompletionRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(
                        serde_json::json!({"error": format!("Invalid request: {}", e)})
                            .to_string()
                            .into(),
                    )))
                    .unwrap());
            }
        };

        // If agent_id is provided, look up the agent's system prompt and prepend it
        if let Some(ref agent_id) = agent_id {
            let configs = self.agents_config.read().await;
            if let Some(agent_cfg) = configs.iter().find(|a| a.id == *agent_id) {
                if let Some(ref system_prompt) = agent_cfg.system_prompt {
                    if !system_prompt.is_empty() {
                        // Check if there's already a system message at the start
                        let has_system = chat_req.messages.first().map_or(false, |m| {
                            matches!(m.role, rockbot_llm::MessageRole::System)
                        });
                        if !has_system {
                            chat_req.messages.insert(
                                0,
                                rockbot_llm::Message {
                                    role: rockbot_llm::MessageRole::System,
                                    content: system_prompt.clone(),
                                    images: vec![],
                                    tool_calls: None,
                                    tool_call_id: None,
                                },
                            );
                        }
                    }
                } else {
                    // Try loading from the agent's SYSTEM-PROMPT.md file
                    let storage_root = self
                        .credentials_config
                        .vault_path
                        .parent()
                        .map_or_else(
                            rockbot_storage_runtime::default_storage_root,
                            std::path::PathBuf::from,
                        );
                    if let Ok(content) = rockbot_storage_runtime::read_agent_context_file(
                        &storage_root,
                        agent_id,
                        "SYSTEM-PROMPT.md",
                    )
                    .await
                    {
                        if !content.trim().is_empty() {
                            let has_system = chat_req.messages.first().map_or(false, |m| {
                                matches!(m.role, rockbot_llm::MessageRole::System)
                            });
                            if !has_system {
                                chat_req.messages.insert(
                                    0,
                                    rockbot_llm::Message {
                                        role: rockbot_llm::MessageRole::System,
                                        content: content.trim().to_string(),
                                        images: vec![],
                                        tool_calls: None,
                                        tool_call_id: None,
                                    },
                                );
                            }
                        }
                    }
                }
            }
            drop(configs);
        }

        let registry = self.llm_registry.read().await;
        let Some(reg) = registry.as_ref() else {
            return Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    r#"{"error":"LLM registry not initialized"}"#.into(),
                )))
                .unwrap());
        };

        // Resolve "default" model to the first available provider's first model
        // Resolve "default" model to the first configured provider's first model
        if chat_req.model == "default" {
            let configured_cache = self.provider_configured.read().await;
            for provider_id in reg.list_providers() {
                if provider_id == "mock" {
                    continue;
                }
                if !configured_cache.get(&provider_id).copied().unwrap_or(false) {
                    continue;
                }
                if let Some(provider) = reg.get_provider(&provider_id) {
                    if let Ok(Ok(models)) = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        provider.list_models(),
                    )
                    .await
                    {
                        if let Some(first_model) = models.first() {
                            chat_req.model = format!("{}/{}", provider_id, first_model.id);
                            break;
                        }
                    }
                }
            }
            drop(configured_cache);
        }

        let provider = match reg.get_provider_for_model(&chat_req.model).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(
                        serde_json::json!({"error": format!("{}", e)})
                            .to_string()
                            .into(),
                    )))
                    .unwrap());
            }
        };

        info!(
            "Chat request: model={}, messages={}, agent={}",
            chat_req.model,
            chat_req.messages.len(),
            agent_id.as_deref().unwrap_or("none")
        );

        match provider.chat_completion(chat_req).await {
            Ok(response) => Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(json_string(&response).into())))
                .unwrap()),
            Err(e) => {
                error!("Chat completion error: {e}");
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(
                        serde_json::json!({"error": format!("{}", e)})
                            .to_string()
                            .into(),
                    )))
                    .unwrap())
            }
        }
    }

    /// Build provider status list from the registry
    async fn build_provider_status_list(
        &self,
        registry: &rockbot_llm::LlmProviderRegistry,
    ) -> Vec<ProviderStatus> {
        let mut statuses = Vec::new();

        for provider_id in registry.list_providers() {
            if let Some(provider) = registry.get_provider(&provider_id) {
                let caps = provider.capabilities();
                let schema = provider.credential_schema();
                let models = tokio::time::timeout(
                    std::time::Duration::from_secs(15),
                    provider.list_models(),
                )
                .await
                .unwrap_or(Ok(Vec::new()))
                .unwrap_or_default()
                .into_iter()
                .map(|m| ProviderModelInfo {
                    id: m.id,
                    name: m.name,
                    description: m.description,
                    kind: m.kind,
                    context_window: m.context_window,
                    max_output_tokens: m.max_output_tokens,
                })
                .collect();

                let name = schema
                    .as_ref()
                    .map_or_else(|| provider_id.clone(), |s| s.provider_name.clone());
                let auth_type = schema
                    .as_ref()
                    .and_then(|s| s.auth_methods.first())
                    .map_or_else(|| "none".to_string(), |m| m.id.clone());

                let configured_cache = self.provider_configured.read().await;
                let available = configured_cache.get(&provider_id).copied().unwrap_or(false);
                drop(configured_cache);

                statuses.push(ProviderStatus {
                    id: provider_id,
                    name,
                    available,
                    auth_type,
                    models,
                    supports_streaming: caps.supports_streaming,
                    supports_tools: caps.supports_tools,
                    supports_vision: caps.supports_vision,
                    credential_schema: schema,
                });
            }
        }

        statuses
    }

    /// Return all credential schemas (LLM providers + channels + tools)
    async fn handle_credential_schemas(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let mut schemas: Vec<rockbot_credentials_schema::CredentialSchema> = Vec::new();

        // Collect from LLM providers
        let registry = self.llm_registry.read().await;
        if let Some(reg) = registry.as_ref() {
            schemas.extend(reg.credential_schemas());
        }

        // Collect from channel registry (self-registered by feature-flagged channel crates)
        schemas.extend(self.channel_registry.credential_schemas());

        // Collect from tool provider registry (self-registered by feature-flagged tool crates)
        schemas.extend(self.tool_provider_registry.credential_schemas());

        let json = serde_json::json!({ "schemas": schemas });
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json_string(&json).into())))
            .unwrap())
    }

    // ==================== Session API Handlers ====================

    /// Handle list sessions (GET /api/sessions?agent_id=xxx)
    async fn handle_list_sessions<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
    {
        // Parse optional query params
        let uri = req.uri();
        let query_string = uri.query().unwrap_or("");
        let mut agent_id_filter: Option<String> = None;

        for pair in query_string.split('&') {
            if let Some(val) = pair.strip_prefix("agent_id=") {
                if !val.is_empty() {
                    agent_id_filter = Some(val.to_string());
                }
            }
        }

        let query = rockbot_session::SessionQuery {
            agent_id: agent_id_filter,
            exclude_archived: true,
            limit: Some(100),
            ..Default::default()
        };

        match self.session_manager.query_sessions(query).await {
            Ok(sessions) => {
                let json = serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string());
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(
                &format!("Failed to query sessions: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle create session (POST /api/sessions)
    async fn handle_create_session<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };
        let body_str = String::from_utf8_lossy(&body);

        #[derive(Deserialize)]
        struct CreateSessionRequest {
            agent_id: Option<String>,
            model: Option<String>,
        }

        let parsed: CreateSessionRequest = match serde_json::from_str(&body_str) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        // Use agent_id if provided, otherwise "ad-hoc"
        let agent_id = parsed.agent_id.as_deref().unwrap_or("ad-hoc");
        let session_key = uuid::Uuid::new_v4().to_string();

        match self
            .session_manager
            .create_session(agent_id, &session_key)
            .await
        {
            Ok(mut session) => {
                // Resolve model: use explicit model, or fall back to agent's configured model
                let model = if let Some(model) = parsed.model {
                    Some(model)
                } else {
                    let configs = self.agents_config.read().await;
                    configs
                        .iter()
                        .find(|c| c.id == agent_id)
                        .and_then(|c| c.model.clone())
                };
                if let Some(model) = model {
                    session.set_metadata("model", &model);
                    let _ = self.session_manager.update_session(&session).await;
                }

                let json = serde_json::to_string(&session).unwrap_or_else(|_| "{}".to_string());
                Ok(Response::builder()
                    .status(StatusCode::CREATED)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(
                &format!("Failed to create session: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle delete session (DELETE /api/sessions/{id})
    async fn handle_delete_session(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let session_id = path.strip_prefix("/api/sessions/").unwrap_or("");
        if session_id.is_empty() {
            return Ok(Self::json_error(
                "Missing session ID",
                StatusCode::BAD_REQUEST,
            ));
        }

        match self.session_manager.archive_session(session_id).await {
            Ok(()) => Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new("{\"archived\":true}".into())))
                .unwrap()),
            Err(e) => Ok(Self::json_error(
                &format!("Failed to archive session: {e}"),
                StatusCode::NOT_FOUND,
            )),
        }
    }

    /// Handle get session messages (GET /api/sessions/{id}/messages)
    async fn handle_get_session_messages(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let session_id = path
            .strip_prefix("/api/sessions/")
            .and_then(|p| p.strip_suffix("/messages"))
            .unwrap_or("");
        if session_id.is_empty() {
            return Ok(Self::json_error(
                "Missing session ID",
                StatusCode::BAD_REQUEST,
            ));
        }

        match self
            .session_manager
            .get_message_history(session_id, Some(200), None)
            .await
        {
            Ok(history) => {
                let json = serde_json::to_string(&history)
                    .unwrap_or_else(|_| r#"{"messages":[],"total_count":0}"#.to_string());
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(
                &format!("Failed to get messages: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    // ==================== Session Export ====================

    /// Handle session export (GET /api/agents/{agent_id}/sessions/{session_id}/export)
    async fn handle_session_export(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        // Path: /api/agents/{agent_id}/sessions/{session_id}/export
        let segments: Vec<&str> = path.splitn(8, '/').collect();
        // segments: ["", "api", "agents", agent_id, "sessions", session_id, "export"]
        if segments.len() < 7 {
            return Ok(Self::json_error("Invalid path", StatusCode::BAD_REQUEST));
        }
        let agent_id = segments[3].to_string();
        let session_id = segments[5].to_string();
        if agent_id.is_empty() || session_id.is_empty() {
            return Ok(Self::json_error(
                "Missing agent_id or session_id",
                StatusCode::BAD_REQUEST,
            ));
        }

        let session = match self.session_manager.get_session(&session_id).await {
            Ok(Some(s)) => s,
            Ok(None) => return Ok(Self::json_error("Session not found", StatusCode::NOT_FOUND)),
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Failed to get session: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ))
            }
        };

        if session.agent_id != agent_id {
            return Ok(Self::json_error("Session not found", StatusCode::NOT_FOUND));
        }

        let history = match self
            .session_manager
            .get_message_history(&session_id, None, None)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Failed to get messages: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ))
            }
        };

        let messages: Vec<SessionExportMessage> = history
            .messages
            .iter()
            .map(|stored| {
                let role = match stored.message.metadata.role {
                    rockbot_config::message::MessageRole::User => "user",
                    rockbot_config::message::MessageRole::Assistant => "assistant",
                    rockbot_config::message::MessageRole::System => "system",
                    rockbot_config::message::MessageRole::Tool => "tool",
                }
                .to_string();
                let content = match &stored.message.content {
                    rockbot_config::message::MessageContent::Text { text } => text.clone(),
                    rockbot_config::message::MessageContent::System { message, .. } => {
                        message.clone()
                    }
                    rockbot_config::message::MessageContent::Error { error, .. } => error.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                SessionExportMessage {
                    role,
                    content,
                    timestamp: stored.message.created_at.to_rfc3339(),
                }
            })
            .collect();

        let payload = SessionExportPayload {
            agent_id,
            session_id: session.id.clone(),
            created_at: session.created_at.to_rfc3339(),
            updated_at: session.updated_at.to_rfc3339(),
            stats: SessionExportStats {
                total_messages: history.total_count,
                input_tokens: session.token_stats.input_tokens,
                output_tokens: session.token_stats.output_tokens,
                total_tokens: session.token_stats.total_tokens,
            },
            messages,
        };

        let json = serde_json::to_string(&payload).unwrap_or_default();
        Ok(Self::json_response(&json, StatusCode::OK))
    }

    // ==================== Gateway Management Handlers ====================

    /// Handle reload agents request
    async fn handle_reload_agents(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        match self.reload_agents().await {
            Ok((created, pending)) => {
                if created > 0 {
                    self.register_agent_tools().await;
                }
                let body = serde_json::json!({
                    "status": "ok",
                    "agents_created": created,
                    "agents_pending": pending,
                });
                Ok(Self::json_response(&body.to_string(), StatusCode::OK))
            }
            Err(e) => Ok(Self::json_error(
                &e.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle list pending agents request
    async fn handle_list_pending_agents(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let pending = self.list_pending_agents().await;
        let pending_info: Vec<_> = pending
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.config.id,
                    "model": p.config.model,
                    "reason": p.reason,
                })
            })
            .collect();

        let body = serde_json::json!({
            "pending_agents": pending_info,
            "count": pending.len(),
        });
        Ok(Self::json_response(&body.to_string(), StatusCode::OK))
    }

    // ==================== Helper methods ====================

    fn json_response(body: &str, status: StatusCode) -> Response<GatewayBody> {
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.to_string().into())))
            .unwrap()
    }

    fn json_error(message: &str, status: StatusCode) -> Response<GatewayBody> {
        let body = serde_json::json!({
            "error": message,
        });
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.to_string().into())))
            .unwrap()
    }

    fn with_public_security_headers(mut response: Response<GatewayBody>) -> Response<GatewayBody> {
        let headers = response.headers_mut();
        headers.insert(
            hyper::header::HeaderName::from_static("content-security-policy"),
            hyper::header::HeaderValue::from_static(
                "default-src 'self'; connect-src 'self' ws: wss:; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
        );
        headers.insert(
            hyper::header::HeaderName::from_static("x-frame-options"),
            hyper::header::HeaderValue::from_static("DENY"),
        );
        headers.insert(
            hyper::header::HeaderName::from_static("x-content-type-options"),
            hyper::header::HeaderValue::from_static("nosniff"),
        );
        headers.insert(
            hyper::header::HeaderName::from_static("referrer-policy"),
            hyper::header::HeaderValue::from_static("no-referrer"),
        );
        headers.insert(
            hyper::header::HeaderName::from_static("strict-transport-security"),
            hyper::header::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
        response
    }

    async fn collect_limited_body(
        req: Request<IncomingBody>,
        max_bytes: usize,
    ) -> std::result::Result<hyper::body::Bytes, Response<GatewayBody>> {
        let collected = req.collect().await.map_err(|e| {
            Self::json_error(
                &format!("Failed to read request body: {e}"),
                StatusCode::BAD_REQUEST,
            )
        })?;
        let bytes = collected.to_bytes();
        if bytes.len() > max_bytes {
            return Err(Self::json_error(
                &format!("Request body too large (max {max_bytes} bytes)"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        Ok(bytes)
    }

    async fn collect_limited_body_generic<B>(
        req: Request<B>,
        max_bytes: usize,
    ) -> std::result::Result<hyper::body::Bytes, Response<GatewayBody>>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let collected = req.collect().await.map_err(|e| {
            Self::json_error(
                &format!("Failed to read request body: {e:?}"),
                StatusCode::BAD_REQUEST,
            )
        })?;
        let bytes = collected.to_bytes();
        if bytes.len() > max_bytes {
            return Err(Self::json_error(
                &format!("Request body too large (max {max_bytes} bytes)"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        Ok(bytes)
    }

    fn is_valid_agent_id(agent_id: &str) -> bool {
        !agent_id.is_empty()
            && agent_id.len() <= 64
            && !agent_id.contains('/')
            && !agent_id.contains('\\')
            && !agent_id.contains("..")
            && agent_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }

    fn is_valid_cert_name(name: &str) -> bool {
        Self::is_valid_agent_id(name)
    }

    async fn allow_cert_sign_attempt(&self, ip: std::net::IpAddr) -> bool {
        let now = std::time::Instant::now();
        let mut attempts = self.cert_sign_attempts.lock().await;
        Self::record_cert_sign_attempt(&mut attempts, ip, now)
    }

    fn record_cert_sign_attempt(
        attempts: &mut HashMap<std::net::IpAddr, Vec<std::time::Instant>>,
        ip: std::net::IpAddr,
        now: std::time::Instant,
    ) -> bool {
        const CERT_SIGN_MAX_ATTEMPTS: usize = 3;
        const CERT_SIGN_WINDOW_SECS: u64 = 60;

        let entry = attempts.entry(ip).or_default();
        entry.retain(|attempt| now.duration_since(*attempt).as_secs() < CERT_SIGN_WINDOW_SECS);
        if entry.len() >= CERT_SIGN_MAX_ATTEMPTS {
            return false;
        }
        entry.push(now);
        true
    }

    fn resolve_keyfile_path(&self, keyfile_hint: Option<&str>) -> Result<std::path::PathBuf> {
        let base_dir = self
            .credentials_config
            .vault_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        match keyfile_hint {
            None => Ok(base_dir.join("vault.key")),
            Some(name)
                if !name.is_empty()
                    && !name.contains('/')
                    && !name.contains('\\')
                    && !name.contains("..") =>
            {
                Ok(base_dir.join(name))
            }
            Some(_) => Err(crate::error::GatewayError::InvalidRequest {
                message: "keyfile_path must be a simple filename relative to the vault directory"
                    .to_string(),
            }
            .into()),
        }
    }

    /// Initialize an agent's canonical document state.
    async fn initialize_agent_state(
        &self,
        agent_id: &str,
        system_prompt: Option<&str>,
    ) -> std::result::Result<(), std::io::Error> {
        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        rockbot_storage_runtime::StorageRuntime::new_with_root_sync(
            &Config::default(),
            storage_root,
        )
        .map_err(std::io::Error::other)?
        .initialize_agent_context(agent_id, system_prompt)
        .await
        .map(|_| ())
        .map_err(std::io::Error::other)
    }

    /// Validate a context filename — alphanumeric, hyphens, underscores, must end with .md
    fn is_valid_context_filename(name: &str) -> bool {
        rockbot_storage_runtime::is_valid_agent_context_filename(name)
    }

    /// Extract agent_id and filename from a path like /api/agents/{id}/files/{name}
    fn parse_agent_file_path(path: &str) -> Option<(&str, &str)> {
        let stripped = path.strip_prefix("/api/agents/")?;
        let (agent_id, rest) = stripped.split_once("/files/")?;
        if agent_id.is_empty() || rest.is_empty() || !Self::is_valid_agent_id(agent_id) {
            return None;
        }
        Some((agent_id, rest))
    }

    /// Extract agent_id from a path like /api/agents/{id}/files
    fn parse_agent_files_list_path(path: &str) -> Option<&str> {
        let stripped = path.strip_prefix("/api/agents/")?;
        let agent_id = stripped.strip_suffix("/files")?;
        if agent_id.is_empty() || !Self::is_valid_agent_id(agent_id) {
            return None;
        }
        Some(agent_id)
    }

    fn parse_agent_objects_list_path(path: &str) -> Option<&str> {
        let stripped = path.strip_prefix("/api/agents/")?;
        let agent_id = stripped.strip_suffix("/objects")?;
        if agent_id.is_empty() || !Self::is_valid_agent_id(agent_id) {
            return None;
        }
        Some(agent_id)
    }

    fn parse_agent_object_path(path: &str) -> Option<(&str, &str)> {
        let stripped = path.strip_prefix("/api/agents/")?;
        let (agent_id, object_id) = stripped.split_once("/objects/")?;
        if agent_id.is_empty() || object_id.is_empty() || !Self::is_valid_agent_id(agent_id) {
            return None;
        }
        Some((agent_id, object_id))
    }

    async fn handle_get_topology(&self) -> Response<GatewayBody> {
        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        let runtime = match rockbot_storage_runtime::StorageRuntime::new_with_root_sync(
            &Config::default(),
            storage_root,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to open topology runtime: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };
        let nodes = match runtime.list_topology_nodes().await {
            Ok(nodes) => nodes,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to load topology nodes: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };
        let edges = match runtime.list_topology_edges().await {
            Ok(edges) => edges,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to load topology edges: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };
        let zones = match runtime.list_zones().await {
            Ok(zones) => zones,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to load zones: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };

        let body = serde_json::json!({
            "nodes": nodes,
            "edges": edges,
            "zones": zones,
        })
        .to_string();
        Self::json_response(&body, StatusCode::OK)
    }

    async fn handle_list_agent_objects(&self, path: &str) -> Response<GatewayBody> {
        let Some(agent_id) = Self::parse_agent_objects_list_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };
        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        let runtime = match rockbot_storage_runtime::StorageRuntime::new_with_root_sync(
            &Config::default(),
            storage_root,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to open agent object store: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };
        match runtime.list_agent_objects(agent_id).await {
            Ok(objects) => Self::json_response(
                &serde_json::to_string(&objects).unwrap_or_default(),
                StatusCode::OK,
            ),
            Err(err) => Self::json_error(
                &format!("Failed to list agent objects: {err}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }

    async fn handle_put_agent_object<B>(&self, path: &str, req: Request<B>) -> Response<GatewayBody>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let Some((agent_id, object_id)) = Self::parse_agent_object_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };

        #[derive(Deserialize)]
        struct UpdateObjectRequest {
            content_type: Option<String>,
            size_bytes: Option<u64>,
            hash: Option<String>,
            replication_class: Option<rockbot_storage_runtime::ReplicationClass>,
            promoted_for_replication: Option<bool>,
            last_replicated_at: Option<String>,
        }

        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return resp,
        };
        let payload: UpdateObjectRequest = match serde_json::from_slice(&body) {
            Ok(payload) => payload,
            Err(err) => {
                return Self::json_error(
                    &format!("Invalid JSON: {err}"),
                    StatusCode::BAD_REQUEST,
                )
            }
        };

        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        let runtime = match rockbot_storage_runtime::StorageRuntime::new_with_root_sync(
            &Config::default(),
            storage_root,
        ) {
            Ok(runtime) => runtime,
            Err(err) => {
                return Self::json_error(
                    &format!("Failed to open agent object store: {err}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        };
        let existing = runtime
            .list_agent_objects(agent_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|record| record.object_id == object_id);
        let record = rockbot_storage_runtime::AgentObjectRecord {
            object_id: object_id.to_string(),
            content_type: payload
                .content_type
                .or_else(|| existing.as_ref().map(|record| record.content_type.clone()))
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            size_bytes: payload
                .size_bytes
                .or_else(|| existing.as_ref().map(|record| record.size_bytes))
                .unwrap_or(0),
            hash: payload
                .hash
                .or_else(|| existing.as_ref().map(|record| record.hash.clone()))
                .unwrap_or_default(),
            replication_class: payload
                .replication_class
                .or_else(|| existing.as_ref().map(|record| record.replication_class.clone()))
                .unwrap_or(rockbot_storage_runtime::ReplicationClass::ManualPromote),
            promoted_for_replication: payload
                .promoted_for_replication
                .or_else(|| existing.as_ref().map(|record| record.promoted_for_replication))
                .unwrap_or(false),
            last_replicated_at: payload
                .last_replicated_at
                .or_else(|| existing.and_then(|record| record.last_replicated_at)),
        };
        match runtime.upsert_agent_object(agent_id, &record).await {
            Ok(()) => Self::json_response(
                &serde_json::json!({"status":"updated","object_id": object_id}).to_string(),
                StatusCode::OK,
            ),
            Err(err) => Self::json_error(
                &format!("Failed to update object metadata: {err}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }

    /// List context files for an agent
    async fn handle_list_agent_files(&self, path: &str) -> Response<GatewayBody> {
        let Some(agent_id) = Self::parse_agent_files_list_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };

        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        let files = match rockbot_storage_runtime::list_agent_context_files(&storage_root, agent_id).await {
            Ok(files) => files,
            Err(_) => return Self::json_error("Invalid agent id", StatusCode::BAD_REQUEST),
        };
        let files: Vec<_> = files
            .into_iter()
            .map(|file| {
                serde_json::json!({
                    "name": file.name,
                    "exists": file.exists,
                    "size_bytes": file.size_bytes,
                    "well_known": file.well_known,
                })
            })
            .collect();

        let body = serde_json::to_string(&files).unwrap_or_default();
        Self::json_response(&body, StatusCode::OK)
    }

    /// Get the content of a single context file
    async fn handle_get_agent_file(&self, path: &str) -> Response<GatewayBody> {
        let Some((agent_id, filename)) = Self::parse_agent_file_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };
        if !Self::is_valid_context_filename(filename) {
            return Self::json_error("Invalid filename", StatusCode::BAD_REQUEST);
        }

        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        match rockbot_storage_runtime::read_agent_context_file(&storage_root, agent_id, filename).await {
            Ok(content) => {
                let body = serde_json::json!({ "name": filename, "content": content }).to_string();
                Self::json_response(&body, StatusCode::OK)
            }
            Err(e) if e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                Self::json_error(
                &format!("File '{filename}' not found"),
                StatusCode::NOT_FOUND,
            )}
            Err(e) => Self::json_error(
                &format!("Failed to read file: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }

    /// Create or update a context file
    async fn handle_put_agent_file<B>(&self, path: &str, req: Request<B>) -> Response<GatewayBody>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let Some((agent_id, filename)) = Self::parse_agent_file_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };
        if !Self::is_valid_context_filename(filename) {
            return Self::json_error("Invalid filename", StatusCode::BAD_REQUEST);
        }

        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return resp,
        };
        let payload: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)
            }
        };
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        match rockbot_storage_runtime::write_agent_context_file(&storage_root, agent_id, filename, content).await {
            Ok(()) => {
                let resp = serde_json::json!({ "written": true, "name": filename, "size_bytes": content.len() }).to_string();
                Self::json_response(&resp, StatusCode::OK)
            }
            Err(e) => Self::json_error(
                &format!("Failed to write file: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }

    /// Delete a context file (rejects deletion of SOUL.md)
    async fn handle_delete_agent_file(&self, path: &str) -> Response<GatewayBody> {
        let Some((agent_id, filename)) = Self::parse_agent_file_path(path) else {
            return Self::json_error("Invalid path", StatusCode::BAD_REQUEST);
        };
        if !Self::is_valid_context_filename(filename) {
            return Self::json_error("Invalid filename", StatusCode::BAD_REQUEST);
        }
        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        match rockbot_storage_runtime::delete_agent_context_file(&storage_root, agent_id, filename).await {
            Ok(()) => {
                let resp = serde_json::json!({ "deleted": true, "name": filename }).to_string();
                Self::json_response(&resp, StatusCode::OK)
            }
            Err(e) if e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
            {
                Self::json_error(
                &format!("File '{filename}' not found"),
                StatusCode::NOT_FOUND,
            )}
            Err(e) => Self::json_error(
                &format!("Failed to delete file: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
        }
    }

    /// Handle WebSocket upgrade request
    #[allow(clippy::expect_used)] // Response::builder() only fails on invalid headers
    async fn handle_websocket_upgrade(
        &self,
        req: Request<IncomingBody>,
        listener_kind: ListenerKind,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        // Validate upgrade headers
        let upgrade_hdr = req
            .headers()
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !upgrade_hdr.eq_ignore_ascii_case("websocket") {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new(
                    "Missing Upgrade: websocket header".into(),
                )))
                .unwrap_or_else(|e| {
                    error!("Failed to build websocket error response: {}", e);
                    Self::json_error(
                        "Failed to build response",
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                }));
        }
        let ws_key = match req.headers().get("sec-websocket-key") {
            Some(k) => k.to_str().unwrap_or("").to_string(),
            None => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new(
                        "Missing Sec-WebSocket-Key".into(),
                    )))
                    .unwrap_or_else(|e| {
                        error!("Failed to build websocket error response: {}", e);
                        Self::json_error(
                            "Failed to build response",
                            StatusCode::INTERNAL_SERVER_ERROR,
                        )
                    }));
            }
        };

        let accept_key = tungstenite::handshake::derive_accept_key(ws_key.as_bytes());
        let conn_id = uuid::Uuid::new_v4().to_string();

        // Spawn task to handle the upgraded connection
        let gateway = self.clone();
        let conn_id_clone = conn_id.clone();
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    let io = TokioIo::new(upgraded);
                    let ws_stream = tokio_tungstenite::WebSocketStream::from_raw_socket(
                        io,
                        tokio_tungstenite::tungstenite::protocol::Role::Server,
                        None,
                    )
                    .await;

                    info!("WebSocket connection established: {}", conn_id_clone);
                    gateway
                        .handle_websocket_connection(ws_stream, conn_id_clone, listener_kind)
                        .await;
                }
                Err(e) => {
                    error!("WebSocket upgrade failed: {}", e);
                }
            }
        });

        // Return 101 Switching Protocols
        Ok(Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Accept", accept_key)
            .body(GatewayBody::Left(Full::new(hyper::body::Bytes::new())))
            .unwrap_or_else(|e| {
                error!("Failed to build websocket upgrade response: {}", e);
                Self::json_error(
                    "Failed to build response",
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }))
    }

    /// Handle an active WebSocket connection (read/write loop)
    async fn handle_websocket_connection<S>(
        &self,
        ws_stream: tokio_tungstenite::WebSocketStream<S>,
        conn_id: String,
        listener_kind: ListenerKind,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        use futures_util::{SinkExt, StreamExt};

        let (mut ws_sink, mut ws_source) = ws_stream.split();
        let (outbound_tx, mut outbound_rx) =
            tokio::sync::mpsc::channel::<WsMessage>(MAX_WS_OUTBOUND_QUEUE);

        // Register connection
        {
            let mut conns = self.ws_connections.write().await;
            conns.insert(
                conn_id.clone(),
                WsConnection {
                    sender: outbound_tx.clone(),
                    identity: None,
                    listener_kind,
                    browser_auth: BrowserAuthState::default(),
                    connected_at: std::time::Instant::now(),
                },
            );
        }
        info!(
            "WebSocket registered: {} (total: {})",
            conn_id,
            self.ws_connections.read().await.len()
        );

        // Writer task: forward outbound messages to WebSocket sink (with write timeout)
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = outbound_rx.recv().await {
                match tokio::time::timeout(std::time::Duration::from_secs(15), ws_sink.send(msg))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) | Err(_) => break,
                }
            }
        });

        // Reader loop: process incoming WebSocket messages
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let inflight_ws_messages = Arc::new(tokio::sync::Semaphore::new(
            MAX_WS_INFLIGHT_MESSAGES_PER_CONNECTION,
        ));
        loop {
            tokio::select! {
                msg = ws_source.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            let gateway = self.clone();
                            let conn_id_clone = conn_id.clone();
                            let outbound_clone = outbound_tx.clone();
                            let permit = match inflight_ws_messages.clone().acquire_owned().await {
                                Ok(permit) => permit,
                                Err(_) => break,
                            };
                            tokio::spawn(async move {
                                let _permit = permit;
                                gateway
                                    .handle_ws_message(&conn_id_clone, &outbound_clone, &text)
                                    .await;
                            });
                        }
                        Some(Ok(WsMessage::Ping(data))) => {
                            let _ = enqueue_ws_message(&outbound_tx, WsMessage::Pong(data));
                        }
                        Some(Ok(WsMessage::Close(_))) | None => {
                            debug!("WebSocket closed: {}", conn_id);
                            break;
                        }
                        Some(Err(e)) => {
                            debug!("WebSocket error for {}: {}", conn_id, e);
                            break;
                        }
                        _ => {}
                    }
                }
                _ = shutdown_rx.recv() => {
                    let _ = enqueue_ws_message(&outbound_tx, WsMessage::Close(None));
                    break;
                }
            }
        }

        // Capture identity before removing so we can include it in the log
        let disconnect_host = self.client_hostname(&conn_id).await;
        let connected_for = {
            let conns = self.ws_connections.read().await;
            conns.get(&conn_id).map(|conn| conn.connected_at.elapsed())
        };

        // Cleanup
        {
            let mut conns = self.ws_connections.write().await;
            conns.remove(&conn_id);
        }
        #[cfg(feature = "remote-exec")]
        {
            self.remote_exec_registry.remove(&conn_id).await;
            if let Some(states) = NOISE_HANDSHAKE_STATES.get() {
                states.lock().await.remove(&conn_id);
            }
            if let Some(transports) = NOISE_TRANSPORT_STATES.get() {
                transports.lock().await.remove(&conn_id);
            }
        }
        writer_handle.abort();
        info!(
            "WebSocket disconnected: {} [{}] after {:.1?} (remaining: {})",
            conn_id,
            disconnect_host,
            connected_for.unwrap_or_default(),
            self.ws_connections.read().await.len()
        );
    }

    /// Return a human-readable identifier for a connected WebSocket client.
    ///
    /// Returns `"hostname"` or `"hostname(label)"` when the client has sent a
    /// `client_identify` message, or the first 8 characters of the connection
    /// ID as a fallback.
    async fn client_hostname(&self, conn_id: &str) -> String {
        self.ws_connections.read().await.get(conn_id).map_or_else(
            || conn_id[..8.min(conn_id.len())].to_string(),
            |conn| {
                if let Some(id) = conn.identity.as_ref() {
                    if let Some(ref label) = id.label {
                        format!("{}({})", id.hostname, label)
                    } else {
                        id.hostname.clone()
                    }
                } else if let Some(cert_name) = conn.browser_auth.cert_name.as_ref() {
                    format!("browser:{cert_name}")
                } else {
                    conn_id[..8.min(conn_id.len())].to_string()
                }
            },
        )
    }

    async fn ws_listener_kind(&self, conn_id: &str) -> Option<ListenerKind> {
        self.ws_connections
            .read()
            .await
            .get(conn_id)
            .map(|conn| conn.listener_kind)
    }

    async fn is_ws_authenticated(&self, conn_id: &str) -> bool {
        self.ws_connections
            .read()
            .await
            .get(conn_id)
            .is_some_and(|conn| match conn.listener_kind {
                ListenerKind::Client => conn.identity.is_some(),
                ListenerKind::Public => conn.browser_auth.authenticated,
            })
    }

    fn requires_ws_auth(msg: &WsMessageType) -> bool {
        !matches!(
            msg,
            WsMessageType::Ping
                | WsMessageType::HealthCheck
                | WsMessageType::WebAuthBegin { .. }
                | WsMessageType::WebAuthComplete { .. }
                | WsMessageType::ClientIdentify { .. }
        )
    }

    fn is_a2a_authorized(headers: &hyper::HeaderMap) -> bool {
        let Ok(expected) = std::env::var("ROCKBOT_A2A_TOKEN") else {
            return false;
        };

        let bearer = headers
            .get(hyper::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        let direct = headers
            .get("x-rockbot-a2a-token")
            .and_then(|v| v.to_str().ok());

        bearer
            .or(direct)
            .is_some_and(|provided| Self::constant_time_secret_eq(provided, &expected))
    }

    fn constant_time_secret_eq(provided: &str, expected: &str) -> bool {
        provided.as_bytes().ct_eq(expected.as_bytes()).into()
    }

    async fn begin_browser_web_auth(
        &self,
        conn_id: &str,
    ) -> anyhow::Result<(String, String, String)> {
        let cert_pem = {
            let conns = self.ws_connections.read().await;
            let conn = conns
                .get(conn_id)
                .ok_or_else(|| anyhow::anyhow!("WebSocket connection not found"))?;
            conn.browser_auth
                .pending_cert_pem
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No pending certificate for this connection"))?
        };

        let mut cursor = Cursor::new(cert_pem.as_bytes());
        let cert_der = rustls_pemfile::certs(&mut cursor)
            .next()
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("No PEM certificate found"))?;
        let fingerprint = rockbot_pki::sha256_fingerprint(cert_der.as_ref());

        let pki_dir = self
            .pki
            .pki_dir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Gateway PKI directory is not configured"))?;
        let mgr = PkiManager::new(pki_dir)?;
        let entry = mgr
            .list_clients()
            .into_iter()
            .find(|entry| {
                entry.status == rockbot_pki::CertStatus::Active
                    && entry.fingerprint_sha256 == fingerprint
            })
            .ok_or_else(|| anyhow::anyhow!("Certificate is not active in the local PKI index"))?;

        let rng = ring::rand::SystemRandom::new();
        let mut challenge = vec![0u8; 32];
        ring::rand::SecureRandom::fill(&rng, &mut challenge)
            .map_err(|_| anyhow::anyhow!("Failed to generate browser auth challenge"))?;
        let challenge_b64 = BASE64_STANDARD.encode(&challenge);
        let cert_name = entry.name.clone();
        let cert_role = entry.role.to_string();

        {
            let mut conns = self.ws_connections.write().await;
            let conn = conns
                .get_mut(conn_id)
                .ok_or_else(|| anyhow::anyhow!("WebSocket connection not found"))?;
            conn.browser_auth.pending_challenge = Some(challenge);
            conn.browser_auth.cert_name = Some(cert_name.clone());
            conn.browser_auth.cert_role = Some(cert_role.clone());
            conn.browser_auth.authenticated = false;
        }

        Ok((challenge_b64, cert_name, cert_role))
    }

    async fn complete_browser_web_auth(
        &self,
        conn_id: &str,
        signature_b64: &str,
    ) -> anyhow::Result<(String, String)> {
        let (cert_pem, challenge, cert_name, cert_role) = {
            let conns = self.ws_connections.read().await;
            let conn = conns
                .get(conn_id)
                .ok_or_else(|| anyhow::anyhow!("WebSocket connection not found"))?;
            (
                conn.browser_auth
                    .pending_cert_pem
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No pending certificate"))?,
                conn.browser_auth
                    .pending_challenge
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No pending challenge"))?,
                conn.browser_auth
                    .cert_name
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No pending certificate identity"))?,
                conn.browser_auth
                    .cert_role
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("No pending certificate role"))?,
            )
        };

        let mut cursor = Cursor::new(cert_pem.as_bytes());
        let cert_der = rustls_pemfile::certs(&mut cursor)
            .next()
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("No PEM certificate found"))?;
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der.as_ref())
            .map_err(|_| anyhow::anyhow!("Failed to parse certificate"))?;
        let public_key = cert.tbs_certificate.subject_pki.subject_public_key.data.to_vec();
        let spki = cert.tbs_certificate.subject_pki.raw.to_owned();
        let signature = BASE64_STANDARD
            .decode(signature_b64)
            .map_err(|e| anyhow::anyhow!("Invalid auth signature encoding: {e}"))?;

        let verify_attempts = [
            (
                &ring::signature::ECDSA_P256_SHA256_ASN1,
                public_key.as_slice(),
            ),
            (
                &ring::signature::ECDSA_P256_SHA256_FIXED,
                public_key.as_slice(),
            ),
            (&ring::signature::ECDSA_P256_SHA256_ASN1, spki.as_slice()),
            (&ring::signature::ECDSA_P256_SHA256_FIXED, spki.as_slice()),
        ];
        let verified = verify_attempts.iter().any(|(algorithm, candidate_key)| {
            ring::signature::UnparsedPublicKey::new(*algorithm, *candidate_key)
                .verify(&challenge, &signature)
                .is_ok()
        });
        if !verified {
            return Err(anyhow::anyhow!("Browser auth signature verification failed"));
        }

        {
            let mut conns = self.ws_connections.write().await;
            let conn = conns
                .get_mut(conn_id)
                .ok_or_else(|| anyhow::anyhow!("WebSocket connection not found"))?;
            conn.browser_auth.authenticated = true;
            conn.browser_auth.pending_challenge = None;
        }

        Ok((cert_name, cert_role))
    }

    /// Process a single incoming WebSocket message
    async fn handle_ws_message(
        &self,
        conn_id: &str,
        outbound_tx: &WsOutboundSender,
        text: &str,
    ) {
        let msg: WsMessageType = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                let resp = WsResponseType::Error {
                    message: format!("Invalid message: {e}"),
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
                return;
            }
        };

        let requires_ws_auth = Self::requires_ws_auth(&msg);
        if requires_ws_auth
            && self
                .ws_listener_kind(conn_id)
                .await
                .is_some_and(|kind| matches!(kind, ListenerKind::Public | ListenerKind::Client))
            && !self.is_ws_authenticated(conn_id).await
        {
            let resp = WsResponseType::Error {
                message: "This WebSocket connection is not authenticated yet".to_string(),
            };
            let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                serde_json::to_string(&resp).unwrap_or_default(),
            ));
            return;
        }

        match msg {
            WsMessageType::Ping => {
                let resp = WsResponseType::Pong;
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
            WsMessageType::HealthCheck => {
                let health = self.get_health_status().await;
                let resp = WsResponseType::HealthStatus { status: health };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
            WsMessageType::AgentMessage {
                agent_id,
                session_key,
                message,
                workspace,
                executor_target,
                allow_active_client_tools,
            } => {
                self.handle_ws_agent_message(
                    conn_id,
                    outbound_tx,
                    agent_id,
                    session_key,
                    message,
                    workspace,
                    executor_target,
                    allow_active_client_tools,
                )
                .await;
            }
            WsMessageType::ClientIdentify {
                client_uuid,
                hostname,
                label,
            } => {
                let previous_uuid = {
                    let conns = self.ws_connections.read().await;
                    conns
                        .get(conn_id)
                        .and_then(|conn| conn.identity.as_ref())
                        .map(|identity| identity.client_uuid.clone())
                };
                let uuid = previous_uuid.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                if client_uuid.as_ref().is_some_and(|claimed| claimed != &uuid) {
                    warn!(
                        "Ignoring self-reported client UUID '{}' for {} and using gateway-assigned '{}'",
                        client_uuid.as_deref().unwrap_or_default(),
                        conn_id,
                        uuid
                    );
                }
                let trusted_hostname = format!("client-{}", &uuid[..8.min(uuid.len())]);
                if !hostname.is_empty() || label.is_some() {
                    debug!(
                        "Ignoring self-reported client identity metadata for {}: hostname='{}', label={:?}",
                        conn_id, hostname, label
                    );
                }
                info!(
                    "WebSocket client {} identified: uuid={}, hostname='{}', label={:?}",
                    conn_id,
                    uuid,
                    trusted_hostname,
                    Option::<String>::None
                );
                let identity = ClientIdentity {
                    client_uuid: uuid.clone(),
                    hostname: trusted_hostname.clone(),
                    label: None,
                };
                {
                    let mut conns = self.ws_connections.write().await;
                    if let Some(conn) = conns.get_mut(conn_id) {
                        conn.identity = Some(identity);
                    }
                }
                // Confirm the identity back to the client so it can persist its UUID
                let response = WsResponseType::ClientIdentityAssigned {
                    client_uuid: uuid,
                    hostname: trusted_hostname,
                    label: None,
                };
                if let Ok(json) = serde_json::to_string(&response) {
                    let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(json));
                }
            }
            WsMessageType::CronResult {
                job_id,
                success,
                error,
                output,
            } => {
                info!(
                    "Cron result for job {}: success={}, output={:?}",
                    job_id,
                    success,
                    output.as_deref().unwrap_or("(none)")
                );
                if !success {
                    if let Some(ref e) = error {
                        error!("Remote cron job {} failed: {}", job_id, e);
                    }
                }
                // State is already updated by the CronExecutor before dispatch;
                // if we want to record the remote result, we'd update the job state here.
                // For now just log it — the scheduler handles its own state tracking.
            }
            #[cfg(feature = "remote-exec")]
            WsMessageType::NoiseHandshake { payload, step } => {
                self.handle_noise_handshake(conn_id, outbound_tx, &payload, step)
                    .await;
            }
            #[cfg(not(feature = "remote-exec"))]
            WsMessageType::NoiseHandshake { .. } => {
                let resp = WsResponseType::Error {
                    message: "Remote execution not enabled on this gateway".to_string(),
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
            WsMessageType::RemoteCapabilities {
                capabilities,
                client_type,
                working_dir,
            } => {
                #[cfg(feature = "remote-exec")]
                {
                    self.handle_remote_capabilities(
                        conn_id,
                        outbound_tx,
                        capabilities,
                        client_type,
                        working_dir,
                    )
                    .await;
                }
                #[cfg(not(feature = "remote-exec"))]
                {
                    let _ = (capabilities, client_type, working_dir);
                    let resp = WsResponseType::Error {
                        message: "Remote execution not enabled on this gateway".to_string(),
                    };
                    let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                        serde_json::to_string(&resp).unwrap_or_default(),
                    ));
                }
            }
            WsMessageType::RemoteToolResponse {
                request_id,
                success,
                output,
                execution_time_ms,
            } => {
                #[cfg(feature = "remote-exec")]
                {
                    let response = rockbot_client::remote_exec::RemoteToolResponse {
                        request_id,
                        success,
                        output,
                        execution_time_ms,
                    };
                    self.remote_exec_registry.deliver_response(response).await;
                }
                #[cfg(not(feature = "remote-exec"))]
                {
                    let _ = (request_id, success, output, execution_time_ms);
                    warn!("Received remote tool response but remote-exec feature is disabled");
                }
            }
            WsMessageType::RemoteToolOutput {
                request_id,
                output,
                stream,
            } => {
                #[cfg(feature = "remote-exec")]
                {
                    let output = rockbot_client::remote_exec::RemoteToolOutput {
                        request_id,
                        output,
                        stream,
                    };
                    self.remote_exec_registry.deliver_output(output).await;
                }
                #[cfg(not(feature = "remote-exec"))]
                {
                    let _ = (request_id, output, stream);
                    warn!("Received remote tool output but remote-exec feature is disabled");
                }
            }
            WsMessageType::ApiRequest {
                request_id,
                method,
                path,
                body,
            } => {
                self.handle_ws_api_request(outbound_tx, request_id, method, path, body)
                    .await;
            }
            WsMessageType::WebAuthBegin { certificate_pem } => {
                {
                    let mut conns = self.ws_connections.write().await;
                    if let Some(conn) = conns.get_mut(conn_id) {
                        conn.browser_auth.pending_cert_pem = Some(certificate_pem);
                        conn.browser_auth.pending_challenge = None;
                        conn.browser_auth.cert_name = None;
                        conn.browser_auth.cert_role = None;
                        conn.browser_auth.authenticated = false;
                    }
                }

                match self.begin_browser_web_auth(conn_id).await {
                    Ok((challenge, _, _)) => {
                        let resp = WsResponseType::WebAuthChallenge { challenge };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    }
                    Err(err) => {
                        let resp = WsResponseType::WebAuthResult {
                            authenticated: false,
                            message: err.to_string(),
                            cert_name: None,
                            cert_role: None,
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    }
                }
            }
            WsMessageType::WebAuthComplete { signature } => {
                match self.complete_browser_web_auth(conn_id, &signature).await {
                    Ok((cert_name, cert_role)) => {
                        let resp = WsResponseType::WebAuthResult {
                            authenticated: true,
                            message: "Browser WebSocket authenticated".to_string(),
                            cert_name: Some(cert_name),
                            cert_role: Some(cert_role),
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    }
                    Err(err) => {
                        if let Some(conn) = self.ws_connections.write().await.get_mut(conn_id) {
                            conn.browser_auth.authenticated = false;
                            conn.browser_auth.pending_challenge = None;
                            conn.browser_auth.cert_name = None;
                            conn.browser_auth.cert_role = None;
                        }
                        let resp = WsResponseType::WebAuthResult {
                            authenticated: false,
                            message: err.to_string(),
                            cert_name: None,
                            cert_role: None,
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }

    async fn handle_ws_api_request(
        &self,
        outbound_tx: &WsOutboundSender,
        request_id: String,
        method: String,
        path: String,
        body: Option<serde_json::Value>,
    ) {
        let response = match self.dispatch_ws_api_request(&method, &path, body).await {
            Ok(resp) => resp,
            Err(message) => Self::json_error(&message, StatusCode::BAD_REQUEST),
        };

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = match response.into_body().collect().await {
            Ok(collected) => String::from_utf8_lossy(&collected.to_bytes()).to_string(),
            Err(e) => serde_json::json!({ "error": format!("Failed to read response body: {e}") })
                .to_string(),
        };

        let response = WsResponseType::ApiResponse {
            request_id,
            status,
            body,
            content_type,
        };
        if let Ok(json) = serde_json::to_string(&response) {
            let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(json));
        }
    }

    async fn dispatch_ws_api_request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> std::result::Result<Response<GatewayBody>, String> {
        let method = Method::from_bytes(method.as_bytes())
            .map_err(|e| format!("Invalid method '{method}': {e}"))?;
        let method_label = method.as_str().to_string();
        let uri: hyper::Uri = path
            .parse()
            .map_err(|e| format!("Invalid API path '{path}': {e}"))?;
        let body_bytes = match body {
            Some(value) => serde_json::to_vec(&value)
                .map_err(|e| format!("Failed to encode API request body: {e}"))?,
            None => Vec::new(),
        };
        if body_bytes.len() > MAX_WS_API_BODY_BYTES {
            return Err(format!(
                "API request body too large (max {MAX_WS_API_BODY_BYTES} bytes)"
            ));
        }
        let req = Request::builder()
            .method(method.clone())
            .uri(uri)
            .body(Full::new(hyper::body::Bytes::from(body_bytes)))
            .map_err(|e| format!("Failed to build API request: {e}"))?;

        match (method, path) {
            (Method::GET, "/api/status") => {
                self.handle_health_check().await.map_err(|e| e.to_string())
            }
            (Method::GET, "/api/providers") => self
                .handle_list_providers()
                .await
                .map_err(|e| e.to_string()),
            (Method::POST, p) if p.starts_with("/api/providers/") && p.ends_with("/test") => self
                .handle_test_provider(path)
                .await
                .map_err(|e| e.to_string()),
            (Method::GET, "/api/agents") => {
                self.handle_list_agents().await.map_err(|e| e.to_string())
            }
            (Method::GET, "/api/topology") => Ok(self.handle_get_topology().await),
            (Method::POST, "/api/agents") => self
                .handle_create_agent(req)
                .await
                .map_err(|e| e.to_string()),
            (Method::PUT, p)
                if p.starts_with("/api/agents/")
                    && !p.contains("/files/")
                    && !p.contains("/objects/") =>
            {
                self
                .handle_update_agent(req)
                .await
                .map_err(|e| e.to_string())
            }
            (Method::GET, p) if p.starts_with("/api/agents/") && p.ends_with("/files") => {
                Ok(self.handle_list_agent_files(path).await)
            }
            (Method::GET, p) if p.starts_with("/api/agents/") && p.ends_with("/objects") => {
                Ok(self.handle_list_agent_objects(path).await)
            }
            (Method::GET, p) if p.starts_with("/api/agents/") && p.contains("/files/") => {
                Ok(self.handle_get_agent_file(path).await)
            }
            (Method::PUT, p) if p.starts_with("/api/agents/") && p.contains("/files/") => {
                Ok(self.handle_put_agent_file(path, req).await)
            }
            (Method::PUT, p) if p.starts_with("/api/agents/") && p.contains("/objects/") => {
                Ok(self.handle_put_agent_object(path, req).await)
            }
            (Method::DELETE, p) if p.starts_with("/api/agents/") && p.contains("/files/") => {
                Ok(self.handle_delete_agent_file(path).await)
            }
            (Method::GET, "/api/credentials/schemas") => self
                .handle_credential_schemas()
                .await
                .map_err(|e| e.to_string()),
            (Method::POST, "/api/credentials/endpoints") => self
                .handle_create_endpoint(req)
                .await
                .map_err(|e| e.to_string()),
            (Method::POST, p)
                if p.starts_with("/api/credentials/endpoints/") && p.ends_with("/credential") =>
            {
                self.handle_store_credential(req, path)
                    .await
                    .map_err(|e| e.to_string())
            }
            (Method::GET, p) if p == "/api/sessions" || p.starts_with("/api/sessions?") => self
                .handle_list_sessions(req)
                .await
                .map_err(|e| e.to_string()),
            (Method::POST, "/api/sessions") => self
                .handle_create_session(req)
                .await
                .map_err(|e| e.to_string()),
            (Method::DELETE, p) if p.starts_with("/api/sessions/") => self
                .handle_delete_session(path)
                .await
                .map_err(|e| e.to_string()),
            (Method::GET, p) if p.starts_with("/api/sessions/") && p.ends_with("/messages") => self
                .handle_get_session_messages(path)
                .await
                .map_err(|e| e.to_string()),
            (Method::GET, "/api/cron/jobs") => self
                .handle_list_cron_jobs()
                .await
                .map_err(|e| e.to_string()),
            (Method::PUT, p) if p.starts_with("/api/cron/jobs/") => self
                .handle_update_cron_job(req, path)
                .await
                .map_err(|e| e.to_string()),
            (Method::DELETE, p) if p.starts_with("/api/cron/jobs/") => self
                .handle_delete_cron_job(path)
                .await
                .map_err(|e| e.to_string()),
            (Method::POST, p) if p.starts_with("/api/cron/jobs/") && p.ends_with("/trigger") => {
                self.handle_trigger_cron_job(path)
                    .await
                    .map_err(|e| e.to_string())
            }
            (Method::GET, "/api/executors") => self
                .handle_list_executors()
                .await
                .map_err(|e| e.to_string()),
            _ => Ok(Self::json_error(
                &format!("Unsupported WS API route: {method_label} {path}"),
                StatusCode::NOT_FOUND,
            )),
        }
    }

    /// Handle an agent message received over WebSocket.
    ///
    /// Runs the agent through the proven non-streaming `process_message` path
    /// and sends the result back over the WebSocket. This avoids issues with
    /// providers that don't fully support streaming (e.g. some Bedrock models).
    #[allow(clippy::too_many_arguments)]
    async fn handle_ws_agent_message(
        &self,
        conn_id: &str,
        outbound_tx: &WsOutboundSender,
        agent_id: String,
        session_key: String,
        user_message: String,
        workspace: Option<String>,
        executor_target: Option<String>,
        allow_active_client_tools: Option<bool>,
    ) {
        // Intercept /overseer commands before agent processing
        #[cfg(feature = "overseer")]
        if user_message.trim().starts_with("/overseer") {
            let output = if let Some(ref overseer) = self.overseer {
                match overseer.dispatch_command(&user_message) {
                    rockbot_overseer::CommandResult::Handled(out) => out,
                    rockbot_overseer::CommandResult::NotHandled => String::new(),
                }
            } else if let Some(ref err) = self.overseer_init_error {
                format!(
                    "## Overseer\n\n\
                     The overseer is configured but failed to initialize:\n\n\
                     ```\n{err}\n```\n\n\
                     Check the gateway logs for details and restart after fixing the issue."
                )
            } else {
                "## Overseer\n\n\
                 The overseer feature is compiled in but not configured.\n\n\
                 Add an `[overseer]` section to your config file to enable it:\n\n\
                 ```toml\n\
                 [overseer]\n\
                 # Uses defaults (Qwen2.5-1.5B-Instruct, advisory mode)\n\
                 ```\n\n\
                 Or configure specific options:\n\n\
                 ```toml\n\
                 [overseer]\n\
                 model_id = \"Qwen/Qwen2.5-1.5B-Instruct-GGUF\"\n\
                 enforce = false\n\
                 ```"
                .to_string()
            };
            if !output.is_empty() {
                let resp = WsResponseType::AgentResponseMsg {
                    session_key,
                    content: output,
                    tool_calls: vec![],
                    tokens_used: None,
                    processing_time_ms: None,
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
                return;
            }
        }

        // Intercept /butler commands before agent processing
        #[cfg(feature = "butler")]
        if user_message.trim().starts_with("/butler") {
            if let Some(ref butler) = self.butler {
                match butler.dispatch_command(user_message.trim()) {
                    rockbot_butler::CommandResult::Handled(output) => {
                        let resp = WsResponseType::AgentResponseMsg {
                            session_key,
                            content: output,
                            tool_calls: vec![],
                            tokens_used: None,
                            processing_time_ms: None,
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                        return;
                    }
                    rockbot_butler::CommandResult::NotHandled => {}
                }
            }
        }

        // Intercept gateway slash commands before agent processing
        if let Some(output) = self.handle_slash_commands(&user_message).await {
            let resp = WsResponseType::AgentResponseMsg {
                session_key,
                content: output,
                tool_calls: vec![],
                tokens_used: None,
                processing_time_ms: None,
            };
            let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                serde_json::to_string(&resp).unwrap_or_default(),
            ));
            return;
        }

        // Look up agent
        let agents = self.agents.read().await;
        let agent = match agents.get(&agent_id) {
            Some(a) => Arc::clone(a),
            None => {
                let resp = WsResponseType::AgentError {
                    session_key,
                    error: format!("Agent '{agent_id}' not found"),
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
                return;
            }
        };
        drop(agents);

        let client_host = self.client_hostname(conn_id).await;
        info!("Agent message from {client_host}: agent={agent_id}, session={session_key}");

        let session_id = format!("{agent_id}:{session_key}");
        let tx = outbound_tx.clone();
        let sk = session_key.clone();

        // Build the domain Message
        let message = Message::text(user_message)
            .with_session_id(&session_id)
            .with_role(MessageRole::User);
        // Use the client-provided workspace only if it exists on this machine.
        // When a remote TUI sends its local cwd, that path won't exist here —
        // fall back to the agent's configured workspace instead.
        let workspace_path = workspace
            .clone()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists());
        let requesting_identity = self
            .ws_connections
            .read()
            .await
            .get(conn_id)
            .and_then(|conn| conn.identity.as_ref())
            .cloned();
        let requesting_client_uuid = requesting_identity
            .as_ref()
            .map(|identity| identity.client_uuid.clone());
        let resolved_executor_target = if executor_target.as_deref() == Some("gateway") {
            None
        } else if let Some(target) = executor_target.clone() {
            Some(target)
        } else if allow_active_client_tools.unwrap_or(true) {
            requesting_client_uuid
                .clone()
                .or_else(|| Some(conn_id.to_string()))
        } else {
            None
        };
        let strict_executor_target = executor_target.is_some();
        let is_active_client_target = resolved_executor_target.as_ref().is_some_and(|target| {
            target == conn_id
                || requesting_identity
                    .as_ref()
                    .map(|identity| identity.client_uuid.as_str())
                    .is_some_and(|client_uuid| client_uuid == target)
        });
        let remote_workspace_override = if is_active_client_target {
            workspace.clone()
        } else {
            None
        };

        // Create a progress channel to send real-time updates to the client
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let progress_ws_tx = tx.clone();
        let progress_sk = sk.clone();
        let progress_handle = tokio::spawn(async move {
            while let Some(event) = progress_rx.recv().await {
                let messages: Vec<WsResponseType> = match event {
                    rockbot_agent::agent::AgentProgressEvent::ToolStart {
                        ref tool_name,
                        ref locality,
                    } => {
                        vec![
                            WsResponseType::ToolCall {
                                session_key: progress_sk.clone(),
                                tool_name: tool_name.clone(),
                                arguments: String::new(),
                                locality: Some(tool_locality_label(locality)),
                            },
                            WsResponseType::ThinkingStatus {
                                session_key: progress_sk.clone(),
                                phase: "tool".to_string(),
                                tool_name: Some(tool_name.clone()),
                                iteration: None,
                            },
                        ]
                    }
                    rockbot_agent::agent::AgentProgressEvent::ToolDone {
                        ref tool_name,
                        ref result_preview,
                        success,
                        duration_ms,
                        ref locality,
                    } => {
                        vec![WsResponseType::ToolResult {
                            session_key: progress_sk.clone(),
                            tool_name: tool_name.clone(),
                            result: result_preview
                                .clone()
                                .map(|preview| truncate_utf8(&preview, MAX_TOOL_OUTPUT_CHARS))
                                .unwrap_or_default(),
                            success,
                            duration_ms,
                            locality: Some(tool_locality_label(locality)),
                        }]
                    }
                    rockbot_agent::agent::AgentProgressEvent::ToolOutput {
                        ref tool_name,
                        ref output,
                        ref locality,
                        ..
                    } => split_utf8_chunks(output, MAX_TOOL_OUTPUT_CHARS)
                        .into_iter()
                        .map(|chunk| WsResponseType::ToolOutput {
                            session_key: progress_sk.clone(),
                            tool_name: tool_name.clone(),
                            output: chunk,
                            locality: Some(tool_locality_label(locality)),
                        })
                        .collect(),
                    rockbot_agent::agent::AgentProgressEvent::TextDelta { ref text } => {
                        // Stream the model's actual text/reasoning to the client
                        vec![WsResponseType::StreamChunk {
                            session_key: progress_sk.clone(),
                            delta: text.clone(),
                        }]
                    }
                    rockbot_agent::agent::AgentProgressEvent::TokenUsage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                        cumulative_total,
                    } => {
                        // Send structured token usage (not embedded in chat text)
                        vec![WsResponseType::TokenUsageMsg {
                            session_key: progress_sk.clone(),
                            prompt_tokens,
                            completion_tokens,
                            total_tokens,
                            cumulative_total,
                        }]
                    }
                    rockbot_agent::agent::AgentProgressEvent::LlmCall {
                        iteration,
                        message_count: _,
                    } => {
                        vec![WsResponseType::ThinkingStatus {
                            session_key: progress_sk.clone(),
                            phase: "llm".to_string(),
                            tool_name: None,
                            iteration: Some(iteration),
                        }]
                    }
                    rockbot_agent::agent::AgentProgressEvent::Handoff {
                        ref from_agent,
                        ref to_agent,
                        ref context_preview,
                    } => {
                        vec![WsResponseType::StreamChunk {
                            session_key: progress_sk.clone(),
                            delta: format!("\n**[{from_agent} → {to_agent}]** {context_preview}\n"),
                        }]
                    }
                };
                for resp in messages {
                    let json = serde_json::to_string(&resp).unwrap_or_default();
                    if !enqueue_ws_message(&progress_ws_tx, WsMessage::Text(json)) {
                        break;
                    }
                }
            }
        });

        // Run the agent with progress reporting
        let mut result = agent
            .process_message_with_progress(
                session_id.clone(),
                message,
                ProcessMessageOptions {
                    workspace_override: workspace_path,
                    remote_executor_target: resolved_executor_target.clone(),
                    remote_executor_strict: strict_executor_target,
                    remote_workspace_override,
                    delegation_depth: 0,
                },
                progress_tx,
            )
            .await;

        // Progress channel is closed when agent completes; clean up the forwarder
        progress_handle.abort();

        // Handle handoff chain — follow handoffs through to the final agent
        let mut handoff_depth = 0u32;
        while let Ok(ref response) = result {
            if let Some(ref handoff) = response.handoff {
                handoff_depth += 1;
                if handoff_depth > 5 {
                    warn!("Handoff chain depth exceeded in WS handler");
                    break;
                }

                // Notify the client about the handoff
                let chunk = WsResponseType::StreamChunk {
                    session_key: sk.clone(),
                    delta: format!(
                        "\n**[Handing off to agent '{}']**\n",
                        handoff.target_agent_id
                    ),
                };
                let _ = enqueue_ws_message(&tx, WsMessage::Text(
                    serde_json::to_string(&chunk).unwrap_or_default(),
                ));

                // Look up target agent and invoke it
                let agents = self.agents.read().await;
                let target = agents.get(&handoff.target_agent_id).cloned();
                drop(agents);

                if let Some(target_agent) = target {
                    let target_message = if let Some(ref override_msg) = handoff.message_override {
                        override_msg.clone()
                    } else {
                        format!(
                            "Context from previous agent:\n{}\n\nOriginal user request was forwarded to you.",
                            handoff.context
                        )
                    };
                    let msg = rockbot_config::message::Message::text(target_message)
                        .with_session_id(&session_id)
                        .with_role(rockbot_config::message::MessageRole::User);

                    result = target_agent
                        .process_message(
                            session_id.clone(),
                            msg,
                            ProcessMessageOptions {
                                workspace_override: None,
                                remote_executor_target: resolved_executor_target.clone(),
                                remote_executor_strict: strict_executor_target,
                                remote_workspace_override: workspace.clone(),
                                delegation_depth: handoff_depth,
                            },
                        )
                        .await;
                } else {
                    warn!("Handoff target '{}' not found", handoff.target_agent_id);
                    break;
                }
            } else {
                break;
            }
        }

        // Send final response or error over WebSocket
        match result {
            Ok(response) => {
                let tool_calls: Vec<WsToolCallInfo> = response
                    .tool_results
                    .iter()
                    .map(|tr| {
                        let raw_result = match &tr.result {
                            rockbot_tools::message::ToolResult::Text { content } => content.clone(),
                            rockbot_tools::message::ToolResult::Error { message, .. } => {
                                format!("Error: {message}")
                            }
                            rockbot_tools::message::ToolResult::Json { data } => {
                                serde_json::to_string(data).unwrap_or_default()
                            }
                            rockbot_tools::message::ToolResult::File { path, .. } => {
                                format!("[File: {path}]")
                            }
                            rockbot_tools::message::ToolResult::Handoff {
                                target_agent_id, ..
                            } => {
                                format!("[Handoff to {target_agent_id}]")
                            }
                        };
                        // Cap tool results to avoid sending megabytes over WebSocket
                        let result = truncate_utf8(&raw_result, 2000);
                        WsToolCallInfo {
                            tool_name: tr.tool_name.clone(),
                            result,
                            success: tr.success,
                            duration_ms: tr.execution_time_ms,
                            locality: None,
                        }
                    })
                    .collect();

                let content = response.message.extract_text().unwrap_or_default();
                let tokens = if response.tokens_used.total_tokens > 0 {
                    Some(WsTokenUsage {
                        prompt_tokens: response.tokens_used.prompt_tokens,
                        completion_tokens: response.tokens_used.completion_tokens,
                        total_tokens: response.tokens_used.total_tokens,
                    })
                } else {
                    None
                };
                let resp = WsResponseType::AgentResponseMsg {
                    session_key: sk,
                    content,
                    tool_calls,
                    tokens_used: tokens,
                    processing_time_ms: Some(response.processing_time_ms),
                };
                let _ = enqueue_ws_message(&tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
            Err(e) => {
                let resp = WsResponseType::AgentError {
                    session_key: sk,
                    error: e.to_string(),
                };
                let _ = enqueue_ws_message(&tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
        }
    }

    /// Handle a Noise Protocol handshake message from a client.
    ///
    /// The XX pattern has 3 messages: client->server (step 1), server->client (step 2),
    /// client->server (step 3). After step 3 both sides have an encrypted transport.
    #[cfg(feature = "remote-exec")]
    async fn handle_noise_handshake(
        &self,
        conn_id: &str,
        outbound_tx: &WsOutboundSender,
        payload_b64: &str,
        step: u8,
    ) {
        use rockbot_client::remote_exec;

        // Decode the incoming handshake payload
        let payload = match BASE64_STANDARD.decode(payload_b64) {
            Ok(p) => p,
            Err(e) => {
                let resp = WsResponseType::Error {
                    message: format!("Invalid base64 in Noise handshake: {e}"),
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
                return;
            }
        };

        // For step 1: create a new responder, read msg 1, write msg 2
        // For step 3: read msg 3, complete handshake
        // We store in-progress handshake states keyed by conn_id in a static map
        // and explicitly clear them when the websocket disconnects.
        let states = NOISE_HANDSHAKE_STATES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));

        let mut states_lock = states.lock().await;

        match step {
            1 => {
                // Create a new responder and process the first message
                let mut responder = match remote_exec::create_responder(&self.noise_keypair) {
                    Ok(r) => r,
                    Err(e) => {
                        let resp = WsResponseType::Error {
                            message: format!("Failed to create Noise responder: {e}"),
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                        return;
                    }
                };

                let mut buf = vec![0u8; 65535];
                if let Err(e) = responder.read_message(&payload, &mut buf) {
                    let resp = WsResponseType::Error {
                        message: format!("Noise handshake step 1 failed: {e}"),
                    };
                    let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                        serde_json::to_string(&resp).unwrap_or_default(),
                    ));
                    return;
                }

                // Write message 2 (server -> client)
                match responder.write_message(&[], &mut buf) {
                    Ok(len) => {
                        let response_b64 = BASE64_STANDARD.encode(&buf[..len]);
                        let resp = WsResponseType::NoiseHandshake {
                            payload: response_b64,
                            step: 2,
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                        // Store the in-progress handshake
                        states_lock.insert(conn_id.to_string(), responder);
                        info!("Noise handshake step 1+2 complete for {conn_id}");
                    }
                    Err(e) => {
                        let resp = WsResponseType::Error {
                            message: format!("Noise handshake step 2 write failed: {e}"),
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    }
                }
            }
            3 => {
                // Read the final handshake message
                if let Some(mut responder) = states_lock.remove(conn_id) {
                    let mut buf = vec![0u8; 65535];
                    if let Err(e) = responder.read_message(&payload, &mut buf) {
                        let resp = WsResponseType::Error {
                            message: format!("Noise handshake step 3 failed: {e}"),
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                        return;
                    }

                    if responder.is_handshake_finished() {
                        info!("Noise handshake complete for {conn_id} — awaiting capability advertisement");
                        // Transport state is ready; we store it temporarily until capabilities arrive.
                        // We re-insert with a sentinel key to signal "handshake done, awaiting caps".
                        // The transport will be consumed when RemoteCapabilities arrives.
                        let transport = match responder.into_transport_mode() {
                            Ok(t) => t,
                            Err(e) => {
                                let resp = WsResponseType::Error {
                                    message: format!("Noise transport init failed: {e}"),
                                };
                                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                                    serde_json::to_string(&resp).unwrap_or_default(),
                                ));
                                return;
                            }
                        };

                        // Store the transport in the shared module-level map
                        let transports = NOISE_TRANSPORT_STATES
                            .get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
                        transports
                            .lock()
                            .await
                            .insert(conn_id.to_string(), transport);

                        // Acknowledge the handshake completion
                        let resp = WsResponseType::RemoteCapabilitiesAck {
                            accepted: true,
                            message:
                                "Noise handshake complete. Send remote_capabilities to register."
                                    .to_string(),
                        };
                        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                            serde_json::to_string(&resp).unwrap_or_default(),
                        ));
                    } else {
                        warn!("Noise handshake not finished after step 3 for {conn_id}");
                    }
                } else {
                    let resp = WsResponseType::Error {
                        message: "No pending Noise handshake for this connection".to_string(),
                    };
                    let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                        serde_json::to_string(&resp).unwrap_or_default(),
                    ));
                }
            }
            _ => {
                let resp = WsResponseType::Error {
                    message: format!("Invalid Noise handshake step: {step} (expected 1 or 3)"),
                };
                let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                    serde_json::to_string(&resp).unwrap_or_default(),
                ));
            }
        }
    }

    /// Handle capability advertisement from a remote client.
    ///
    /// Called after the Noise handshake completes. Creates a `NoiseSession` and
    /// registers it in the `RemoteExecutorRegistry`.
    #[cfg(feature = "remote-exec")]
    async fn handle_remote_capabilities(
        &self,
        conn_id: &str,
        outbound_tx: &WsOutboundSender,
        capabilities: Vec<String>,
        client_type: String,
        working_dir: Option<String>,
    ) {
        use rockbot_client::remote_exec::{ClientCapabilities, NoiseSession, ToolCapability};

        // Parse capability strings into ToolCapability enums
        let mut cap_set = std::collections::HashSet::new();
        for cap_str in &capabilities {
            match cap_str.as_str() {
                "filesystem" => {
                    cap_set.insert(ToolCapability::Filesystem);
                }
                "shell" => {
                    cap_set.insert(ToolCapability::Shell);
                }
                "browser" => {
                    cap_set.insert(ToolCapability::Browser);
                }
                "network" => {
                    cap_set.insert(ToolCapability::Network);
                }
                "agent" => {
                    cap_set.insert(ToolCapability::Agent);
                }
                "memory" => {
                    cap_set.insert(ToolCapability::Memory);
                }
                "full" => {
                    cap_set.insert(ToolCapability::Full);
                }
                other => {
                    warn!("Unknown capability '{}' from client {}", other, conn_id);
                }
            }
        }

        // Retrieve the transport state from the handshake (shared module-level map)
        let transports =
            NOISE_TRANSPORT_STATES.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()));
        let transport = transports.lock().await.remove(conn_id);

        let Some(transport) = transport else {
            let resp = WsResponseType::Error {
                message: "No completed Noise handshake found. Complete handshake first."
                    .to_string(),
            };
            let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
                serde_json::to_string(&resp).unwrap_or_default(),
            ));
            return;
        };

        // Create a channel for sending tool requests to this client
        let (tool_tx, mut tool_rx) = tokio::sync::mpsc::unbounded_channel();

        let client_caps = ClientCapabilities {
            capabilities: cap_set,
            client_type: client_type.clone(),
            client_name: None,
            working_dir,
        };

        let identity = self
            .ws_connections
            .read()
            .await
            .get(conn_id)
            .and_then(|conn| conn.identity.as_ref())
            .map(|id| rockbot_client::remote_exec::ExecutorIdentity {
                conn_id: conn_id.to_string(),
                client_uuid: Some(id.client_uuid.clone()),
                hostname: Some(id.hostname.clone()),
                label: id.label.clone(),
            })
            .unwrap_or_else(|| rockbot_client::remote_exec::ExecutorIdentity {
                conn_id: conn_id.to_string(),
                client_uuid: None,
                hostname: None,
                label: None,
            });

        let session = NoiseSession {
            identity,
            conn_id: conn_id.to_string(),
            transport,
            capabilities: client_caps,
            tool_tx,
        };

        self.remote_exec_registry.register(session).await;

        // Spawn a task that forwards tool requests from the registry to the WS connection
        let outbound_clone = outbound_tx.clone();
        let conn_id_clone = conn_id.to_string();
        tokio::spawn(async move {
            while let Some(request) = tool_rx.recv().await {
                let msg = WsResponseType::RemoteToolRequest {
                    request_id: request.request_id,
                    tool_name: request.tool_name,
                    params: request.params,
                    agent_id: request.agent_id,
                    session_id: request.session_id,
                    workspace_path: request.workspace_path,
                };
                if !enqueue_ws_message(
                    &outbound_clone,
                    WsMessage::Text(serde_json::to_string(&msg).unwrap_or_default()),
                ) {
                    debug!("WS connection closed for remote executor {conn_id_clone}");
                    break;
                }
            }
        });

        let count = self.remote_exec_registry.executor_count().await;
        let resp = WsResponseType::RemoteCapabilitiesAck {
            accepted: true,
            message: format!(
                "Registered as remote executor ({client_type}). Total executors: {count}"
            ),
        };
        let _ = enqueue_ws_message(outbound_tx, WsMessage::Text(
            serde_json::to_string(&resp).unwrap_or_default(),
        ));

        info!(
            "Remote executor registered: conn={}, type={}, capabilities={:?}",
            conn_id, client_type, capabilities
        );
    }

    /// Get the remote executor registry (for agents to dispatch tool calls).
    #[cfg(feature = "remote-exec")]
    pub fn remote_exec_registry(
        &self,
    ) -> &Arc<rockbot_client::remote_exec::RemoteExecutorRegistry> {
        &self.remote_exec_registry
    }

    /// Handle health check endpoint
    async fn handle_health_check(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let health = self.get_health_status().await;
        let body = json_string(&health);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    /// `GET /api/metrics` — return basic runtime metrics as JSON.
    async fn handle_metrics(&self) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let agents = self.agents.read().await;
        let agent_count = agents.len() as u64;
        drop(agents);

        rockbot_agent::metrics::set_active_agents(agent_count);

        // Return a simple JSON snapshot of key counts
        let body = serde_json::to_string(&serde_json::json!({
            "active_agents": agent_count,
            "note": "Install a metrics recorder (e.g. metrics-exporter-prometheus) for full Prometheus-format metrics at this endpoint."
        })).unwrap_or_default();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    /// Handle list agents endpoint — returns full agent info by merging
    /// active agents, pending agents, and config-declared agents.
    async fn handle_list_agents(&self) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let active = self.agents.read().await;
        let pending = self.pending_agents.read().await;
        let configs = self.agents_config.read().await;

        let mut seen = std::collections::HashSet::new();
        let mut agent_list: Vec<serde_json::Value> = Vec::new();

        // Active agents — get session count from session manager
        for (id, agent) in active.iter() {
            seen.insert(id.clone());
            let session_count = self
                .session_manager
                .query_sessions(rockbot_session::SessionQuery {
                    agent_id: Some(id.clone()),
                    exclude_archived: true,
                    ..Default::default()
                })
                .await
                .map(|s| s.len())
                .unwrap_or(0);
            let cfg = &agent.config;
            agent_list.push(serde_json::json!({
                "id": id,
                "status": "active",
                "model": cfg.model,
                "parent_id": cfg.parent_id,
                "creator_agent_id": cfg.creator_agent_id,
                "owner_agent_id": cfg.owner_agent_id,
                "zone_id": cfg.zone_id,
                "system_prompt": cfg.system_prompt,
                "workspace": cfg.workspace.as_ref().map(|p| p.display().to_string()),
                "primary": cfg.primary,
                "max_tool_calls": cfg.max_tool_calls,
                "temperature": cfg.temperature,
                "max_tokens": cfg.max_tokens,
                "enabled": cfg.enabled,
                "session_count": session_count,
            }));
        }

        // Pending agents
        for p in pending.iter() {
            if seen.insert(p.config.id.clone()) {
                agent_list.push(serde_json::json!({
                    "id": p.config.id,
                    "status": "pending",
                    "model": p.config.model,
                    "parent_id": p.config.parent_id,
                    "creator_agent_id": p.config.creator_agent_id,
                    "owner_agent_id": p.config.owner_agent_id,
                    "zone_id": p.config.zone_id,
                    "system_prompt": p.config.system_prompt,
                    "workspace": p.config.workspace.as_ref().map(|p| p.display().to_string()),
                    "primary": p.config.primary,
                    "max_tool_calls": p.config.max_tool_calls,
                    "temperature": p.config.temperature,
                    "max_tokens": p.config.max_tokens,
                    "enabled": p.config.enabled,
                    "session_count": 0,
                    "reason": p.reason,
                }));
            }
        }

        // Config-declared agents that aren't active or pending (e.g. disabled)
        for cfg in configs.iter() {
            if seen.insert(cfg.id.clone()) {
                let status = if cfg.enabled {
                    "configured"
                } else {
                    "disabled"
                };
                agent_list.push(serde_json::json!({
                    "id": cfg.id,
                    "status": status,
                    "model": cfg.model,
                    "parent_id": cfg.parent_id,
                    "creator_agent_id": cfg.creator_agent_id,
                    "owner_agent_id": cfg.owner_agent_id,
                    "zone_id": cfg.zone_id,
                    "system_prompt": cfg.system_prompt,
                    "workspace": cfg.workspace.as_ref().map(|p| p.display().to_string()),
                    "primary": cfg.primary,
                    "max_tool_calls": cfg.max_tool_calls,
                    "temperature": cfg.temperature,
                    "max_tokens": cfg.max_tokens,
                    "enabled": cfg.enabled,
                    "session_count": 0,
                }));
            }
        }

        let body = json_string(&agent_list);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    /// Handle create agent request
    async fn handle_create_agent<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        fn default_enabled() -> bool {
            true
        }

        #[derive(Deserialize)]
        struct CreateAgentRequest {
            id: String,
            model: Option<String>,
            parent_id: Option<String>,
            creator_agent_id: Option<String>,
            owner_agent_id: Option<String>,
            zone_id: Option<String>,
            workspace: Option<String>,
            primary: Option<bool>,
            max_tool_calls: Option<u32>,
            temperature: Option<f32>,
            max_tokens: Option<u32>,
            system_prompt: Option<String>,
            #[serde(default = "default_enabled")]
            enabled: bool,
        }

        let req: CreateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        if !Self::is_valid_agent_id(req.id.trim()) {
            return Ok(Self::json_error(
                "Agent ID must be 1-64 chars and use only letters, numbers, '-' or '_'",
                StatusCode::BAD_REQUEST,
            ));
        }

        let mut config = rockbot_config::AgentInstance {
            id: req.id.clone(),
            primary: false,
            model: req.model,
            workspace: req.workspace.map(std::path::PathBuf::from),
            max_tool_calls: req.max_tool_calls,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            parent_id: req.parent_id,
            creator_agent_id: req.creator_agent_id.clone().or(req.owner_agent_id.clone()),
            owner_agent_id: req.owner_agent_id.clone(),
            zone_id: req.zone_id.clone(),
            system_prompt: req.system_prompt.clone(),
            enabled: req.enabled,
            mcp_servers: std::collections::HashMap::new(),
            config: std::collections::HashMap::new(),
            max_context_tokens: 128000,
            guardrails: Vec::new(),
            reflection_enabled: false,
            breakpoint_tools: Vec::new(),
            planning_mode: "never".to_string(),
            expose_as_tool: None,
            episodic_memory: false,
            workflow: None,
            llm_timeout_secs: 45,
            tool_timeout_secs: 120,
        };

        {
            let mut configs = self.agents_config.write().await;
            if self.agents.read().await.contains_key(&req.id) {
                return Ok(Self::json_error(
                    &format!("Agent '{}' already exists", req.id),
                    StatusCode::CONFLICT,
                ));
            }
            if configs.iter().any(|c| c.id == req.id) {
                return Ok(Self::json_error(
                    &format!("Agent '{}' already exists in config", req.id),
                    StatusCode::CONFLICT,
                ));
            }

            let has_primary = configs.iter().any(|cfg| cfg.primary);
            config.primary = req.primary.unwrap_or(!has_primary);
            configs.push(config.clone());
        }

        if let Err(e) = self
            .initialize_agent_state(&config.id, req.system_prompt.as_deref())
            .await
        {
            error!("Failed to initialize canonical agent state: {}", e);
        }
        let storage_root = self
            .credentials_config
            .vault_path
            .parent()
            .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
        if let Ok(runtime) =
            rockbot_storage_runtime::StorageRuntime::new_with_root_sync(&Config::default(), storage_root)
        {
            let _ = runtime.ensure_agent_topology(&config, "gateway_api");
        }

        // Persist to authoritative store when available; otherwise keep runtime-only.
        self.persist_agent_to_store(&config);

        // Try to create the agent via factory
        let status = if let Some(ref factory) = self.agent_factory {
            match factory(config.clone()).await {
                Ok(mut agent) => {
                    // Inject agent invoker, blackboard, and remote exec registry
                    if let Some(a) = Arc::get_mut(&mut agent) {
                        a.set_agent_invoker(self.agent_invoker());
                        a.set_blackboard(self.blackboard());
                        #[cfg(feature = "remote-exec")]
                        a.set_remote_exec_registry(self.remote_exec_registry.clone());
                    }
                    self.agents.write().await.insert(req.id.clone(), agent);
                    "created"
                }
                Err(e) => {
                    self.pending_agents.write().await.push(PendingAgent {
                        config,
                        reason: e.to_string(),
                    });
                    "pending"
                }
            }
        } else {
            self.pending_agents.write().await.push(PendingAgent {
                config,
                reason: "No agent factory configured".to_string(),
            });
            "pending"
        };

        if status == "created" {
            self.register_agent_tools().await;
        }

        let body = serde_json::json!({ "status": status, "id": req.id });
        let code = if status == "created" {
            StatusCode::CREATED
        } else {
            StatusCode::ACCEPTED
        };
        Ok(Self::json_response(&body.to_string(), code))
    }

    /// Handle update agent request
    async fn handle_update_agent<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let path = req.uri().path().to_string();
        let agent_id = path.strip_prefix("/api/agents/").unwrap_or("").to_string();

        if !Self::is_valid_agent_id(&agent_id) {
            return Ok(Self::json_error(
                "Invalid agent ID",
                StatusCode::BAD_REQUEST,
            ));
        }

        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(Deserialize)]
        struct UpdateAgentRequest {
            model: Option<String>,
            parent_id: Option<String>,
            owner_agent_id: Option<String>,
            zone_id: Option<String>,
            workspace: Option<String>,
            primary: Option<bool>,
            max_tool_calls: Option<u32>,
            temperature: Option<f32>,
            max_tokens: Option<u32>,
            system_prompt: Option<String>,
            enabled: Option<bool>,
        }

        let update: UpdateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ))
            }
        };

        // Check if agent exists in config or runtime
        let mut configs = self.agents_config.write().await;
        let config_entry = configs.iter_mut().find(|c| c.id == agent_id);

        let agents = self.agents.read().await;
        let exists_active = agents.contains_key(&agent_id);
        drop(agents);

        if config_entry.is_none() && !exists_active {
            return Ok(Self::json_error(
                &format!("Agent '{agent_id}' not found"),
                StatusCode::NOT_FOUND,
            ));
        }

        // Update in-memory config
        if let Some(cfg) = config_entry {
            if let Some(model) = &update.model {
                cfg.model = if model.is_empty() {
                    None
                } else {
                    Some(model.clone())
                };
            }
            if let Some(parent_id) = &update.parent_id {
                cfg.parent_id = if parent_id.is_empty() {
                    None
                } else {
                    Some(parent_id.clone())
                };
            }
            if let Some(owner_agent_id) = &update.owner_agent_id {
                cfg.owner_agent_id = if owner_agent_id.is_empty() {
                    None
                } else {
                    Some(owner_agent_id.clone())
                };
            }
            if let Some(zone_id) = &update.zone_id {
                cfg.zone_id = if zone_id.is_empty() {
                    None
                } else {
                    Some(zone_id.clone())
                };
            }
            if let Some(workspace) = &update.workspace {
                cfg.workspace = if workspace.is_empty() {
                    None
                } else {
                    Some(std::path::PathBuf::from(workspace))
                };
            }
            if let Some(primary) = update.primary {
                cfg.primary = primary;
            }
            if let Some(max_tool_calls) = update.max_tool_calls {
                cfg.max_tool_calls = Some(max_tool_calls);
            }
            if let Some(temperature) = update.temperature {
                cfg.temperature = Some(temperature);
            }
            if let Some(max_tokens) = update.max_tokens {
                cfg.max_tokens = Some(max_tokens);
            }
            if let Some(system_prompt) = &update.system_prompt {
                cfg.system_prompt = if system_prompt.is_empty() {
                    None
                } else {
                    Some(system_prompt.clone())
                };
                let storage_root = self
                    .credentials_config
                    .vault_path
                    .parent()
                    .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
                let _ = rockbot_storage_runtime::write_agent_context_file(
                    &storage_root,
                    &agent_id,
                    "SYSTEM-PROMPT.md",
                    system_prompt,
                )
                .await;
            }
            if let Some(enabled) = update.enabled {
                cfg.enabled = enabled;
            }
        }

        // Persist to authoritative store when available; otherwise keep runtime-only.
        // Grab the updated config for potential agent recreation
        let updated_config = configs.iter().find(|c| c.id == agent_id).cloned();
        if let Some(ref cfg) = updated_config {
            self.persist_agent_to_store(cfg);
            let storage_root = self
                .credentials_config
                .vault_path
                .parent()
                .map_or_else(rockbot_storage_runtime::default_storage_root, std::path::PathBuf::from);
            if let Ok(runtime) =
                rockbot_storage_runtime::StorageRuntime::new_with_root_sync(&Config::default(), storage_root)
            {
                let _ = runtime.ensure_agent_topology(cfg, "gateway_update");
            }
        }
        drop(configs);

        // Recreate the running agent instance so it picks up config changes (e.g. model)
        if let (Some(ref factory), Some(cfg)) = (&self.agent_factory, updated_config) {
            match factory(cfg).await {
                Ok(mut new_agent) => {
                    if let Some(a) = Arc::get_mut(&mut new_agent) {
                        a.set_agent_invoker(self.agent_invoker());
                        a.set_blackboard(self.blackboard());
                        #[cfg(feature = "remote-exec")]
                        a.set_remote_exec_registry(self.remote_exec_registry.clone());
                    }
                    let mut agents = self.agents.write().await;
                    agents.insert(agent_id.clone(), new_agent);
                    info!("Recreated agent '{}' with updated config", agent_id);
                }
                Err(e) => {
                    warn!(
                        "Could not recreate agent '{}' after config update: {}",
                        agent_id, e
                    );
                    // Config is still persisted; agent will pick up changes on restart
                }
            }
        }

        let body = serde_json::json!({ "status": "updated", "id": agent_id });
        Ok(Self::json_response(&body.to_string(), StatusCode::OK))
    }

    /// Handle delete agent request
    async fn handle_delete_agent(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let agent_id = path.strip_prefix("/api/agents/").unwrap_or("").to_string();

        if !Self::is_valid_agent_id(&agent_id) {
            return Ok(Self::json_error(
                "Invalid agent ID",
                StatusCode::BAD_REQUEST,
            ));
        }

        let removed_active = self.agents.write().await.remove(&agent_id);
        self.pending_agents
            .write()
            .await
            .retain(|p| p.config.id != agent_id);

        // Remove from config
        let mut configs = self.agents_config.write().await;
        let had_config = configs.len();
        configs.retain(|c| c.id != agent_id);
        let removed_config = configs.len() < had_config;

        if removed_config {
            // Remove from authoritative store when available.
            self.delete_agent_from_store(&agent_id);
            drop(configs);
        } else {
            drop(configs);
        }

        if removed_active.is_some() || removed_config {
            let body = serde_json::json!({ "status": "deleted", "id": agent_id });
            Ok(Self::json_response(&body.to_string(), StatusCode::OK))
        } else {
            Ok(Self::json_error(
                &format!("Agent '{agent_id}' not found"),
                StatusCode::NOT_FOUND,
            ))
        }
    }

    /// Handle agent message via HTTP API
    async fn handle_agent_message(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path
            .strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/message"))
            .unwrap_or("");

        if !Self::is_valid_agent_id(agent_id) {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }

        // Keep a copy of agent_id since path will be consumed
        let agent_id = agent_id.to_string();

        // Parse request body
        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        let message_request: MessageRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Invalid JSON".into())))
                    .unwrap());
            }
        };

        // Intercept /overseer commands before agent processing
        #[cfg(feature = "overseer")]
        if message_request.message.trim().starts_with("/overseer") {
            let output = if let Some(ref overseer) = self.overseer {
                match overseer.dispatch_command(&message_request.message) {
                    rockbot_overseer::CommandResult::Handled(out) => Some(out),
                    rockbot_overseer::CommandResult::NotHandled => None,
                }
            } else if let Some(ref err) = self.overseer_init_error {
                Some(format!(
                    "The overseer is configured but failed to initialize: {err}. \
                     Check the gateway logs for details."
                ))
            } else {
                Some(
                    "The overseer feature is compiled in but not configured. \
                      Add an `[overseer]` section to your config file."
                        .to_string(),
                )
            };
            if let Some(output) = output {
                let resp = serde_json::json!({ "response": output });
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(
                        serde_json::to_string(&resp).unwrap_or_default().into(),
                    )))
                    .unwrap());
            }
        }

        // Intercept /butler commands before agent processing
        #[cfg(feature = "butler")]
        if message_request.message.trim().starts_with("/butler") {
            if let Some(ref butler) = self.butler {
                match butler.dispatch_command(message_request.message.trim()) {
                    rockbot_butler::CommandResult::Handled(output) => {
                        let resp = serde_json::json!({ "response": output });
                        return Ok(Response::builder()
                            .status(StatusCode::OK)
                            .header("Content-Type", "application/json")
                            .body(GatewayBody::Left(Full::new(
                                serde_json::to_string(&resp).unwrap_or_default().into(),
                            )))
                            .unwrap());
                    }
                    rockbot_butler::CommandResult::NotHandled => {}
                }
            }
        }

        // Intercept gateway slash commands before agent processing
        if let Some(output) = self.handle_slash_commands(&message_request.message).await {
            let resp = serde_json::json!({ "response": output });
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(
                    serde_json::to_string(&resp).unwrap_or_default().into(),
                )))
                .unwrap());
        }

        // Process message
        match self.process_agent_message(&agent_id, message_request).await {
            Ok(response) => {
                let body = json_string(&response);
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(body.into())))
                    .unwrap())
            }
            Err(e) => {
                let error_response = ErrorResponse {
                    error: e.to_string(),
                    code: None,
                };
                let body = json_string(&error_response);
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(body.into())))
                    .unwrap())
            }
        }
    }

    /// Handle agent message with SSE streaming response.
    ///
    /// `POST /api/agents/{id}/stream` — returns `text/event-stream` with
    /// `StreamingChunk` JSON objects as SSE data events.
    async fn handle_agent_message_stream(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path
            .strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/stream"))
            .unwrap_or("");

        if !Self::is_valid_agent_id(agent_id) {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }

        let agent_id = agent_id.to_string();

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        let message_request: MessageRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Invalid JSON".into())))
                    .unwrap());
            }
        };

        // Get agent reference
        let agent = {
            let agents = self.agents.read().await;
            match agents.get(&agent_id) {
                Some(a) => a.clone(),
                None => {
                    return Ok(Self::json_error(
                        &format!("Agent '{agent_id}' not found"),
                        StatusCode::NOT_FOUND,
                    ));
                }
            }
        };

        let session_id = format!("{agent_id}:{}", message_request.session_key);
        let message = Message::text(message_request.message)
            .with_session_id(&session_id)
            .with_role(MessageRole::User);
        let workspace = message_request.workspace.map(std::path::PathBuf::from);

        // Create mpsc channel for SSE events
        let (sse_tx, sse_rx) = tokio::sync::mpsc::channel::<
            std::result::Result<Frame<hyper::body::Bytes>, std::convert::Infallible>,
        >(128);

        // Create streaming chunk channel for the agent
        let (stream_tx, mut stream_rx) =
            tokio::sync::mpsc::channel::<rockbot_llm::StreamingChunk>(128);

        // Spawn the agent processing task
        let processing_sse_tx = sse_tx.clone();
        let processing_handle = tokio::spawn(async move {
            let result = agent
                .process_message_streaming(session_id, message, workspace, stream_tx)
                .await;

            // Send final event with the complete AgentResponse
            match result {
                Ok(response) => {
                    if let Ok(json) = serde_json::to_string(&response) {
                        let event = format!("event: done\ndata: {json}\n\n");
                        let _ = processing_sse_tx
                            .send(Ok(Frame::data(hyper::body::Bytes::from(event))))
                            .await;
                    }
                }
                Err(e) => {
                    let error_json = serde_json::json!({"error": e.to_string()});
                    let event = format!("event: error\ndata: {error_json}\n\n");
                    let _ = processing_sse_tx
                        .send(Ok(Frame::data(hyper::body::Bytes::from(event))))
                        .await;
                }
            }
            // Channel drops, stream ends
        });
        let processing_abort = processing_handle.abort_handle();

        // Spawn task to forward StreamingChunks as SSE data events
        let sse_tx_clone = sse_tx.clone();
        tokio::spawn(async move {
            while let Some(chunk) = stream_rx.recv().await {
                let json = match serde_json::to_string(&chunk) {
                    Ok(j) => j,
                    Err(_) => continue,
                };
                let sse_event = format!("data: {json}\n\n");
                let frame = Frame::data(hyper::body::Bytes::from(sse_event));
                if sse_tx_clone.send(Ok(frame)).await.is_err() {
                    processing_abort.abort();
                    break;
                }
            }
        });

        // Return SSE streaming response
        let stream = tokio_stream::wrappers::ReceiverStream::new(sse_rx);
        let body = StreamBody::new(stream);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/event-stream")
            .header("Cache-Control", "no-cache")
            .header("Connection", "keep-alive")
            .body(GatewayBody::Right(body))
            .unwrap())
    }

    /// Process an agent message
    async fn process_agent_message(
        &self,
        agent_id: &str,
        request: MessageRequest,
    ) -> Result<AgentResponse> {
        let agents = self.agents.read().await;
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| GatewayError::InvalidRequest {
                message: format!("Agent '{agent_id}' not found"),
            })?;

        // Create session ID from session key
        let session_id = format!("{}:{}", agent_id, request.session_key);

        // Convert request to message
        let message = Message::text(request.message)
            .with_session_id(&session_id)
            .with_role(MessageRole::User);

        let workspace_override = request
            .workspace
            .as_ref()
            .map(|workspace| std::path::PathBuf::from(workspace.as_str()));
        let remote_workspace_override = if request.allow_active_client_tools.unwrap_or(false) {
            request.workspace.clone()
        } else {
            None
        };
        let executor_target = request.executor_target.clone();
        let strict_executor_target = executor_target.is_some();

        // Process message with optional workspace override from the client
        Ok(agent
            .process_message(
                session_id,
                message,
                ProcessMessageOptions {
                    workspace_override,
                    remote_executor_target: executor_target,
                    remote_executor_strict: strict_executor_target,
                    remote_workspace_override,
                    delegation_depth: 0,
                },
            )
            .await?)
    }

    /// Get gateway health status
    pub(crate) async fn get_health_status(&self) -> GatewayHealth {
        let agents = self.agents.read().await;
        let connections = self.ws_connections.read().await;

        let mut agent_health = Vec::new();
        for agent in agents.values() {
            if let Ok(health) = agent.health_check().await {
                agent_health.push(health);
            }
        }

        // Get session statistics
        let session_stats = self.session_manager.get_statistics().await.unwrap_or(
            rockbot_session::SessionStatistics {
                total_sessions: 0,
                active_sessions: 0,
                total_messages: 0,
                total_tokens: 0,
            },
        );

        let pending = self.pending_agents.read().await;
        let uptime_secs = self.started_at.elapsed().as_secs();
        let memory_bytes = Self::current_process_memory_bytes().unwrap_or(0);

        GatewayHealth {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime_secs,
            active_connections: connections.len(),
            active_sessions: session_stats.active_sessions as usize,
            pending_agents: pending.len(),
            agents: agent_health,
            memory_usage: MemoryUsage {
                allocated_bytes: memory_bytes,
                heap_size_bytes: 0,
            },
        }
    }

    fn current_process_memory_bytes() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        Some(kb * 1024)
    }

    /// Shutdown the gateway
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down gateway");

        // Stop the cron scheduler
        self.cron_scheduler.shutdown().await;

        let _ = self.shutdown_tx.send(());

        // Close all WebSocket connections
        let connections = self.ws_connections.read().await;
        for connection in connections.values() {
            let _ = enqueue_ws_message(&connection.sender, WsMessage::Close(None));
        }

        Ok(())
    }

    /// `POST /api/agents/{id}/approve` — approve or deny a pending tool call.
    ///
    /// Request body: `{ "request_id": "...", "approved": true|false }`
    /// Returns 200 with `{ "status": "approved" }` or `{ "status": "denied" }`.
    /// Returns 404 if no pending approval with that ID exists.
    async fn handle_tool_approval(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path
            .strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/approve"))
            .unwrap_or("");

        if !Self::is_valid_agent_id(agent_id) {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        let approval: ToolApprovalRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(
                        serde_json::to_string(&serde_json::json!({
                            "error": "Invalid JSON. Expected: {\"request_id\": \"...\", \"approved\": true|false}"
                        })).unwrap_or_default().into()
                    )))
                    .unwrap());
            }
        };

        // Verify agent exists
        {
            let agents = self.agents.read().await;
            if !agents.contains_key(agent_id) {
                return Ok(Self::json_error(
                    &format!("Agent '{agent_id}' not found"),
                    StatusCode::NOT_FOUND,
                ));
            }
        }

        let status = if approval.approved {
            "approved"
        } else {
            "denied"
        };
        let response_json = serde_json::to_string(&serde_json::json!({
            "status": status,
            "request_id": approval.request_id,
            "agent_id": agent_id,
        }))
        .unwrap_or_default();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(response_json.into())))
            .unwrap())
    }

    /// `GET /.well-known/agent.json` — serve the A2A agent card for discovery.
    async fn handle_agent_card(&self) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let base_url = format!("http://{}:{}", self.config.bind_host, self.config.port);
        let card = crate::a2a::build_agent_card("rockbot", "RockBot AI Gateway", &base_url, true);
        let body = serde_json::to_string(&card).unwrap_or_default();
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    // `POST /a2a` — JSON-RPC 2.0 dispatch for A2A protocol.
    // -----------------------------------------------------------------------
    // Cron API handlers
    // -----------------------------------------------------------------------

    async fn handle_list_cron_jobs(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let jobs = self.cron_scheduler.list_jobs(false).await;
        let json = serde_json::to_string(&jobs).unwrap_or_else(|_| "[]".to_string());
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json.into())))
            .unwrap())
    }

    async fn handle_create_cron_job<B>(
        &self,
        req: Request<B>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };
        let job: crate::cron::CronJob = match serde_json::from_slice(&body) {
            Ok(j) => j,
            Err(e) => {
                let json = serde_json::json!({"error": format!("Invalid job: {e}")}).to_string();
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap());
            }
        };

        match self.cron_scheduler.add_job(job).await {
            Ok(created) => {
                let json = serde_json::to_string(&created).unwrap_or_default();
                Ok(Response::builder()
                    .status(StatusCode::CREATED)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => {
                let json = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
        }
    }

    async fn handle_get_cron_job(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let job_id = path.strip_prefix("/api/cron/jobs/").unwrap_or("");
        match self.cron_scheduler.get_job(job_id).await {
            Ok(Some(job)) => {
                let json = serde_json::to_string(&job).unwrap_or_default();
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Ok(None) => {
                let json = serde_json::json!({"error": "Job not found"}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => {
                let json = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
        }
    }

    async fn handle_update_cron_job<B>(
        &self,
        req: Request<B>,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error>
    where
        B: hyper::body::Body<Data = hyper::body::Bytes> + Send,
        B::Error: std::fmt::Debug,
    {
        let _job_id = path.strip_prefix("/api/cron/jobs/").unwrap_or("");
        let body = match Self::collect_limited_body_generic(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };
        let job: crate::cron::CronJob = match serde_json::from_slice(&body) {
            Ok(j) => j,
            Err(e) => {
                let json = serde_json::json!({"error": format!("Invalid job: {e}")}).to_string();
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap());
            }
        };

        match self.cron_scheduler.update_job(job).await {
            Ok(updated) => {
                let json = serde_json::to_string(&updated).unwrap_or_default();
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => {
                let json = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
        }
    }

    async fn handle_delete_cron_job(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let job_id = path.strip_prefix("/api/cron/jobs/").unwrap_or("");
        match self.cron_scheduler.remove_job(job_id).await {
            Ok(()) => {
                let json = serde_json::json!({"status": "deleted"}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => {
                let json = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
        }
    }

    async fn handle_trigger_cron_job(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let job_id = path
            .strip_prefix("/api/cron/jobs/")
            .and_then(|s| s.strip_suffix("/trigger"))
            .unwrap_or("");
        match self.cron_scheduler.trigger_now(job_id).await {
            Ok(()) => {
                let json = serde_json::json!({"status": "triggered"}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => {
                let json = serde_json::json!({"error": e.to_string()}).to_string();
                Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
        }
    }

    /// List connected clients with their identity info (for cron target selection)
    async fn handle_list_cron_clients(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let conns = self.ws_connections.read().await;
        let clients: Vec<serde_json::Value> = conns
            .iter()
            .map(|(conn_id, c)| {
                let (client_uuid, hostname, label) = match &c.identity {
                    Some(id) => (
                        Some(id.client_uuid.as_str()),
                        Some(id.hostname.as_str()),
                        id.label.as_deref(),
                    ),
                    None => (None, None, None),
                };
                serde_json::json!({
                    "id": conn_id,
                    "client_uuid": client_uuid,
                    "hostname": hostname,
                    "label": label,
                    "connected": true,
                })
            })
            .collect();
        let json = serde_json::to_string(&clients).unwrap_or_else(|_| "[]".to_string());
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json.into())))
            .unwrap())
    }

    /// List registered remote executors with identity and capability details.
    #[cfg(feature = "remote-exec")]
    async fn handle_list_executors(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let executors = self.remote_exec_registry.list_executors().await;
        let payload: Vec<serde_json::Value> = executors
            .into_iter()
            .map(|(conn_id, identity, caps)| {
                let target_id = identity
                    .client_uuid
                    .clone()
                    .unwrap_or_else(|| conn_id.clone());
                let capabilities: Vec<String> = caps
                    .capabilities
                    .iter()
                    .map(|cap| format!("{cap:?}").to_lowercase())
                    .collect();
                serde_json::json!({
                    "conn_id": conn_id,
                    "target_id": target_id,
                    "client_uuid": identity.client_uuid,
                    "hostname": identity.hostname,
                    "label": identity.label,
                    "client_type": caps.client_type,
                    "working_dir": caps.working_dir,
                    "capabilities": capabilities,
                    "connected": true,
                })
            })
            .collect();
        let json = serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_string());
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json.into())))
            .unwrap())
    }

    #[cfg(not(feature = "remote-exec"))]
    async fn handle_list_executors(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new("[]".into())))
            .unwrap())
    }

    // ==================== Certificate API Handlers ====================

    /// Handle POST /api/cert/sign — PSK-authenticated CSR signing
    async fn handle_cert_sign(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let source_ip = req.extensions().get::<SocketAddr>().map(|addr| addr.ip());
        match source_ip {
            Some(ip) if self.allow_cert_sign_attempt(ip).await => {}
            Some(_) => {
                return Ok(Self::json_error(
                    "Too many certificate signing attempts from this IP",
                    StatusCode::TOO_MANY_REQUESTS,
                ));
            }
            None => {
                return Ok(Self::json_error(
                    "Missing client address for rate limiting",
                    StatusCode::BAD_REQUEST,
                ));
            }
        }

        // Check that PKI is configured
        let pki_dir = match &self.pki.pki_dir {
            Some(d) => Self::expand_tilde(d),
            None => {
                let default = dirs::config_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                    .join("rockbot")
                    .join("pki");
                default
            }
        };

        let ca_cert_path = pki_dir.join("ca.crt");

        // Parse request body
        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        #[derive(serde::Deserialize)]
        struct SignRequest {
            csr: String,
            psk: String,
            name: String,
            #[serde(default = "default_role")]
            role: String,
            #[serde(default = "default_days")]
            days: u32,
        }
        fn default_role() -> String {
            "agent".to_string()
        }
        fn default_days() -> u32 {
            365
        }

        let sign_req: SignRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Invalid JSON: {e}"),
                    StatusCode::BAD_REQUEST,
                ));
            }
        };

        if !Self::is_valid_cert_name(&sign_req.name) {
            return Ok(Self::json_error(
                "Invalid certificate name",
                StatusCode::BAD_REQUEST,
            ));
        }

        // Validate PSK against enrollment tokens in the index
        let index_path = pki_dir.join("index.json");
        let mut index = match rockbot_pki::PkiIndex::load(&index_path) {
            Ok(idx) => idx,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Failed to load PKI index: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                ));
            }
        };

        let role = match rockbot_pki::CertRole::from_str(&sign_req.role) {
            Some(r) => r,
            None => {
                return Ok(Self::json_error(
                    &format!(
                        "Invalid role '{}'. Must be: gateway, agent, or tui",
                        sign_req.role
                    ),
                    StatusCode::BAD_REQUEST,
                ));
            }
        };

        let enrollment = match index.validate_enrollment(&sign_req.psk, role) {
            Ok(enrollment) => enrollment,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("Enrollment failed: {e}"),
                    StatusCode::FORBIDDEN,
                ));
            }
        };

        // Load CA cert and key
        let ca_cert_pem = match std::fs::read_to_string(&ca_cert_path) {
            Ok(s) => s,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("PKI not initialized: failed to read CA cert: {e}"),
                    StatusCode::SERVICE_UNAVAILABLE,
                ));
            }
        };

        let pki_manager = match rockbot_pki::PkiManager::new(pki_dir.clone()) {
            Ok(manager) => manager,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("PKI not initialized: failed to open PKI manager: {e}"),
                    StatusCode::SERVICE_UNAVAILABLE,
                ));
            }
        };
        let ca_key = match pki_manager.ca_key_handle() {
            Ok(k) => k,
            Err(e) => {
                return Ok(Self::json_error(
                    &format!("PKI not initialized: failed to load CA key: {e}"),
                    StatusCode::SERVICE_UNAVAILABLE,
                ));
            }
        };

        // Sign the CSR
        let serial = index.next_serial();
        match rockbot_pki::sign_csr(
            &sign_req.csr,
            &ca_cert_pem,
            &ca_key,
            &sign_req.name,
            role,
            sign_req.days,
            serial,
            &enrollment.roles,
            &[],
        ) {
            Ok((cert_pem, entry)) => {
                // Save the signed cert to the clients directory
                let clients_dir = pki_dir.join("clients");
                let _ = std::fs::create_dir_all(&clients_dir);
                let cert_path = clients_dir.join(format!("{}.crt", sign_req.name));
                let _ = std::fs::write(&cert_path, &cert_pem);

                index.add_entry(entry);
                if let Err(e) = index.save(&index_path) {
                    warn!("Failed to save PKI index after signing: {e}");
                }

                info!(
                    "Signed CSR for '{}' (role: {}, serial: {})",
                    sign_req.name, sign_req.role, serial
                );

                let response = serde_json::json!({
                    "certificate": cert_pem,
                    "ca_certificate": ca_cert_pem,
                });
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(response.to_string().into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(
                &format!("Failed to sign CSR: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    /// Handle GET /api/cert/ca — return CA certificate info (public)
    async fn handle_cert_ca_info(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let pki_dir = match &self.pki.pki_dir {
            Some(d) => Self::expand_tilde(d),
            None => dirs::config_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join("rockbot")
                .join("pki"),
        };

        let ca_cert_path = pki_dir.join("ca.crt");
        if !ca_cert_path.exists() {
            return Ok(Self::json_error(
                "PKI not initialized",
                StatusCode::SERVICE_UNAVAILABLE,
            ));
        }

        match std::fs::read_to_string(&ca_cert_path) {
            Ok(pem) => {
                let response = serde_json::json!({
                    "ca_certificate": pem,
                });
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(response.to_string().into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(
                &format!("Failed to read CA cert: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }

    async fn handle_a2a_request(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        if !Self::is_a2a_authorized(req.headers()) {
            return Ok(Self::json_error(
                "A2A requests require a valid bearer token",
                StatusCode::UNAUTHORIZED,
            ));
        }

        let body = match Self::collect_limited_body(req, MAX_HTTP_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(resp) => return Ok(resp),
        };

        let rpc_request: crate::a2a::JsonRpcRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                let resp =
                    crate::a2a::JsonRpcResponse::error(None, -32700, "Parse error: invalid JSON");
                let json = serde_json::to_string(&resp).unwrap_or_default();
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap());
            }
        };

        let dispatcher = crate::a2a::A2ADispatcher::with_invoker(
            Arc::clone(&self.a2a_task_store),
            self.agent_invoker(),
        );
        let rpc_response = dispatcher.dispatch(rpc_request).await;
        let json = serde_json::to_string(&rpc_response).unwrap_or_default();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(json.into())))
            .unwrap())
    }
}

// Clone trait for Gateway (needed for Tokio spawning)
impl Clone for Gateway {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pki: self.pki.clone(),
            credentials_config: self.credentials_config.clone(),
            config_path: self.config_path.clone(),
            store: self.store.clone(),
            agents_config: Arc::clone(&self.agents_config),
            agents: Arc::clone(&self.agents),
            pending_agents: Arc::clone(&self.pending_agents),
            agent_factory: self.agent_factory.clone(),
            session_manager: Arc::clone(&self.session_manager),
            credential_manager: self.credential_manager.clone(),
            llm_registry: Arc::clone(&self.llm_registry),
            provider_configured: Arc::clone(&self.provider_configured),
            channel_registry: Arc::clone(&self.channel_registry),
            tool_provider_registry: Arc::clone(&self.tool_provider_registry),
            ws_connections: Arc::clone(&self.ws_connections),
            a2a_task_store: Arc::clone(&self.a2a_task_store),
            cert_sign_attempts: Arc::clone(&self.cert_sign_attempts),
            blackboard: Arc::clone(&self.blackboard),
            cron_scheduler: Arc::clone(&self.cron_scheduler),
            #[cfg(feature = "overseer")]
            overseer: self.overseer.clone(),
            #[cfg(feature = "overseer")]
            overseer_init_error: self.overseer_init_error.clone(),
            #[cfg(feature = "butler")]
            butler: self.butler.clone(),
            #[cfg(feature = "remote-exec")]
            remote_exec_registry: Arc::clone(&self.remote_exec_registry),
            #[cfg(feature = "remote-exec")]
            noise_keypair: Arc::clone(&self.noise_keypair),
            #[cfg(feature = "bedrock-deploy")]
            deploy_distributor: self.deploy_distributor.clone(),
            #[cfg(feature = "bedrock-deploy")]
            deploy_dns: self.deploy_dns.clone(),
            #[cfg(feature = "bedrock-deploy")]
            deploy_config: self.deploy_config.clone(),
            #[cfg(feature = "bedrock-deploy")]
            deploy_init_error: self.deploy_init_error.clone(),
            shutdown_tx: self.shutdown_tx.clone(),
            started_at: self.started_at,
        }
    }
}

/// Gateway implements AgentInvoker so agents can delegate to sibling agents.
///
/// The gateway wraps itself in an `Arc` via `GatewayInvoker` (which holds the
/// shared `agents` map) to avoid requiring `Gateway: 'static`.
#[derive(Clone)]
pub struct GatewayInvoker {
    agents: Arc<tokio::sync::RwLock<HashMap<String, Arc<Agent>>>>,
}

impl GatewayInvoker {
    /// Create a new invoker from the gateway's agent map.
    pub fn new(agents: Arc<tokio::sync::RwLock<HashMap<String, Arc<Agent>>>>) -> Self {
        Self { agents }
    }
}

/// Maximum handoff chain depth to prevent infinite loops
const MAX_HANDOFF_CHAIN_DEPTH: u32 = 5;

#[async_trait::async_trait]
impl rockbot_tools::AgentInvoker for GatewayInvoker {
    async fn invoke_agent(
        &self,
        agent_id: &str,
        message: &str,
        session_id: &str,
        depth: u32,
    ) -> std::result::Result<String, rockbot_tools::ToolError> {
        if depth > MAX_HANDOFF_CHAIN_DEPTH {
            return Err(rockbot_tools::ToolError::ExecutionFailed {
                message: format!("Handoff chain depth limit ({MAX_HANDOFF_CHAIN_DEPTH}) exceeded"),
            });
        }

        let agents = self.agents.read().await;
        let agent =
            agents
                .get(agent_id)
                .ok_or_else(|| rockbot_tools::ToolError::ExecutionFailed {
                    message: format!("invoke_agent: agent '{agent_id}' not found"),
                })?;
        let agent = Arc::clone(agent);
        drop(agents);

        let msg = rockbot_config::message::Message::text(message)
            .with_session_id(session_id)
            .with_role(rockbot_config::message::MessageRole::User);

        match agent
            .process_message(
                session_id.to_string(),
                msg,
                ProcessMessageOptions {
                    delegation_depth: depth,
                    ..ProcessMessageOptions::default()
                },
            )
            .await
        {
            Ok(response) => {
                // If the response includes a handoff, follow the chain
                if let Some(handoff) = &response.handoff {
                    info!(
                        "Handoff chain: {} -> {} (depth {})",
                        agent_id,
                        handoff.target_agent_id,
                        depth + 1
                    );
                    let target_message = if let Some(ref override_msg) = handoff.message_override {
                        override_msg.clone()
                    } else {
                        format!(
                            "Context from agent '{agent_id}':\n{}\n\nOriginal request:\n{message}",
                            handoff.context
                        )
                    };
                    return self
                        .invoke_agent(
                            &handoff.target_agent_id,
                            &target_message,
                            session_id,
                            depth + 1,
                        )
                        .await;
                }

                let text = match &response.message.content {
                    rockbot_config::message::MessageContent::Text { text } => text.clone(),
                    other => format!("{other:?}"),
                };
                Ok(text)
            }
            Err(e) => Err(rockbot_tools::ToolError::ExecutionFailed {
                message: format!("invoke_agent: agent '{agent_id}' error: {e}"),
            }),
        }
    }
}

/// HTTP API tool approval request
#[derive(Debug, Deserialize)]
struct ToolApprovalRequest {
    request_id: String,
    approved: bool,
}

/// HTTP API message request
#[derive(Debug, Deserialize)]
struct MessageRequest {
    session_key: String,
    message: String,
    /// Working directory override (e.g. TUI's cwd)
    workspace: Option<String>,
    /// Explicit execution target: "gateway" or a client UUID/label/hostname.
    #[serde(default)]
    executor_target: Option<String>,
    /// Whether the requesting active client should be used for tools by default.
    #[serde(default)]
    allow_active_client_tools: Option<bool>,
}

/// HTTP API error response
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: Option<String>,
}

// ---------------------------------------------------------------------------
// Cron executor — bridges the cron scheduler to agent execution + remote dispatch
// ---------------------------------------------------------------------------

/// Executes cron jobs by invoking agents locally or dispatching to remote clients.
struct GatewayCronExecutor {
    agents: Arc<RwLock<HashMap<String, Arc<Agent>>>>,
    ws_connections: Arc<RwLock<HashMap<String, WsConnection>>>,
}

#[async_trait::async_trait]
impl crate::cron::CronExecutor for GatewayCronExecutor {
    async fn execute(&self, job: &crate::cron::CronJob) -> std::result::Result<(), String> {
        // If the job targets a specific client, dispatch over WebSocket
        if let Some(ref target_label) = job.target_client {
            return self.dispatch_to_client(job, target_label).await;
        }

        // Otherwise, execute locally on the gateway
        self.execute_locally(job).await
    }
}

impl GatewayCronExecutor {
    /// Execute a cron job locally by invoking the target agent.
    async fn execute_locally(&self, job: &crate::cron::CronJob) -> std::result::Result<(), String> {
        let agent_id = job
            .agent_id
            .as_deref()
            .ok_or_else(|| "Cron job has no agent_id configured".to_string())?;

        let agents = self.agents.read().await;
        let agent = agents
            .get(agent_id)
            .ok_or_else(|| format!("Agent '{}' not found", agent_id))?
            .clone();
        drop(agents);

        let message_text = match &job.payload {
            crate::cron::CronPayload::AgentTurn { message, .. } => message.clone(),
            crate::cron::CronPayload::SystemEvent { event, data } => {
                format!(
                    "[system event: {}] {}",
                    event,
                    data.as_ref().map(|d| d.to_string()).unwrap_or_default()
                )
            }
        };

        let session_id = job
            .session_key
            .clone()
            .unwrap_or_else(|| format!("cron:{}", job.id));

        let user_message = rockbot_config::message::Message::text(&message_text)
            .with_role(rockbot_config::message::MessageRole::User);

        match agent
            .process_message(
                session_id,
                user_message,
                ProcessMessageOptions::default(),
            )
            .await
        {
            Ok(response) => {
                debug!(
                    "Cron job '{}' completed: {} tokens used",
                    job.name, response.tokens_used.total_tokens
                );
                Ok(())
            }
            Err(e) => Err(format!("Agent execution failed: {e}")),
        }
    }

    /// Dispatch a cron job to a specific remote client over WebSocket.
    ///
    /// `target` is matched against client UUID first (exact match), then
    /// falls back to label match, then hostname match. UUID is the
    /// recommended targeting mechanism for cron jobs.
    async fn dispatch_to_client(
        &self,
        job: &crate::cron::CronJob,
        target: &str,
    ) -> std::result::Result<(), String> {
        let conns = self.ws_connections.read().await;

        // Try UUID match first, then label, then hostname
        let target_conn = conns
            .values()
            .find(|c| {
                c.identity
                    .as_ref()
                    .is_some_and(|id| id.client_uuid == target)
            })
            .or_else(|| {
                conns.values().find(|c| {
                    c.identity.as_ref().and_then(|id| id.label.as_deref()) == Some(target)
                })
            })
            .or_else(|| {
                conns
                    .values()
                    .find(|c| c.identity.as_ref().is_some_and(|id| id.hostname == target))
            });

        let conn =
            target_conn.ok_or_else(|| format!("Target client '{}' is not connected", target))?;

        let dispatch_msg = WsResponseType::CronDispatch {
            job_id: job.id.clone(),
            job_name: job.name.clone(),
            agent_id: job.agent_id.clone(),
            payload: job.payload.clone(),
        };

        let json = serde_json::to_string(&dispatch_msg)
            .map_err(|e| format!("Failed to serialize cron dispatch: {e}"))?;

        if !enqueue_ws_message(&conn.sender, WsMessage::Text(json)) {
            return Err(format!(
                "Failed to send cron dispatch to client '{}'",
                target
            ));
        }

        info!("Cron job '{}' dispatched to client '{}'", job.name, target);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use rockbot_config::config::{GatewayPublicConfig, NoiseTransportConfig};
    use rockbot_config::{
        AgentConfig, AgentDefaults, PkiConfig, ProvidersConfig, SandboxConfig, SecurityConfig,
        ToolConfig,
    };
    use std::collections::HashMap;
    use tempfile::NamedTempFile;

    async fn create_test_gateway() -> Gateway {
        let temp_db = NamedTempFile::new().unwrap();
        let session_manager = Arc::new(SessionManager::new(temp_db.path(), 100).await.unwrap());

        let config = Config {
            gateway: GatewayConfig {
                bind_host: "127.0.0.1".to_string(),
                listen_ips: Vec::new(),
                port: 0, // Use 0 for testing to avoid port conflicts
                client_port: 1,
                max_connections: 100,
                request_timeout: 30,
                require_api_key: None,
                pki: PkiConfig::default(),
                public: GatewayPublicConfig::default(),
            },
            pki: PkiConfig::default(),
            agents: AgentConfig {
                defaults: AgentDefaults {
                    workspace: std::env::temp_dir(),
                    model: "test-model".to_string(),
                    heartbeat_interval: "5m".to_string(),
                    max_context_tokens: 128000,
                },
                list: vec![],
            },
            tools: ToolConfig {
                profile: "standard".to_string(),
                deny: vec![],
                configs: HashMap::new(),
            },
            security: SecurityConfig {
                sandbox: SandboxConfig {
                    mode: "tools".to_string(),
                    scope: "session".to_string(),
                    image: None,
                    allowed_command_patterns: vec![],
                    blocked_command_patterns: vec![],
                },
                capabilities: Default::default(),
                storage: Default::default(),
                roles: Default::default(),
                noise: NoiseTransportConfig::default(),
            },
            client: Default::default(),
            credentials: CredentialsConfig::default(),
            providers: ProvidersConfig::default(),
            overseer: None,
            doctor: None,
            deploy: None,
            tui: Default::default(),
            seed_model: Default::default(),
        };

        Gateway::new(config, session_manager, None).await.unwrap()
    }

    #[tokio::test]
    async fn test_gateway_creation() {
        let gateway = create_test_gateway().await;
        let health = gateway.get_health_status().await;

        assert_eq!(health.active_connections, 0);
        assert_eq!(health.agents.len(), 0);
    }

    #[test]
    fn test_requires_ws_auth_allows_client_identify() {
        assert!(!Gateway::requires_ws_auth(&WsMessageType::ClientIdentify {
            client_uuid: None,
            hostname: "host".to_string(),
            label: None,
        }));
        assert!(Gateway::requires_ws_auth(&WsMessageType::AgentMessage {
            agent_id: "agent".to_string(),
            session_key: "s".to_string(),
            message: "hi".to_string(),
            workspace: None,
            executor_target: None,
            allow_active_client_tools: None,
        }));
    }

    #[test]
    fn test_a2a_authorization_accepts_bearer_token() {
        let _guard = EnvGuard::set("ROCKBOT_A2A_TOKEN", Some("secret-token"));
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::AUTHORIZATION,
            hyper::header::HeaderValue::from_static("Bearer secret-token"),
        );
        assert!(Gateway::is_a2a_authorized(&headers));
    }

    #[test]
    fn test_a2a_authorization_rejects_missing_token() {
        let _guard = EnvGuard::set("ROCKBOT_A2A_TOKEN", Some("secret-token"));
        let headers = hyper::HeaderMap::new();
        assert!(!Gateway::is_a2a_authorized(&headers));
    }

    #[tokio::test]
    async fn test_cert_sign_rate_limit_allows_three_attempts_per_minute() {
        let mut attempts = HashMap::new();
        let ip = std::net::IpAddr::from([127, 0, 0, 1]);
        let now = std::time::Instant::now();
        assert!(Gateway::record_cert_sign_attempt(&mut attempts, ip, now));
        assert!(Gateway::record_cert_sign_attempt(&mut attempts, ip, now));
        assert!(Gateway::record_cert_sign_attempt(&mut attempts, ip, now));
        assert!(!Gateway::record_cert_sign_attempt(&mut attempts, ip, now));
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let previous = std::env::var(key).ok();
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let gateway = create_test_gateway().await;
        let response = gateway.handle_health_check().await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
