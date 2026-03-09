//! Gateway server for Krabbykrus
//!
//! This module provides the main gateway server that handles WebSocket connections,
//! HTTP API endpoints, and coordinates agent execution.

use crate::agent::{Agent, AgentResponse};
use crate::config::{Config, CredentialsConfig, GatewayConfig};
use krabbykrus_credentials::{CredentialManager, MasterKey, generate_salt, PermissionLevel, PathPermission, HilApprovalResponse};

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
use http_body_util::{BodyExt, Full};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info};

/// Pending agent info (for agents that couldn't be created due to missing credentials)
#[derive(Debug, Clone)]
pub struct PendingAgent {
    pub config: crate::config::AgentInstance,
    pub reason: String,
}

/// Agent factory callback for creating agents
pub type AgentFactory = Arc<dyn Fn(crate::config::AgentInstance) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Arc<Agent>>> + Send>> + Send + Sync>;

/// Main gateway server
pub struct Gateway {
    /// Gateway configuration
    config: GatewayConfig,
    /// Credentials configuration
    credentials_config: CredentialsConfig,
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
    /// Active WebSocket connections
    ws_connections: Arc<RwLock<HashMap<String, WsConnection>>>,
    /// Shutdown broadcast channel
    shutdown_tx: broadcast::Sender<()>,
}

/// WebSocket connection information
struct WsConnection {
    id: String,
    sender: tokio::sync::mpsc::UnboundedSender<WsMessage>,
    user_id: Option<String>,
    connected_at: std::time::Instant,
}

/// WebSocket message types
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
    pub active_connections: usize,
    pub active_sessions: usize,
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
            if !krabbykrus_credentials::CredentialVault::exists(&config.credentials.vault_path) {
                info!(
                    "Credential vault not initialized at {}. Use 'krabbykrus credentials init' or the TUI to set up.",
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
        
        Ok(Self {
            config: config.gateway,
            credentials_config: config.credentials,
            agents: Arc::new(RwLock::new(HashMap::new())),
            pending_agents: Arc::new(RwLock::new(Vec::new())),
            agent_factory: None,
            session_manager,
            credential_manager,
            ws_connections: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx,
        })
    }

    /// Initialize the credential manager based on configuration
    async fn init_credential_manager(config: &CredentialsConfig) -> Result<CredentialManager> {
        let manager = CredentialManager::new(&config.vault_path)
            .map_err(|e| GatewayError::InvalidRequest {
                message: format!("Failed to open credential vault: {}", e),
            })?;
        
        // Auto-unlock if configured
        match config.unlock_method.as_str() {
            "env" => {
                if let Ok(password) = std::env::var(&config.password_env_var) {
                    let salt = generate_salt();
                    let master_key = MasterKey::derive_from_password(&password, &salt)
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to derive master key: {}", e),
                        })?;
                    manager.unlock(master_key).await
                        .map_err(|e| GatewayError::InvalidRequest {
                            message: format!("Failed to unlock vault: {}", e),
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
    
    /// Set the agent factory for creating new agents
    pub fn set_agent_factory(&mut self, factory: AgentFactory) {
        self.agent_factory = Some(factory);
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
            error!("Error serving connection: {:?}", err);
        }
        
        Ok(())
    }
    
    /// Handle HTTP request (which may be upgraded to WebSocket)
    async fn handle_request(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let path = req.uri().path().to_string();
        
        match (req.method(), path.as_str()) {
            // Web UI
            (&Method::GET, "/") | (&Method::GET, "/index.html") => {
                self.handle_web_ui().await
            }
            (&Method::GET, "/ws") => {
                self.handle_websocket_upgrade(req).await
            }
            (&Method::GET, "/health") => {
                self.handle_health_check().await
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
                let endpoint_path = format!("/api/credentials/endpoints/{}", id);
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
            // Gateway management
            (&Method::POST, "/api/gateway/reload") => {
                self.handle_reload_agents().await
            }
            (&Method::GET, "/api/gateway/pending") => {
                self.handle_list_pending_agents().await
            }
            (&Method::POST, p) if p.starts_with("/api/agents/") => {
                self.handle_agent_message(req).await
            }
            _ => {
                Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new("Not Found".into()))
                    .unwrap())
            }
        }
    }

    // ==================== Credentials API Handlers ====================

    /// Handle list endpoints
    async fn handle_list_endpoints(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        let endpoint_type = match request.endpoint_type.as_str() {
            "home_assistant" => krabbykrus_credentials::EndpointType::HomeAssistant,
            "gmail" => krabbykrus_credentials::EndpointType::Gmail,
            "spotify" => krabbykrus_credentials::EndpointType::Spotify,
            "generic_rest" => krabbykrus_credentials::EndpointType::GenericRest,
            "generic_oauth2" => krabbykrus_credentials::EndpointType::GenericOAuth2,
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let endpoint_id = path.strip_prefix("/api/credentials/endpoints/").unwrap_or("");
        let uuid = match uuid::Uuid::parse_str(endpoint_id) {
            Ok(id) => id,
            Err(_) => return Ok(Self::json_error("Invalid endpoint ID", StatusCode::BAD_REQUEST)),
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Parse endpoint ID from path
        let endpoint_id = path
            .strip_prefix("/api/credentials/endpoints/")
            .and_then(|s| s.strip_suffix("/credential"))
            .unwrap_or("");
        let endpoint_uuid = match uuid::Uuid::parse_str(endpoint_id) {
            Ok(id) => id,
            Err(_) => return Ok(Self::json_error("Invalid endpoint ID", StatusCode::BAD_REQUEST)),
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        let credential_type = match request.credential_type.as_str() {
            "bearer_token" => krabbykrus_credentials::CredentialType::BearerToken,
            _ => return Ok(Self::json_error("Invalid credential type", StatusCode::BAD_REQUEST)),
        };

        // Decode base64 secret
        let secret = match base64_decode(&request.secret) {
            Ok(s) => s,
            Err(_) => return Ok(Self::json_error("Invalid base64 secret", StatusCode::BAD_REQUEST)),
        };

        match manager.store_credential(endpoint_uuid, credential_type, &secret).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle list pending approvals
    async fn handle_list_approvals(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read request body", StatusCode::BAD_REQUEST)),
        };

        let response: krabbykrus_credentials::HilApprovalResponse = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        match manager.respond_to_approval(response).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"ok"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::BAD_REQUEST)),
        }
    }

    /// Handle credentials status
    async fn handle_credentials_status(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let vault_exists = krabbykrus_credentials::CredentialVault::exists(&self.credentials_config.vault_path);
        
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        let salt = generate_salt();
        let master_key = match MasterKey::derive_from_password(&request.password, &salt) {
            Ok(key) => key,
            Err(e) => return Ok(Self::json_error(&format!("Failed to derive key: {}", e), StatusCode::BAD_REQUEST)),
        };

        match manager.unlock(master_key).await {
            Ok(()) => Ok(Self::json_response(r#"{"status":"unlocked"}"#, StatusCode::OK)),
            Err(e) => Ok(Self::json_error(&e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)),
        }
    }

    /// Handle lock vault
    async fn handle_lock_vault(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        // Check if credentials are enabled in config
        if !self.credentials_config.enabled {
            return Ok(Self::json_error("Credential management not enabled in config", StatusCode::SERVICE_UNAVAILABLE));
        }

        // Check if vault already exists
        if krabbykrus_credentials::CredentialVault::exists(&self.credentials_config.vault_path) {
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        let method = request.method.as_deref().unwrap_or("password");

        match method {
            "password" => {
                let password = match &request.password {
                    Some(p) if p.len() >= 8 => p.clone(),
                    Some(_) => return Ok(Self::json_error("Password must be at least 8 characters", StatusCode::BAD_REQUEST)),
                    None => return Ok(Self::json_error("Password is required for password method", StatusCode::BAD_REQUEST)),
                };

                match krabbykrus_credentials::CredentialVault::init_with_password(&self.credentials_config.vault_path, &password) {
                    Ok(_) => {
                        info!("Vault initialized with password at {}", self.credentials_config.vault_path.display());
                        Ok(Self::json_response(r#"{"status":"initialized","method":"password"}"#, StatusCode::CREATED))
                    }
                    Err(e) => Ok(Self::json_error(&format!("Failed to initialize vault: {}", e), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            }
            "keyfile" => {
                use std::os::unix::fs::OpenOptionsExt;
                
                let keyfile_path = request.keyfile_path
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        self.credentials_config.vault_path.parent()
                            .unwrap_or(std::path::Path::new("."))
                            .join("vault.key")
                    });

                // Create parent directory if needed
                if let Some(parent) = keyfile_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }

                // Generate keyfile if it doesn't exist
                if !keyfile_path.exists() {
                    // Generate 32 random bytes for the key using the existing crypto infrastructure
                    let key_bytes = krabbykrus_credentials::crypto::generate_salt(); // 32-byte salt works as a key

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
                                return Ok(Self::json_error(&format!("Failed to write keyfile: {}", e), StatusCode::INTERNAL_SERVER_ERROR));
                            }
                        }
                        Err(e) => return Ok(Self::json_error(&format!("Failed to create keyfile: {}", e), StatusCode::INTERNAL_SERVER_ERROR)),
                    }
                }

                match krabbykrus_credentials::CredentialVault::init_with_keyfile(&self.credentials_config.vault_path, &keyfile_path) {
                    Ok(_) => {
                        info!("Vault initialized with keyfile at {}", self.credentials_config.vault_path.display());
                        let body = serde_json::json!({
                            "status": "initialized",
                            "method": "keyfile",
                            "keyfile_path": keyfile_path.display().to_string(),
                        });
                        Ok(Self::json_response(&body.to_string(), StatusCode::CREATED))
                    }
                    Err(e) => Ok(Self::json_error(&format!("Failed to initialize vault: {}", e), StatusCode::INTERNAL_SERVER_ERROR)),
                }
            }
            _ => Ok(Self::json_error("Invalid method. Use 'password' or 'keyfile'", StatusCode::BAD_REQUEST)),
        }
    }

    /// Handle list permissions
    async fn handle_list_permissions(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        let level = match request.level.as_str() {
            "allow" => krabbykrus_credentials::PermissionLevel::Allow,
            "allow_hil" | "hil" => krabbykrus_credentials::PermissionLevel::AllowHil,
            "allow_hil_2fa" | "hil_2fa" => krabbykrus_credentials::PermissionLevel::AllowHil2fa,
            "deny" => krabbykrus_credentials::PermissionLevel::Deny,
            _ => return Ok(Self::json_error("Invalid permission level. Use: allow, allow_hil, allow_hil_2fa, deny", StatusCode::BAD_REQUEST)),
        };

        let permission = krabbykrus_credentials::PathPermission {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        let permission_id = path.strip_prefix("/api/credentials/permissions/").unwrap_or("");
        let uuid = match uuid::Uuid::parse_str(permission_id) {
            Ok(id) => id,
            Err(_) => return Ok(Self::json_error("Invalid permission ID", StatusCode::BAD_REQUEST)),
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/approve
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/approve"))
            .unwrap_or("");
        
        let uuid = match uuid::Uuid::parse_str(request_id) {
            Ok(id) => id,
            Err(_) => return Ok(Self::json_error("Invalid request ID", StatusCode::BAD_REQUEST)),
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

        let response = krabbykrus_credentials::HilApprovalResponse {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let Some(manager) = &self.credential_manager else {
            return Ok(Self::json_error("Credential management not enabled", StatusCode::SERVICE_UNAVAILABLE));
        };

        // Extract request ID from path: /api/credentials/approvals/{id}/deny
        let request_id = path
            .strip_prefix("/api/credentials/approvals/")
            .and_then(|s| s.strip_suffix("/deny"))
            .unwrap_or("");
        
        let uuid = match uuid::Uuid::parse_str(request_id) {
            Ok(id) => id,
            Err(_) => return Ok(Self::json_error("Invalid request ID", StatusCode::BAD_REQUEST)),
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

        let response = krabbykrus_credentials::HilApprovalResponse {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let html = crate::web_ui::get_dashboard_html();
        
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Full::new(html.into()))
            .unwrap())
    }

    // ==================== Gateway Management Handlers ====================

    /// Handle reload agents request
    async fn handle_reload_agents(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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

    fn json_response(body: &str, status: StatusCode) -> Response<Full<hyper::body::Bytes>> {
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(Full::new(body.to_string().into()))
            .unwrap()
    }

    fn json_error(message: &str, status: StatusCode) -> Response<Full<hyper::body::Bytes>> {
        let body = serde_json::json!({
            "error": message,
        });
        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(Full::new(body.to_string().into()))
            .unwrap()
    }
    
    /// Handle WebSocket upgrade request
    async fn handle_websocket_upgrade(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        // For simplicity, we'll return an error for now
        // In a full implementation, this would handle the WebSocket upgrade protocol
        Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Full::new("WebSocket upgrade not implemented in this demo".into()))
            .unwrap())
    }
    
    /// Handle health check endpoint
    async fn handle_health_check(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let health = self.get_health_status().await;
        let body = serde_json::to_string(&health).unwrap();
        
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Full::new(body.into()))
            .unwrap())
    }
    
    /// Handle list agents endpoint
    async fn handle_list_agents(
        &self,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let agents = self.agents.read().await;
        let pending = self.pending_agents.read().await;

        let mut agent_list: Vec<serde_json::Value> = agents.keys().map(|id| {
            serde_json::json!({ "id": id, "status": "active" })
        }).collect();

        for p in pending.iter() {
            agent_list.push(serde_json::json!({
                "id": p.config.id,
                "model": p.config.model,
                "parent_id": p.config.parent_id,
                "status": "pending",
                "reason": p.reason,
            }));
        }

        let body = serde_json::to_string(&agent_list).unwrap();

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Full::new(body.into()))
            .unwrap())
    }

    /// Handle create agent request
    async fn handle_create_agent(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let body = match req.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return Ok(Self::json_error("Failed to read body", StatusCode::BAD_REQUEST)),
        };

        #[derive(Deserialize)]
        struct CreateAgentRequest {
            id: String,
            model: Option<String>,
            parent_id: Option<String>,
            workspace: Option<String>,
            max_tool_calls: Option<u32>,
            system_prompt: Option<String>,
        }

        let req: CreateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        if req.id.trim().is_empty() {
            return Ok(Self::json_error("Agent ID is required", StatusCode::BAD_REQUEST));
        }

        // Check if agent already exists
        let agents = self.agents.read().await;
        if agents.contains_key(&req.id) {
            return Ok(Self::json_error(&format!("Agent '{}' already exists", req.id), StatusCode::CONFLICT));
        }
        drop(agents);

        let config = crate::config::AgentInstance {
            id: req.id.clone(),
            model: req.model,
            workspace: req.workspace.map(std::path::PathBuf::from),
            max_tool_calls: req.max_tool_calls,
            parent_id: req.parent_id,
            system_prompt: req.system_prompt,
            enabled: true,
            config: std::collections::HashMap::new(),
        };

        // Try to create the agent via factory
        if let Some(ref factory) = self.agent_factory {
            match factory(config.clone()).await {
                Ok(agent) => {
                    self.agents.write().await.insert(req.id.clone(), agent);
                    let body = serde_json::json!({ "status": "created", "id": req.id });
                    Ok(Self::json_response(&body.to_string(), StatusCode::CREATED))
                }
                Err(e) => {
                    // Add to pending
                    self.pending_agents.write().await.push(PendingAgent {
                        config,
                        reason: e.to_string(),
                    });
                    let body = serde_json::json!({
                        "status": "pending",
                        "id": req.id,
                        "reason": e.to_string(),
                    });
                    Ok(Self::json_response(&body.to_string(), StatusCode::ACCEPTED))
                }
            }
        } else {
            // No factory, just add to pending
            self.pending_agents.write().await.push(PendingAgent {
                config,
                reason: "No agent factory configured".to_string(),
            });
            let body = serde_json::json!({ "status": "pending", "id": req.id });
            Ok(Self::json_response(&body.to_string(), StatusCode::ACCEPTED))
        }
    }

    /// Handle update agent request
    async fn handle_update_agent(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
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
            system_prompt: Option<String>,
            enabled: Option<bool>,
        }

        let _update: UpdateAgentRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => return Ok(Self::json_error(&format!("Invalid JSON: {}", e), StatusCode::BAD_REQUEST)),
        };

        // Check if agent exists
        let agents = self.agents.read().await;
        if !agents.contains_key(&agent_id) {
            return Ok(Self::json_error(&format!("Agent '{}' not found", agent_id), StatusCode::NOT_FOUND));
        }
        drop(agents);

        // Agent runtime updates would go here (e.g., hot-reload model)
        // For now, acknowledge the update - config file changes are handled by TUI/WebUI
        let body = serde_json::json!({ "status": "updated", "id": agent_id });
        Ok(Self::json_response(&body.to_string(), StatusCode::OK))
    }

    /// Handle delete agent request
    async fn handle_delete_agent(
        &self,
        path: &str,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let agent_id = path.strip_prefix("/api/agents/").unwrap_or("").to_string();

        if agent_id.is_empty() {
            return Ok(Self::json_error("Invalid agent ID", StatusCode::BAD_REQUEST));
        }

        let removed = self.agents.write().await.remove(&agent_id);
        // Also remove from pending
        self.pending_agents.write().await.retain(|p| p.config.id != agent_id);

        if removed.is_some() {
            let body = serde_json::json!({ "status": "deleted", "id": agent_id });
            Ok(Self::json_response(&body.to_string(), StatusCode::OK))
        } else {
            Ok(Self::json_error(&format!("Agent '{}' not found", agent_id), StatusCode::NOT_FOUND))
        }
    }
    
    /// Handle agent message via HTTP API
    async fn handle_agent_message(
        &self,
        req: Request<IncomingBody>,
    ) -> std::result::Result<Response<Full<hyper::body::Bytes>>, hyper::Error> {
        let path = req.uri().path().to_string();
        let agent_id = path.strip_prefix("/api/agents/")
            .and_then(|s| s.strip_suffix("/message"))
            .unwrap_or("");
        
        if agent_id.is_empty() {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Full::new("Invalid agent ID".into()))
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
                    .body(Full::new("Failed to read request body".into()))
                    .unwrap());
            }
        };
        
        let message_request: MessageRequest = match serde_json::from_slice(&body) {
            Ok(req) => req,
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new("Invalid JSON".into()))
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
                    .body(Full::new(body.into()))
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
                    .body(Full::new(body.into()))
                    .unwrap())
            }
        }
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
                message: format!("Agent '{}' not found", agent_id),
            })?;
        
        // Create session ID from session key
        let session_id = format!("{}:{}", agent_id, request.session_key);
        
        // Convert request to message
        let message = Message::text(request.message)
            .with_session_id(&session_id)
            .with_role(MessageRole::User);
        
        // Process message
        agent.process_message(session_id, message).await
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
            .unwrap_or_else(|_| crate::session::SessionStatistics {
                total_sessions: 0,
                active_sessions: 0,
                total_messages: 0,
                total_tokens: 0,
            });
        
        GatewayHealth {
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: 0, // TODO: Track actual uptime
            active_connections: connections.len(),
            active_sessions: session_stats.active_sessions as usize,
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
}

// Clone trait for Gateway (needed for Tokio spawning)
impl Clone for Gateway {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            credentials_config: self.credentials_config.clone(),
            agents: Arc::clone(&self.agents),
            pending_agents: Arc::clone(&self.pending_agents),
            agent_factory: self.agent_factory.clone(),
            session_manager: Arc::clone(&self.session_manager),
            credential_manager: self.credential_manager.clone(),
            ws_connections: Arc::clone(&self.ws_connections),
            shutdown_tx: self.shutdown_tx.clone(),
        }
    }
}

/// HTTP API message request
#[derive(Debug, Deserialize)]
struct MessageRequest {
    session_key: String,
    message: String,
}

/// HTTP API error response
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: Option<String>,
}

#[cfg(test)]
mod tests {
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