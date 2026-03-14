//! Gateway server for RockBot
//!
//! This module provides the main gateway server that handles WebSocket connections,
//! HTTP API endpoints, and coordinates agent execution.

use crate::agent::{Agent, AgentResponse};
use crate::config::{Config, CredentialsConfig, GatewayConfig};
use rockbot_credentials::{CredentialManager, MasterKey, generate_salt};

/// Simple base64 decoding (using standard alphabet)
fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, &'static str> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    
    let input = input.trim_end_matches('=');
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    
    for c in input.bytes() {
        let val = match ALPHABET.iter().position(|&b| b == c) {
            Some(v) => v as u32,
            None => return Err("invalid base64 character"),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    
    Ok(output)
}
use crate::error::{GatewayError, Result};
use crate::message::{Message, MessageRole};
use crate::session::SessionManager;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{body::Incoming as IncomingBody, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

/// Response body type supporting both full and SSE streaming responses.
type GatewayBody = http_body_util::Either<
    Full<hyper::body::Bytes>,
    StreamBody<tokio_stream::wrappers::ReceiverStream<std::result::Result<Frame<hyper::body::Bytes>, std::convert::Infallible>>>,
>;

/// Pending agent info (for agents that couldn't be created due to missing credentials)
#[derive(Debug, Clone)]
pub struct PendingAgent {
    pub config: crate::config::AgentInstance,
    pub reason: String,
}

/// Agent factory callback for creating agents
pub type AgentFactory = Arc<dyn Fn(crate::config::AgentInstance) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Arc<Agent>>> + Send>> + Send + Sync>;

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

/// Model info returned by the provider API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
}

/// Main gateway server
pub struct Gateway {
    /// Gateway configuration
    config: GatewayConfig,
    /// Credentials configuration
    credentials_config: CredentialsConfig,
    /// Path to the TOML config file (for persisting agent changes)
    config_path: Option<std::path::PathBuf>,
    /// Agent configurations from the config file (source of truth for declared agents)
    agents_config: Arc<RwLock<Vec<crate::config::AgentInstance>>>,
    /// Registered agents
    agents: Arc<RwLock<HashMap<String, Arc<Agent>>>>,
    /// Pending agents (couldn't be created, e.g., missing API keys)
    pending_agents: Arc<RwLock<Vec<PendingAgent>>>,
    /// Agent factory for creating new agents
    agent_factory: Option<AgentFactory>,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Credential manager (optional, if credentials are enabled)
    credential_manager: Option<Arc<CredentialManager>>,
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
    /// Shutdown broadcast channel
    shutdown_tx: broadcast::Sender<()>,
}

/// WebSocket connection information
struct WsConnection {
    #[allow(dead_code)]
    id: String,
    sender: tokio::sync::mpsc::UnboundedSender<WsMessage>,
    #[allow(dead_code)]
    user_id: Option<String>,
    #[allow(dead_code)]
    connected_at: std::time::Instant,
}

/// WebSocket message types
#[allow(dead_code)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WsMessageType {
    #[serde(rename = "auth")]
    Auth { token: String },
    #[serde(rename = "agent_message")]
    AgentMessage {
        agent_id: String,
        session_key: String,
        message: Message,
    },
    #[serde(rename = "session_list")]
    SessionList { agent_id: Option<String> },
    #[serde(rename = "session_history")]
    SessionHistory {
        session_id: String,
        limit: Option<usize>,
        offset: Option<usize>,
    },
    #[serde(rename = "health_check")]
    HealthCheck,
}

/// WebSocket response types
#[allow(dead_code)]
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum WsResponseType {
    #[serde(rename = "auth_result")]
    AuthResult { success: bool, user_id: Option<String> },
    #[serde(rename = "agent_response")]
    AgentResponse {
        session_id: String,
        response: AgentResponse,
    },
    #[serde(rename = "session_list")]
    SessionList { sessions: Vec<crate::session::Session> },
    #[serde(rename = "session_history")]
    SessionHistory { history: crate::session::MessageHistory },
    #[serde(rename = "health_status")]
    HealthStatus { status: GatewayHealth },
    #[serde(rename = "error")]
    Error { message: String, code: Option<String> },
}

/// Gateway health status
#[derive(Debug, Serialize, Deserialize)]
pub struct GatewayHealth {
    pub version: String,
    pub uptime_seconds: u64,
    /// Alias for TUI compatibility (reads `uptime_secs`)
    pub uptime_secs: u64,
    pub active_connections: usize,
    pub active_sessions: usize,
    pub pending_agents: usize,
    pub agents: Vec<crate::agent::AgentHealthStatus>,
    pub memory_usage: MemoryUsage,
}

/// Memory usage statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryUsage {
    pub allocated_bytes: usize,
    pub heap_size_bytes: usize,
}

impl Gateway {
    /// Create a new gateway with the given configuration
    pub async fn new(config: Config, session_manager: Arc<SessionManager>) -> Result<Self> {
        let (shutdown_tx, _) = broadcast::channel(1);
        
        // Initialize credential manager if enabled
        let credential_manager = if config.credentials.enabled {
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
            tool_provider_registry.register(std::sync::Arc::new(
                rockbot_tools_mcp::McpTool::new(),
            ));
        }
        #[cfg(feature = "tools-markdown")]
        {
            tool_provider_registry.register(std::sync::Arc::new(
                rockbot_tools_markdown::MarkdownTool::new(),
            ));
        }

        Ok(Self {
            config: config.gateway,
            credentials_config: config.credentials,
            config_path: None,
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
            shutdown_tx,
        })
    }

    /// Initialize the credential manager based on configuration
    async fn init_credential_manager(config: &CredentialsConfig) -> Result<CredentialManager> {
        let manager = CredentialManager::new(&config.vault_path)
            .map_err(|e| GatewayError::InvalidRequest {
                message: format!("Failed to open credential vault: {e}"),
            })?;
        
        // Auto-unlock if configured
        match config.unlock_method.as_str() {
            "env" => {
                if let Ok(password) = std::env::var(&config.password_env_var) {
                    let salt = generate_salt();
                    let master_key = MasterKey::derive_from_password(&password, &salt)
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to derive master key: {e}"),
                        })?;
                    manager.unlock(master_key).await
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to unlock vault: {e}"),
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
                // TODO: Implement keyring support
                debug!("Keyring unlock not yet implemented");
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
    
    /// Register an agent with the gateway
    pub async fn register_agent(&self, agent: Arc<Agent>) {
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

    /// Set the agent factory for creating new agents
    pub fn set_agent_factory(&mut self, factory: AgentFactory) {
        self.agent_factory = Some(factory);
    }

    /// Set the config file path (for persisting agent changes)
    pub fn set_config_path(&mut self, path: std::path::PathBuf) {
        self.config_path = Some(path);
    }

    /// Set the LLM provider registry (single source of truth for provider state).
    /// Probes each provider's `is_configured()` and caches the result.
    pub async fn set_llm_registry(&self, registry: Arc<rockbot_llm::LlmProviderRegistry>) {
        // Probe and cache availability for each provider
        let mut cache = HashMap::new();
        for provider_id in registry.list_providers() {
            if let Some(provider) = registry.get_provider(&provider_id) {
                let configured = provider.is_configured().await;
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
                    cache.insert(provider_id, provider.is_configured().await);
                }
            }
            let mut configured = self.provider_configured.write().await;
            *configured = cache;
        }
    }
    
    /// Add a pending agent (couldn't be created, e.g., missing API key)
    pub async fn add_pending_agent(&self, config: crate::config::AgentInstance, reason: String) {
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
            None => return Err(GatewayError::InvalidRequest {
                message: "Agent factory not configured".to_string(),
            }.into()),
        };
        
        let mut pending = self.pending_agents.write().await;
        let mut still_pending = Vec::new();
        let mut created = 0;
        
        for pending_agent in pending.drain(..) {
            let config = pending_agent.config.clone();
            let agent_id = config.id.clone();
            
            match factory(config).await {
                Ok(agent) => {
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
    
    /// Start the gateway server
    pub async fn start(&self) -> Result<()> {
        let addr = format!("{}:{}", self.config.bind_host, self.config.port);
        let listener = TcpListener::bind(&addr).await.map_err(|_| {
            GatewayError::BindFailed {
                host: self.config.bind_host.clone(),
                port: self.config.port,
            }
        })?;
        
        info!("Gateway server listening on {}", addr);
        
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let gateway = self.clone();
                            tokio::spawn(async move {
                                if let Err(e) = gateway.handle_connection(stream, addr).await {
                                    error!("Connection error: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Gateway shutdown requested");
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle a new TCP connection
    async fn handle_connection(&self, stream: TcpStream, addr: SocketAddr) -> Result<()> {
        debug!("New connection from {}", addr);
        
        let io = TokioIo::new(stream);
        
        let gateway = self.clone();
        let service = service_fn(move |req| {
            let gateway = gateway.clone();
            async move { gateway.handle_request(req).await }
        });
        
        if let Err(err) = http1::Builder::new()
            .serve_connection(io, service)
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
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        
        match (req.method(), path.as_str()) {
            // Web UI
            (&Method::GET, "/") | (&Method::GET, "/index.html") => {
                self.handle_web_ui().await
            }
            (&Method::GET, "/ws") => {
                self.handle_websocket_upgrade(req).await
            }
            // A2A Protocol
            (&Method::GET, "/.well-known/agent.json") => {
                self.handle_agent_card().await
            }
            (&Method::POST, "/a2a") => {
                self.handle_a2a_request(req).await
            }
            (&Method::GET, "/health") | (&Method::GET, "/api/status") => {
                self.handle_health_check().await
            }
            (&Method::GET, "/api/metrics") => {
                self.handle_metrics().await
            }
            (&Method::GET, "/api/agents") => {
                self.handle_list_agents().await
            }
            (&Method::POST, "/api/agents") => {
                self.handle_create_agent(req).await
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
            (&Method::DELETE, p) if p.starts_with("/api/credentials/") && !p.contains("/permissions/") && !p.contains("/approvals/") => {
                // DELETE /api/credentials/{id} - alternative to /api/credentials/endpoints/{id}
                let id = path.strip_prefix("/api/credentials/").unwrap_or("");
                let endpoint_path = format!("/api/credentials/endpoints/{id}");
                self.handle_delete_endpoint(&endpoint_path).await
            }
            (&Method::POST, p) if p.starts_with("/api/credentials/endpoints/") && p.ends_with("/credential") => {
                self.handle_store_credential(req, &path).await
            }
            // Permissions API
            (&Method::GET, "/api/credentials/permissions") => {
                self.handle_list_permissions().await
            }
            (&Method::POST, "/api/credentials/permissions") => {
                self.handle_add_permission(req).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/credentials/permissions/") => {
                self.handle_delete_permission(&path).await
            }
            // Audit API
            (&Method::GET, "/api/credentials/audit") => {
                self.handle_get_audit_log(req).await
            }
            // Approvals API
            (&Method::GET, "/api/credentials/approvals") => {
                self.handle_list_approvals().await
            }
            (&Method::POST, p) if p.starts_with("/api/credentials/approvals/") && p.ends_with("/approve") => {
                self.handle_approve_request(&path, req).await
            }
            (&Method::POST, p) if p.starts_with("/api/credentials/approvals/") && p.ends_with("/deny") => {
                self.handle_deny_request(&path, req).await
            }
            (&Method::POST, "/api/credentials/approvals/respond") => {
                self.handle_approval_response(req).await
            }
            (&Method::GET, "/api/credentials/status") => {
                self.handle_credentials_status().await
            }
            (&Method::POST, "/api/credentials/unlock") => {
                self.handle_unlock_vault(req).await
            }
            (&Method::POST, "/api/credentials/lock") => {
                self.handle_lock_vault().await
            }
            (&Method::POST, "/api/credentials/init") => {
                self.handle_init_vault(req).await
            }
            // Provider API endpoints
            (&Method::GET, "/api/providers") => {
                self.handle_list_providers().await
            }
            (&Method::GET, p) if p.starts_with("/api/providers/") && !p.contains("/test") => {
                self.handle_get_provider(&path).await
            }
            (&Method::POST, p) if p.starts_with("/api/providers/") && p.ends_with("/test") => {
                self.handle_test_provider(&path).await
            }
            (&Method::POST, "/api/chat") => {
                self.handle_chat(req).await
            }
            (&Method::GET, "/api/credentials/schemas") => {
                self.handle_credential_schemas().await
            }
            // Sessions API
            (&Method::GET, "/api/sessions") => {
                self.handle_list_sessions(req).await
            }
            (&Method::POST, "/api/sessions") => {
                self.handle_create_session(req).await
            }
            (&Method::GET, p) if p.starts_with("/api/sessions/") && p.ends_with("/messages") => {
                self.handle_get_session_messages(&path).await
            }
            (&Method::DELETE, p) if p.starts_with("/api/sessions/") => {
                self.handle_delete_session(&path).await
            }
            // Gateway management
            (&Method::POST, "/api/gateway/reload") => {
                self.handle_reload_agents().await
            }
            (&Method::GET, "/api/gateway/pending") => {
                self.handle_list_pending_agents().await
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
            _ => {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(GatewayBody::Left(Full::new("Not Found".into())))
                    .unwrap())
            }
        }
    }

    // ==================== Credentials API Handlers ====================

    /// Handle list endpoints
    async fn handle_list_endpoints(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let endpoints = manager.list_endpoints().await;
        // Don't include credential data in the list
        let endpoint_list: Vec<_> = endpoints.iter().map(|e| serde_json::json!({
            "id": e.id,
            "name": e.name,
            "endpoint_type": e.endpoint_type,
            "base_url": e.base_url,
            "created_at": e.created_at,
            "updated_at": e.updated_at,
        })).collect();

        let body = serde_json::to_string(&endpoint_list).unwrap();
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle create endpoint
    async fn handle_create_endpoint(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct CreateEndpointRequest {
            name: String,
            endpoint_type: String,
            base_url: String,
        }

        let request: CreateEndpointRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
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
            _ => return Ok(Self::json_error("Invalid endpoint type", StatusCode::BAD_REQUEST)),
        };

        match manager.create_endpoint(request.name, endpoint_type, request.base_url).await {
            Ok(endpoint) => {
                let body = serde_json::to_string(&endpoint).unwrap();
                Ok(Self::json_response(&body, StatusCode::CREATED))
            }
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle delete endpoint
    async fn handle_delete_endpoint(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let endpoint_id = path.strip_prefix("/api/credentials/endpoints/").unwrap_or("");
        let Ok(uuid) = uuid::Uuid::parse_str(endpoint_id) else {
            return Ok(Self::json_error("Invalid endpoint ID", StatusCode::BAD_REQUEST));
        };

        match manager.delete_endpoint(uuid).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::NOT_FOUND)),
        }
    }

    /// Handle store credential
    async fn handle_store_credential(
        &self,
        req: Request<IncomingBody>,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Parse endpoint ID from path
        let endpoint_id = path
            .strip_prefix("/api/credentials/endpoints/")
            .and_then(|s| s.strip_suffix("/credential"))
            .unwrap_or("");
        let Ok(endpoint_uuid) = uuid::Uuid::parse_str(endpoint_id) else {
            return Ok(Self::json_error("Invalid endpoint ID", StatusCode::BAD_REQUEST));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct StoreCredentialRequest {
            credential_type: String,
            secret: String,  // Base64 encoded
        }

        let request: StoreCredentialRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        let credential_type = match request.credential_type.as_str() {
            "bearer_token" => rockbot_credentials::CredentialType::BearerToken,
            "api_key" => rockbot_credentials::CredentialType::ApiKey {
                header_name: "Authorization".to_string(),
            },
            "basic_auth" => rockbot_credentials::CredentialType::BasicAuth {
                username: String::new(),
            },
            _ => return Ok(Self::json_error("Invalid credential type", StatusCode::BAD_REQUEST)),
        };

        // Decode base64 secret
        let Ok(secret) = base64_decode(&request.secret) else {
            return Ok(Self::json_error("Invalid base64 secret", StatusCode::BAD_REQUEST));
        };

        match manager.store_credential(endpoint_uuid, credential_type, &secret).await {
            Ok(()) => {
                // Refresh provider availability cache after credential change
                self.refresh_provider_status().await;
                Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK))
            }
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle list pending approvals
    async fn handle_list_approvals(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let approvals = manager.list_pending_approvals().await;
        let body = serde_json::to_string(&approvals).unwrap();
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle approval response
    async fn handle_approval_response(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        let response: rockbot_credentials::HilApprovalResponse = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
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
        let vault_exists = rockbot_credentials::CredentialVault::exists(&self.credentials_config.vault_path);
        
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
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct UnlockRequest {
            password: String,
        }

        let request: UnlockRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        let salt = generate_salt();
        let master_key = match MasterKey::derive_from_password(&request.password, &salt) {
            Ok(key) => key,
            Err(e) => return Ok(Self::json_error(&format!("Failed to derive key: {e}"), StatusCode::BAD_REQUEST)),
        };

        match manager.unlock(master_key).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"unlocked"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle lock vault
    async fn handle_lock_vault(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        match manager.lock().await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"locked"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle init vault - creates a new vault if one doesn't exist
    async fn handle_init_vault(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        // Check if credentials are enabled in config
        if !self.credentials_config.enabled {
            return Ok(Self::json_error("Credential management not enabled in config", StatusCode::SERVICE_UNAVAILABLE));
        }

        // Check if vault already exists
        if rockbot_credentials::CredentialVault::exists(&self.credentials_config.vault_path) {
            return Ok(Self::json_error("Vault already exists. Use unlock instead.", StatusCode::CONFLICT));
        }

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        let method = request.method.as_deref().unwrap_or("password");

        match method {
            "password" => {
                let password = match &request.password {
                    Some(p) if p.len() >= 8 => p.clone(),
                    Some(_) => return Ok(Self::json_error("Password must be at least 8 characters", StatusCode::BAD_REQUEST)),
                    None => return Ok(Self::json_error("Password is required for password method", StatusCode::BAD_REQUEST)),
                };

                match rockbot_credentials::CredentialVault::init_with_password(&self.credentials_config.vault_path, &password) {
                    Ok(_) => {
                        info!("Vault initialized with password at {}", self.credentials_config.vault_path.display());
                        Ok(Self::json_response(r#"{"status":"initialized","method":"password"}"#, StatusCode::CREATED))
                    }
                    Err(e) => Ok(Self::json_error(&format!("Failed to initialize vault: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            }
            "keyfile" => {
                use std::os::unix::fs::OpenOptionsExt;
                
                let keyfile_path = request.keyfile_path.map_or_else(|| {
                        self.credentials_config.vault_path.parent()
                            .unwrap_or(std::path::Path::new("."))
                            .join("vault.key")
                    }, std::path::PathBuf::from);

                // Create parent directory if needed
                if let Some(parent) = keyfile_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
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
                                return Ok(Self::json_error(&format!("Failed to write keyfile: {e}"), StatusCode::INTERNAL_SERVER_ERROR));
                            }
                        }
                        Err(e) => return Ok(Self::json_error(&format!("Failed to create keyfile: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
                    }
                }

                match rockbot_credentials::CredentialVault::init_with_keyfile(&self.credentials_config.vault_path, &keyfile_path) {
                    Ok(_) => {
                        info!("Vault initialized with keyfile at {}", self.credentials_config.vault_path.display());
                        let body = serde_json::json!({
                            "status": "initialized",
                            "method": "keyfile",
                            "keyfile_path": keyfile_path.display().to_string(),
                        });
                        Ok(Self::json_response(&body.to_string(), StatusCode::CREATED))
                    }
                    Err(e) => Ok(Self::json_error(&format!("Failed to initialize vault: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            }
            _ => Ok(Self::json_error("Invalid method. Use 'password' or 'keyfile'", StatusCode::BAD_REQUEST)),
        }
    }

    /// Handle list permissions
    async fn handle_list_permissions(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let permissions = manager.list_path_permissions().await;
        let body = serde_json::to_string(&permissions).unwrap();
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle add permission
    async fn handle_add_permission(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct AddPermissionRequest {
            path_pattern: String,
            level: String,
            description: Option<String>,
        }

        let request: AddPermissionRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        let level = match request.level.as_str() {
            "allow" => rockbot_credentials::PermissionLevel::Allow,
            "allow_hil" | "hil" => rockbot_credentials::PermissionLevel::AllowHil,
            "allow_hil_2fa" | "hil_2fa" => rockbot_credentials::PermissionLevel::AllowHil2fa,
            "deny" => rockbot_credentials::PermissionLevel::Deny,
            _ => return Ok(Self::json_error("Invalid permission level. Use: allow, allow_hil, allow_hil_2fa, deny", StatusCode::BAD_REQUEST)),
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
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let permission_id = path.strip_prefix("/api/credentials/permissions/").unwrap_or("");
        let Ok(uuid) = uuid::Uuid::parse_str(permission_id) else {
            return Ok(Self::json_error("Invalid permission ID", StatusCode::BAD_REQUEST));
        };

        if manager.remove_permission(uuid).await {
            Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK))
        } else {
            Ok(Self::json_error("Permission not found", StatusCode::NOT_FOUND))
        }
    }

    /// Handle get audit log
    async fn handle_get_audit_log(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Parse limit from query string
        let limit = req.uri().query()
            .and_then(|q| {
                q.split('&')
                    .find_map(|pair| {
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
        let body = serde_json::to_string(&entries).unwrap();
        Ok(Self::json_response(&body, StatusCode::OK))
    }

    /// Handle approve HIL request
    async fn handle_approve_request(
        &self,
        path: &str,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/approve
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/approve"))
            .unwrap_or("");
        
        let Ok(uuid) = uuid::Uuid::parse_str(request_id) else {
            return Ok(Self::json_error("Invalid request ID", StatusCode::BAD_REQUEST));
        };

        // Parse optional body for resolved_by
        let resolved_by = if let Ok(collected) = req.collect().await {
            let body = collected.to_bytes();
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
            Ok(()) => Ok(Self::json_response(r#"{"status":"approved"}"#, StatusCode::OK)),
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
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/deny
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/deny"))
            .unwrap_or("");
        
        let Ok(uuid) = uuid::Uuid::parse_str(request_id) else {
            return Ok(Self::json_error("Invalid request ID", StatusCode::BAD_REQUEST));
        };

        // Parse body for resolved_by and denial_reason
        let (resolved_by, denial_reason) = if let Ok(collected) = req.collect().await {
            let body = collected.to_bytes();
            if !body.is_empty() {
                #[derive(Deserialize)]
                struct DenyBody {
                    resolved_by: Option<String>,
                    reason: Option<String>,
                }
                let parsed = serde_json::from_slice::<DenyBody>(&body).ok();
                (
                    parsed.as_ref().and_then(|b| b.resolved_by.clone()).unwrap_or_else(|| "api".to_string()),
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
            Ok(()) => Ok(Self::json_response(r#"{"status":"denied"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
        }
    }

    // ==================== Web UI ====================

    /// Serve the web UI dashboard
    async fn handle_web_ui(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let html = crate::web_ui::get_dashboard_html();
        
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(GatewayBody::Left(Full::new(html.into())))
            .unwrap())
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
            .body(GatewayBody::Left(Full::new(serde_json::to_string(&json).unwrap().into())))
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
                .body(GatewayBody::Left(Full::new(r#"{"error":"LLM registry not initialized"}"#.into())))
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
                .body(GatewayBody::Left(Full::new(serde_json::to_string(&p).unwrap().into())))
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
                .body(GatewayBody::Left(Full::new(r#"{"error":"LLM registry not initialized"}"#.into())))
                .unwrap());
        };

        // Test provider: check credentials and list models
        let result = if let Some(provider) = reg.get_provider(provider_id) {
            let configured = provider.is_configured().await;
            let models = provider.list_models().await;
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
        let body = req.collect().await.unwrap().to_bytes();

        // Parse as raw JSON first to extract agent_id (not part of ChatCompletionRequest)
        let raw_json: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST));
            }
        };
        let agent_id = raw_json.get("agent_id").and_then(|v| v.as_str()).map(String::from);

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
                        let has_system = chat_req.messages.first()
                            .map_or(false, |m| matches!(m.role, rockbot_llm::MessageRole::System));
                        if !has_system {
                            chat_req.messages.insert(0, rockbot_llm::Message {
                                role: rockbot_llm::MessageRole::System,
                                content: system_prompt.clone(),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    }
                } else {
                    // Try loading from the agent's SYSTEM-PROMPT.md file
                    let agent_dir = dirs::config_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join("rockbot/agents")
                        .join(agent_id)
                        .join("SYSTEM-PROMPT.md");
                    if let Ok(content) = tokio::fs::read_to_string(&agent_dir).await {
                        if !content.trim().is_empty() {
                            let has_system = chat_req.messages.first()
                                .map_or(false, |m| matches!(m.role, rockbot_llm::MessageRole::System));
                            if !has_system {
                                chat_req.messages.insert(0, rockbot_llm::Message {
                                    role: rockbot_llm::MessageRole::System,
                                    content: content.trim().to_string(),
                                    tool_calls: None,
                                    tool_call_id: None,
                                });
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
                .body(GatewayBody::Left(Full::new(r#"{"error":"LLM registry not initialized"}"#.into())))
                .unwrap());
        };

        // Resolve "default" model to the first available provider's first model
        // Resolve "default" model to the first configured provider's first model
        if chat_req.model == "default" {
            let configured_cache = self.provider_configured.read().await;
            for provider_id in reg.list_providers() {
                if provider_id == "mock" { continue; }
                if !configured_cache.get(&provider_id).copied().unwrap_or(false) { continue; }
                if let Some(provider) = reg.get_provider(&provider_id) {
                    if let Ok(models) = provider.list_models().await {
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

        info!("Chat request: model={}, messages={}, agent={}", chat_req.model, chat_req.messages.len(), agent_id.as_deref().unwrap_or("none"));

        match provider.chat_completion(chat_req).await {
            Ok(response) => Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(GatewayBody::Left(Full::new(serde_json::to_string(&response).unwrap().into())))
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
                let models = provider
                    .list_models()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| ProviderModelInfo {
                        id: m.id,
                        name: m.name,
                        description: m.description,
                        context_window: m.context_window,
                        max_output_tokens: m.max_output_tokens,
                    })
                    .collect();

                let name = schema
                    .as_ref().map_or_else(|| provider_id.clone(), |s| s.provider_name.clone());
                let auth_type = schema
                    .as_ref()
                    .and_then(|s| s.auth_methods.first()).map_or_else(|| "none".to_string(), |m| m.id.clone());

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
            .body(GatewayBody::Left(Full::new(serde_json::to_string(&json).unwrap().into())))
            .unwrap())
    }



    // ==================== Session API Handlers ====================

    /// Handle list sessions (GET /api/sessions?agent_id=xxx)
    async fn handle_list_sessions(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
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

        let query = crate::session::SessionQuery {
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
            Err(e) => Ok(Self::json_error(&format!("Failed to query sessions: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle create session (POST /api/sessions)
    async fn handle_create_session(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let body = req.collect().await.unwrap_or_default().to_bytes();
        let body_str = String::from_utf8_lossy(&body);

        #[derive(Deserialize)]
        struct CreateSessionRequest {
            agent_id: Option<String>,
            model: Option<String>,
        }

        let parsed: CreateSessionRequest = match serde_json::from_str(&body_str) {
            Ok(v) => v,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        // Use agent_id if provided, otherwise "ad-hoc"
        let agent_id = parsed.agent_id.as_deref().unwrap_or("ad-hoc");
        let session_key = uuid::Uuid::new_v4().to_string();

        match self.session_manager.create_session(agent_id, &session_key).await {
            Ok(mut session) => {
                // Resolve model: use explicit model, or fall back to agent's configured model
                let model = parsed.model.or_else(|| {
                    let configs = self.agents_config.try_read().ok()?;
                    configs.iter()
                        .find(|c| c.id == agent_id)
                        .and_then(|c| c.model.clone())
                });
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
            Err(e) => Ok(Self::json_error(&format!("Failed to create session: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle delete session (DELETE /api/sessions/{id})
    async fn handle_delete_session(
        &self,
        path: &str,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let session_id = path.strip_prefix("/api/sessions/").unwrap_or("");
        if session_id.is_empty() {
            return Ok(Self::json_error("Missing session ID", StatusCode::BAD_REQUEST));
        }

        match self.session_manager.archive_session(session_id).await {
            Ok(()) => {
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new("{\"archived\":true}".into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(&format!("Failed to archive session: {e}"), StatusCode::NOT_FOUND)),
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
            return Ok(Self::json_error("Missing session ID", StatusCode::BAD_REQUEST));
        }

        match self.session_manager.get_message_history(session_id, Some(200), None).await {
            Ok(history) => {
                let json = serde_json::to_string(&history).unwrap_or_else(|_| r#"{"messages":[],"total_count":0}"#.to_string());
                Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap())
            }
            Err(e) => Ok(Self::json_error(&format!("Failed to get messages: {e}"), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    // ==================== Gateway Management Handlers ====================

    /// Handle reload agents request
    async fn handle_reload_agents(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        match self.reload_agents().await {
            Ok((created, pending)) => {
                let body = serde_json::json!({
                    "status": "ok",
                    "agents_created": created,
                    "agents_pending": pending,
                });
                Ok(Self::json_response(&body.to_string(), StatusCode::OK))
            }
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle list pending agents request
    async fn handle_list_pending_agents(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let pending = self.list_pending_agents().await;
        let pending_info: Vec<_> = pending.iter().map(|p| {
            serde_json::json!({
                "id": p.config.id,
                "model": p.config.model,
                "reason": p.reason,
            })
        }).collect();
        
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

    /// Get the directory path for an agent
    #[allow(clippy::unused_self)]
    fn agent_directory(&self, agent_id: &str) -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("rockbot")
            .join("agents")
            .join(agent_id)
    }

    /// Initialize an agent's directory with default files
    async fn initialize_agent_directory(
        &self,
        agent_dir: &std::path::Path,
        system_prompt: Option<&str>,
    ) -> std::result::Result<(), std::io::Error> {
        tokio::fs::create_dir_all(agent_dir).await?;

        let soul_path = agent_dir.join("SOUL.md");
        if !soul_path.exists() {
            tokio::fs::write(
                &soul_path,
                "# Agent Identity\n\n\
                 You are a capable autonomous agent. You accomplish tasks by taking direct action \
                 using your tools — never by describing what you would do.\n\n\
                 ## Principles\n\n\
                 - Act decisively. Start working immediately when given a task.\n\
                 - Be thorough. Complete every step before reporting results.\n\
                 - Be resilient. When something fails, analyze the error and try a different approach.\n\
                 - Be self-sufficient. Never ask the user to do something you can do with your tools.\n",
            ).await?;
        }

        let prompt_path = agent_dir.join("SYSTEM-PROMPT.md");
        if !prompt_path.exists() {
            let content = system_prompt.unwrap_or(
                "# System Prompt\n\nCustomize this agent's system prompt here.\n"
            );
            tokio::fs::write(&prompt_path, content).await?;
        }

        Ok(())
    }

    /// Persist a single new agent to the TOML config file
    async fn persist_agent_to_config(&self, agent: &crate::config::AgentInstance) {
        let Some(ref config_path) = self.config_path else { return };
        let config_path = config_path.clone();
        let agent = agent.clone();

        // Use toml_edit to append without disrupting existing content
        tokio::task::spawn_blocking(move || {
            let content = match std::fs::read_to_string(&config_path) {
                Ok(c) => c,
                Err(e) => { error!("Failed to read config for agent persist: {}", e); return; }
            };
            let mut doc: toml_edit::DocumentMut = match content.parse() {
                Ok(d) => d,
                Err(e) => { error!("Failed to parse config TOML: {}", e); return; }
            };

            if !doc.contains_key("agents") {
                doc["agents"] = toml_edit::Item::Table(toml_edit::Table::new());
            }

            let mut new_agent = toml_edit::Table::new();
            new_agent["id"] = toml_edit::value(&agent.id);
            if let Some(ref model) = agent.model {
                new_agent["model"] = toml_edit::value(model);
            }
            if let Some(ref parent_id) = agent.parent_id {
                new_agent["parent_id"] = toml_edit::value(parent_id);
            }
            if let Some(ref workspace) = agent.workspace {
                new_agent["workspace"] = toml_edit::value(workspace.display().to_string());
            }
            if let Some(max_tool_calls) = agent.max_tool_calls {
                new_agent["max_tool_calls"] = toml_edit::value(max_tool_calls as i64);
            }
            if let Some(temperature) = agent.temperature {
                new_agent["temperature"] = toml_edit::value(temperature as f64);
            }
            if let Some(max_tokens) = agent.max_tokens {
                new_agent["max_tokens"] = toml_edit::value(max_tokens as i64);
            }
            if let Some(ref system_prompt) = agent.system_prompt {
                new_agent["system_prompt"] = toml_edit::value(system_prompt);
            }
            if !agent.enabled {
                new_agent["enabled"] = toml_edit::value(false);
            }

            if let Some(list) = doc["agents"]["list"].as_array_of_tables_mut() {
                list.push(new_agent);
            } else {
                let mut arr = toml_edit::ArrayOfTables::new();
                arr.push(new_agent);
                doc["agents"]["list"] = toml_edit::Item::ArrayOfTables(arr);
            }

            if let Err(e) = std::fs::write(&config_path, doc.to_string()) {
                error!("Failed to write config after agent persist: {}", e);
            }
        }).await.ok();
    }

    /// Persist all agents to the TOML config file (full rewrite of agents section)
    async fn persist_all_agents_to_config(&self, agents: &[crate::config::AgentInstance]) {
        let Some(ref config_path) = self.config_path else { return };
        let config_path = config_path.clone();
        let agents = agents.to_vec();

        tokio::task::spawn_blocking(move || {
            let content = match std::fs::read_to_string(&config_path) {
                Ok(c) => c,
                Err(e) => { error!("Failed to read config for agents persist: {}", e); return; }
            };
            let mut doc: toml_edit::DocumentMut = match content.parse() {
                Ok(d) => d,
                Err(e) => { error!("Failed to parse config TOML: {}", e); return; }
            };

            if !doc.contains_key("agents") {
                doc["agents"] = toml_edit::Item::Table(toml_edit::Table::new());
            }

            // Rebuild the [[agents.list]] array
            let mut arr = toml_edit::ArrayOfTables::new();
            for agent in &agents {
                let mut t = toml_edit::Table::new();
                t["id"] = toml_edit::value(&agent.id);
                if let Some(ref model) = agent.model {
                    t["model"] = toml_edit::value(model);
                }
                if let Some(ref parent_id) = agent.parent_id {
                    t["parent_id"] = toml_edit::value(parent_id);
                }
                if let Some(ref workspace) = agent.workspace {
                    t["workspace"] = toml_edit::value(workspace.display().to_string());
                }
                if let Some(max_tool_calls) = agent.max_tool_calls {
                    t["max_tool_calls"] = toml_edit::value(max_tool_calls as i64);
                }
                if let Some(temperature) = agent.temperature {
                    t["temperature"] = toml_edit::value(temperature as f64);
                }
                if let Some(max_tokens) = agent.max_tokens {
                    t["max_tokens"] = toml_edit::value(max_tokens as i64);
                }
                if let Some(ref system_prompt) = agent.system_prompt {
                    t["system_prompt"] = toml_edit::value(system_prompt);
                }
                if !agent.enabled {
                    t["enabled"] = toml_edit::value(false);
                }
                arr.push(t);
            }
            doc["agents"]["list"] = toml_edit::Item::ArrayOfTables(arr);

            if let Err(e) = std::fs::write(&config_path, doc.to_string()) {
                error!("Failed to write config after agents persist: {}", e);
            }
        }).await.ok();
    }

    /// Handle WebSocket upgrade request
    async fn handle_websocket_upgrade(
        &self,
        _req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        // For simplicity, we'll return an error for now
        // In a full implementation, this would handle the WebSocket upgrade protocol
        Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(GatewayBody::Left(Full::new("WebSocket upgrade not implemented in this demo".into())))
            .unwrap())
    }
    
    /// Handle health check endpoint
    async fn handle_health_check(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let health = self.get_health_status().await;
        let body = serde_json::to_string(&health).unwrap();
        
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }
    
    /// `GET /api/metrics` — return basic runtime metrics as JSON.
    async fn handle_metrics(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let agents = self.agents.read().await;
        let agent_count = agents.len() as u64;
        drop(agents);

        crate::metrics::set_active_agents(agent_count);

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
    async fn handle_list_agents(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let active = self.agents.read().await;
        let pending = self.pending_agents.read().await;
        let configs = self.agents_config.read().await;

        let mut seen = std::collections::HashSet::new();
        let mut agent_list: Vec<serde_json::Value> = Vec::new();

        // Active agents — get session count from session manager
        for (id, agent) in active.iter() {
            seen.insert(id.clone());
            let session_count = self.session_manager
                .query_sessions(crate::session::SessionQuery {
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
                "system_prompt": cfg.system_prompt,
                "workspace": cfg.workspace.as_ref().map(|p| p.display().to_string()),
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
                    "system_prompt": p.config.system_prompt,
                    "workspace": p.config.workspace.as_ref().map(|p| p.display().to_string()),
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
                let status = if cfg.enabled { "configured" } else { "disabled" };
                agent_list.push(serde_json::json!({
                    "id": cfg.id,
                    "status": status,
                    "model": cfg.model,
                    "parent_id": cfg.parent_id,
                    "system_prompt": cfg.system_prompt,
                    "workspace": cfg.workspace.as_ref().map(|p| p.display().to_string()),
                    "max_tool_calls": cfg.max_tool_calls,
                    "temperature": cfg.temperature,
                    "max_tokens": cfg.max_tokens,
                    "enabled": cfg.enabled,
                    "session_count": 0,
                }));
            }
        }

        let body = serde_json::to_string(&agent_list).unwrap();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    /// Handle create agent request
    async fn handle_create_agent(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read body", StatusCode::BAD_REQUEST)),
        };

        fn default_enabled() -> bool { true }

        #[derive(Deserialize)]
        struct CreateAgentRequest {
            id: String,
            model: Option<String>,
            parent_id: Option<String>,
            workspace: Option<String>,
            max_tool_calls: Option<u32>,
            temperature: Option<f32>,
            max_tokens: Option<u32>,
            system_prompt: Option<String>,
            #[serde(default = "default_enabled")]
            enabled: bool,
        }

        let req: CreateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        if req.id.trim().is_empty() {
            return Ok(Self::json_error("Agent ID is required", StatusCode::BAD_REQUEST));
        }

        // Check if agent already exists (active or in config)
        let agents = self.agents.read().await;
        if agents.contains_key(&req.id) {
            return Ok(Self::json_error(&format!("Agent '{}' already exists", req.id), StatusCode::CONFLICT));
        }
        drop(agents);

        let configs = self.agents_config.read().await;
        if configs.iter().any(|c| c.id == req.id) {
            return Ok(Self::json_error(&format!("Agent '{}' already exists in config", req.id), StatusCode::CONFLICT));
        }
        drop(configs);

        let config = crate::config::AgentInstance {
            id: req.id.clone(),
            model: req.model,
            workspace: req.workspace.map(std::path::PathBuf::from),
            max_tool_calls: req.max_tool_calls,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            parent_id: req.parent_id,
            system_prompt: req.system_prompt.clone(),
            enabled: req.enabled,
            mcp_servers: std::collections::HashMap::new(),
            config: std::collections::HashMap::new(),
        };

        // Create agent directory with SOUL.md and SYSTEM-PROMPT.md
        let agent_dir = self.agent_directory(&config.id);
        if let Err(e) = self.initialize_agent_directory(&agent_dir, req.system_prompt.as_deref()).await {
            error!("Failed to create agent directory: {}", e);
            // Non-fatal — continue creating the agent
        }

        // Persist to config file
        self.persist_agent_to_config(&config).await;

        // Add to our in-memory config list
        self.agents_config.write().await.push(config.clone());

        // Try to create the agent via factory
        let status = if let Some(ref factory) = self.agent_factory {
            match factory(config.clone()).await {
                Ok(agent) => {
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

        let body = serde_json::json!({ "status": status, "id": req.id });
        let code = if status == "created" { StatusCode::CREATED } else { StatusCode::ACCEPTED };
        Ok(Self::json_response(&body.to_string(), code))
    }

    /// Handle update agent request
    async fn handle_update_agent(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path.strip_prefix("/api/agents/").unwrap_or("").to_string();

        if agent_id.is_empty() {
            return Ok(Self::json_error("Invalid agent ID", StatusCode::BAD_REQUEST));
        }

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct UpdateAgentRequest {
            model: Option<String>,
            parent_id: Option<String>,
            workspace: Option<String>,
            max_tool_calls: Option<u32>,
            temperature: Option<f32>,
            max_tokens: Option<u32>,
            system_prompt: Option<String>,
            enabled: Option<bool>,
        }

        let update: UpdateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {e}"), StatusCode::BAD_REQUEST)),
        };

        // Check if agent exists in config or runtime
        let mut configs = self.agents_config.write().await;
        let config_entry = configs.iter_mut().find(|c| c.id == agent_id);

        let agents = self.agents.read().await;
        let exists_active = agents.contains_key(&agent_id);
        drop(agents);

        if config_entry.is_none() && !exists_active {
            return Ok(Self::json_error(&format!("Agent '{agent_id}' not found"), StatusCode::NOT_FOUND));
        }

        // Update in-memory config
        if let Some(cfg) = config_entry {
            if let Some(model) = &update.model {
                cfg.model = if model.is_empty() { None } else { Some(model.clone()) };
            }
            if let Some(parent_id) = &update.parent_id {
                cfg.parent_id = if parent_id.is_empty() { None } else { Some(parent_id.clone()) };
            }
            if let Some(workspace) = &update.workspace {
                cfg.workspace = if workspace.is_empty() { None } else { Some(std::path::PathBuf::from(workspace)) };
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
                cfg.system_prompt = if system_prompt.is_empty() { None } else { Some(system_prompt.clone()) };
                // Also update SYSTEM-PROMPT.md in agent directory
                let agent_dir = self.agent_directory(&agent_id);
                let prompt_path = agent_dir.join("SYSTEM-PROMPT.md");
                let _ = tokio::fs::write(&prompt_path, system_prompt).await;
            }
            if let Some(enabled) = update.enabled {
                cfg.enabled = enabled;
            }
        }

        // Persist all configs to file
        // Grab the updated config for potential agent recreation
        let updated_config = configs.iter().find(|c| c.id == agent_id).cloned();
        let configs_snapshot: Vec<_> = configs.iter().cloned().collect();
        drop(configs);
        self.persist_all_agents_to_config(&configs_snapshot).await;

        // Recreate the running agent instance so it picks up config changes (e.g. model)
        if let (Some(ref factory), Some(cfg)) = (&self.agent_factory, updated_config) {
            match factory(cfg).await {
                Ok(new_agent) => {
                    let mut agents = self.agents.write().await;
                    agents.insert(agent_id.clone(), new_agent);
                    info!("Recreated agent '{}' with updated config", agent_id);
                }
                Err(e) => {
                    warn!("Could not recreate agent '{}' after config update: {}", agent_id, e);
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

        if agent_id.is_empty() {
            return Ok(Self::json_error("Invalid agent ID", StatusCode::BAD_REQUEST));
        }

        let removed_active = self.agents.write().await.remove(&agent_id);
        self.pending_agents.write().await.retain(|p| p.config.id != agent_id);

        // Remove from config
        let mut configs = self.agents_config.write().await;
        let had_config = configs.len();
        configs.retain(|c| c.id != agent_id);
        let removed_config = configs.len() < had_config;
        let configs_snapshot: Vec<_> = configs.iter().cloned().collect();
        drop(configs);

        if removed_config {
            self.persist_all_agents_to_config(&configs_snapshot).await;
        }

        if removed_active.is_some() || removed_config {
            let body = serde_json::json!({ "status": "deleted", "id": agent_id });
            Ok(Self::json_response(&body.to_string(), StatusCode::OK))
        } else {
            Ok(Self::json_error(&format!("Agent '{agent_id}' not found"), StatusCode::NOT_FOUND))
        }
    }
    
    /// Handle agent message via HTTP API
    async fn handle_agent_message(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path.strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/message"))
            .unwrap_or("");
        
        if agent_id.is_empty() {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }
        
        // Keep a copy of agent_id since path will be consumed
        let agent_id = agent_id.to_string();
        
        // Parse request body
        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Failed to read request body".into())))
                    .unwrap());
            }
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
        
        // Process message
        match self.process_agent_message(&agent_id, message_request).await {
            Ok(response) => {
                let body = serde_json::to_string(&response).unwrap();
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
                let body = serde_json::to_string(&error_response).unwrap();
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
        let agent_id = path.strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/stream"))
            .unwrap_or("");

        if agent_id.is_empty() {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }

        let agent_id = agent_id.to_string();

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Failed to read request body".into())))
                    .unwrap());
            }
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
                    return Ok(Self::json_error(&format!("Agent '{agent_id}' not found"), StatusCode::NOT_FOUND));
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
        let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<rockbot_llm::StreamingChunk>(128);

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
                    break; // Client disconnected
                }
            }
        });

        // Spawn the agent processing task
        tokio::spawn(async move {
            let result = agent.process_message_streaming(
                session_id, message, workspace, stream_tx,
            ).await;

            // Send final event with the complete AgentResponse
            match result {
                Ok(response) => {
                    if let Ok(json) = serde_json::to_string(&response) {
                        let event = format!("event: done\ndata: {json}\n\n");
                        let _ = sse_tx.send(Ok(Frame::data(hyper::body::Bytes::from(event)))).await;
                    }
                }
                Err(e) => {
                    let error_json = serde_json::json!({"error": e.to_string()});
                    let event = format!("event: error\ndata: {error_json}\n\n");
                    let _ = sse_tx.send(Ok(Frame::data(hyper::body::Bytes::from(event)))).await;
                }
            }
            // Channel drops, stream ends
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
        let agent = agents.get(agent_id)
            .ok_or_else(|| GatewayError::InvalidRequest {
                message: format!("Agent '{agent_id}' not found"),
            })?;
        
        // Create session ID from session key
        let session_id = format!("{}:{}", agent_id, request.session_key);
        
        // Convert request to message
        let message = Message::text(request.message)
            .with_session_id(&session_id)
            .with_role(MessageRole::User);
        
        // Process message with optional workspace override from the client
        agent.process_message(session_id, message, request.workspace.map(std::path::PathBuf::from)).await
    }
    
    /// Get gateway health status
    async fn get_health_status(&self) -> GatewayHealth {
        let agents = self.agents.read().await;
        let connections = self.ws_connections.read().await;
        
        let mut agent_health = Vec::new();
        for agent in agents.values() {
            if let Ok(health) = agent.health_check().await {
                agent_health.push(health);
            }
        }
        
        // Get session statistics
        let session_stats = self.session_manager.get_statistics().await
            .unwrap_or(crate::session::SessionStatistics {
                total_sessions: 0,
                active_sessions: 0,
                total_messages: 0,
                total_tokens: 0,
            });
        
        let pending = self.pending_agents.read().await;

        GatewayHealth {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: 0, // TODO: Track actual uptime
            uptime_secs: 0,
            active_connections: connections.len(),
            active_sessions: session_stats.active_sessions as usize,
            pending_agents: pending.len(),
            agents: agent_health,
            memory_usage: MemoryUsage {
                allocated_bytes: 0, // TODO: Get actual memory usage
                heap_size_bytes: 0,
            },
        }
    }
    
    /// Shutdown the gateway
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down gateway");
        let _ = self.shutdown_tx.send(());
        
        // Close all WebSocket connections
        let connections = self.ws_connections.read().await;
        for connection in connections.values() {
            let _ = connection.sender.send(WsMessage::Close(None));
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
        let agent_id = path.strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/approve"))
            .unwrap_or("");

        if agent_id.is_empty() {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(GatewayBody::Left(Full::new("Invalid agent ID".into())))
                .unwrap());
        }

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Failed to read request body".into())))
                    .unwrap());
            }
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

        let status = if approval.approved { "approved" } else { "denied" };
        let response_json = serde_json::to_string(&serde_json::json!({
            "status": status,
            "request_id": approval.request_id,
            "agent_id": agent_id,
        })).unwrap_or_default();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(response_json.into())))
            .unwrap())
    }

    /// `GET /.well-known/agent.json` — serve the A2A agent card for discovery.
    async fn handle_agent_card(
        &self,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let base_url = format!("http://{}:{}", self.config.bind_host, self.config.port);
        let card = crate::a2a::build_agent_card(
            "rockbot",
            "RockBot AI Gateway",
            &base_url,
            true,
        );
        let body = serde_json::to_string(&card).unwrap_or_default();
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(GatewayBody::Left(Full::new(body.into())))
            .unwrap())
    }

    /// `POST /a2a` — JSON-RPC 2.0 dispatch for A2A protocol.
    async fn handle_a2a_request(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<GatewayBody>, hyper::Error> {
        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(GatewayBody::Left(Full::new("Failed to read request body".into())))
                    .unwrap());
            }
        };

        let rpc_request: crate::a2a::JsonRpcRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                let resp = crate::a2a::JsonRpcResponse::error(
                    None,
                    -32700,
                    "Parse error: invalid JSON",
                );
                let json = serde_json::to_string(&resp).unwrap_or_default();
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(GatewayBody::Left(Full::new(json.into())))
                    .unwrap());
            }
        };

        let dispatcher = crate::a2a::A2ADispatcher::new(Arc::clone(&self.a2a_task_store));
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
            credentials_config: self.credentials_config.clone(),
            config_path: self.config_path.clone(),
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
            shutdown_tx: self.shutdown_tx.clone(),
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

#[async_trait::async_trait]
impl rockbot_tools::AgentInvoker for GatewayInvoker {
    async fn invoke_agent(
        &self,
        agent_id: &str,
        message: &str,
        session_id: &str,
        _depth: u32,
    ) -> std::result::Result<String, rockbot_tools::ToolError> {
        let agents = self.agents.read().await;
        let agent = agents.get(agent_id).ok_or_else(|| {
            rockbot_tools::ToolError::ExecutionFailed {
                message: format!("invoke_agent: agent '{agent_id}' not found"),
            }
        })?;

        let msg = crate::message::Message::text(message)
            .with_session_id(session_id)
            .with_role(crate::message::MessageRole::User);

        match agent.process_message(session_id.to_string(), msg, None).await {
            Ok(response) => {
                let text = match &response.message.content {
                    crate::message::MessageContent::Text { text } => text.clone(),
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
}

/// HTTP API error response
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::config::{AgentConfig, AgentDefaults, AgentInstance, ToolConfig, SecurityConfig, SandboxConfig, ProvidersConfig};
    use std::collections::HashMap;
    use tempfile::NamedTempFile;
    
    async fn create_test_gateway() -> Gateway {
        let temp_db = NamedTempFile::new().unwrap();
        let session_manager = Arc::new(
            SessionManager::new(temp_db.path(), 100).await.unwrap()
        );
        
        let config = Config {
            gateway: GatewayConfig {
                bind_host: "127.0.0.1".to_string(),
                port: 0, // Use 0 for testing to avoid port conflicts
                max_connections: 100,
                request_timeout: 30,
                require_api_key: None,
            },
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
                },
                capabilities: Default::default(),
            },
            credentials: CredentialsConfig::default(),
            providers: ProvidersConfig::default(),
        };

        Gateway::new(config, session_manager).await.unwrap()
    }
    
    #[tokio::test]
    async fn test_gateway_creation() {
        let gateway = create_test_gateway().await;
        let health = gateway.get_health_status().await;
        
        assert_eq!(health.active_connections, 0);
        assert_eq!(health.agents.len(), 0);
    }
    
    #[tokio::test]
    async fn test_health_endpoint() {
        let gateway = create_test_gateway().await;
        let response = gateway.handle_health_check().await.unwrap();
        
        assert_eq!(response.status(), StatusCode::OK);
    }
}