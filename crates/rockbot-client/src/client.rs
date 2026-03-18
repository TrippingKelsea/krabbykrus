//! Gateway WebSocket client with structured event emission.
//!
//! `GatewayClient` manages a persistent WebSocket connection to a RockBot
//! gateway and emits parsed `GatewayEvent`s over a broadcast channel.
//! Consumers (TUI, CLI agents, etc.) subscribe and map events into their
//! own message types.

use futures_util::{SinkExt, StreamExt};
use rockbot_config::PkiConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

/// Summary of a tool call returned in an agent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub tool_name: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
    pub locality: Option<String>,
}

/// Events emitted by a `GatewayClient`.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    /// Incremental streaming text from an agent.
    StreamChunk { session_key: String, delta: String },
    /// Complete agent response (final message after streaming).
    AgentResponse {
        session_key: String,
        content: String,
        tool_calls: Vec<ToolCallSummary>,
        tokens_used: Option<TokenUsageInfo>,
        processing_time_ms: Option<u64>,
    },
    /// Agent processing error.
    AgentError { session_key: String, error: String },
    /// A tool call has started.
    ToolCall {
        tool_name: String,
        locality: Option<String>,
    },
    /// A tool call has completed.
    ToolResult {
        session_key: String,
        tool_name: String,
        result: String,
        success: bool,
        duration_ms: u64,
        locality: Option<String>,
    },
    /// Token usage update.
    TokenUsage {
        session_key: String,
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        cumulative_total: u64,
    },
    /// Agent thinking/processing status.
    ThinkingStatus {
        session_key: String,
        phase: String,
        tool_name: Option<String>,
        iteration: Option<usize>,
    },
    /// Gateway health status.
    HealthStatus {
        connected: bool,
        version: Option<String>,
        uptime_secs: Option<u64>,
        active_connections: usize,
        active_sessions: usize,
        pending_agents: usize,
    },
    /// Keepalive response.
    Pong,
    /// WebSocket connection established.
    Connected,
    ClientIdentityAssigned {
        client_uuid: String,
        hostname: String,
        label: Option<String>,
    },
    /// WebSocket disconnected.
    Disconnected { reason: String },
    /// Gateway-level error message.
    Error { message: String },
    /// Noise Protocol handshake step (remote-exec).
    NoiseHandshakeStep { step: u8, payload: String },
    /// Server acknowledged remote capabilities (remote-exec).
    RemoteCapabilitiesAck { accepted: bool, message: String },
    /// Tool execution request from the gateway (remote-exec).
    RemoteToolRequest {
        request_id: String,
        tool_name: String,
        params: String,
        agent_id: String,
        session_id: String,
        workspace_path: String,
    },
    /// Response to a generic WS API request.
    ApiResponse {
        request_id: String,
        status: u16,
        body: String,
        content_type: Option<String>,
    },
}

/// Token usage info attached to an agent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageInfo {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Gateway client that maintains a WebSocket connection and emits events.
#[derive(Clone)]
pub struct GatewayClient {
    /// Send raw text over the WebSocket.
    ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>>,
    /// Broadcast channel for events.
    event_tx: broadcast::Sender<GatewayEvent>,
    /// Whether the WS is currently connected.
    connected: Arc<AtomicBool>,
    /// In-flight WS API requests waiting on a response.
    pending_api_requests: Arc<Mutex<HashMap<String, oneshot::Sender<ApiResponse>>>>,
}

/// Structured response returned by the gateway WS API tunnel.
#[derive(Debug, Clone)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
    pub content_type: Option<String>,
}

impl GatewayClient {
    /// Create and connect a new gateway client.
    ///
    /// Spawns a background task that connects (with retry) to the gateway
    /// WebSocket at `ws_url` and emits `GatewayEvent`s.
    pub fn connect(ws_url: &str) -> Self {
        Self::connect_with_pki(ws_url, None)
    }

    /// Create and connect a new gateway client using the provided PKI config
    /// for outbound client-auth TLS when certificate material is available.
    pub fn connect_with_pki(ws_url: &str, pki: Option<&PkiConfig>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>> =
            Arc::new(tokio::sync::RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));
        let pending_api_requests = Arc::new(Mutex::new(HashMap::new()));

        let client = Self {
            ws_tx: Arc::clone(&ws_tx),
            event_tx: event_tx.clone(),
            connected: Arc::clone(&connected),
            pending_api_requests: Arc::clone(&pending_api_requests),
        };

        let url = ws_url.to_string();
        let pki = pki.cloned();
        tokio::spawn(async move {
            Self::run_connection(url, ws_tx, event_tx, connected, pending_api_requests, pki).await;
        });

        client
    }

    /// Subscribe to gateway events.
    pub fn subscribe(&self) -> broadcast::Receiver<GatewayEvent> {
        self.event_tx.subscribe()
    }

    /// Whether the WebSocket connection is active.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Send a raw JSON string over the WebSocket.
    pub async fn send_raw(&self, text: String) -> Result<(), ClientError> {
        let guard = self.ws_tx.read().await;
        match guard.as_ref() {
            Some(tx) => tx.send(text).map_err(|_| ClientError::Disconnected),
            None => Err(ClientError::Disconnected),
        }
    }

    /// Send an agent message over WebSocket.
    pub async fn send_agent_message(
        &self,
        agent_id: &str,
        session_key: &str,
        message: &str,
    ) -> Result<(), ClientError> {
        let msg = serde_json::json!({
            "type": "agent_message",
            "agent_id": agent_id,
            "session_key": session_key,
            "message": message,
        });
        self.send_raw(serde_json::to_string(&msg).map_err(|_| ClientError::Disconnected)?)
            .await
    }

    /// Send a health check request.
    pub async fn send_health_check(&self) -> Result<(), ClientError> {
        self.send_raw(r#"{"type":"health_check"}"#.to_string())
            .await
    }

    /// Send a ping keepalive.
    pub async fn send_ping(&self) -> Result<(), ClientError> {
        self.send_raw(r#"{"type":"ping"}"#.to_string()).await
    }

    /// Send a management/data API request over the native WS control plane.
    pub async fn send_api_request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<ApiResponse, ClientError> {
        self.wait_for_connection(Duration::from_secs(5)).await?;

        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending_api_requests
            .lock()
            .await
            .insert(request_id.clone(), tx);

        let msg = serde_json::json!({
            "type": "api_request",
            "request_id": request_id,
            "method": method,
            "path": path,
            "body": body,
        });

        if let Err(err) = self
            .send_raw(serde_json::to_string(&msg).map_err(|_| ClientError::Disconnected)?)
            .await
        {
            self.pending_api_requests.lock().await.remove(
                msg.get("request_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            );
            return Err(err);
        }

        match tokio::time::timeout(Duration::from_secs(10), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(ClientError::Disconnected),
            Err(_) => {
                self.pending_api_requests
                    .lock()
                    .await
                    .remove(msg.get("request_id").and_then(serde_json::Value::as_str).unwrap_or_default());
                Err(ClientError::Timeout)
            }
        }
    }

    async fn wait_for_connection(&self, timeout: Duration) -> Result<(), ClientError> {
        let start = std::time::Instant::now();
        while !self.is_connected() {
            if start.elapsed() >= timeout {
                return Err(ClientError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Ok(())
    }

    /// Get a clonable sender handle for raw WS messages.
    pub fn sender(&self) -> GatewaySender {
        GatewaySender {
            ws_tx: Arc::clone(&self.ws_tx),
        }
    }

    /// Build a TLS connector that accepts self-signed certificates.
    fn tls_connector(pki: Option<&PkiConfig>) -> Option<tokio_tungstenite::Connector> {
        /// Verifier that accepts any server certificate (for self-signed certs).
        #[derive(Debug)]
        struct AcceptAnyCert;

        impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
            fn verify_server_cert(
                &self,
                _end_entity: &rustls::pki_types::CertificateDer<'_>,
                _intermediates: &[rustls::pki_types::CertificateDer<'_>],
                _server_name: &rustls::pki_types::ServerName<'_>,
                _ocsp_response: &[u8],
                _now: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }

            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer<'_>,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer<'_>,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }

            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                rustls::crypto::ring::default_provider()
                    .signature_verification_algorithms
                    .supported_schemes()
            }
        }

        let builder = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(AcceptAnyCert));

        let config = match pki.and_then(|cfg| Self::load_client_identity(cfg).ok().flatten()) {
            Some((certs, key)) => builder.with_client_auth_cert(certs, key).ok()?,
            None => builder.with_no_client_auth(),
        };

        Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
            config,
        )))
    }

    fn load_client_identity(
        pki: &PkiConfig,
    ) -> Result<
        Option<(
            Vec<rustls::pki_types::CertificateDer<'static>>,
            rustls::pki_types::PrivateKeyDer<'static>,
        )>,
        ClientError,
    > {
        let (cert_path, key_path) = match (&pki.tls_cert, &pki.tls_key) {
            (Some(cert), Some(key)) => (Self::expand_tilde(cert), Self::expand_tilde(key)),
            _ => return Ok(None),
        };

        let cert_pem = std::fs::read(&cert_path).map_err(|e| {
            ClientError::TlsConfig(format!(
                "Failed to read client certificate {}: {e}",
                cert_path.display()
            ))
        })?;
        let key_pem = std::fs::read(&key_path).map_err(|e| {
            ClientError::TlsConfig(format!(
                "Failed to read client key {}: {e}",
                key_path.display()
            ))
        })?;

        let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| ClientError::TlsConfig(format!("Invalid client certificate PEM: {e}")))?;
        let key = rustls_pemfile::private_key(&mut &key_pem[..])
            .map_err(|e| ClientError::TlsConfig(format!("Invalid client key PEM: {e}")))?
            .ok_or_else(|| ClientError::TlsConfig("No private key found in client key file".into()))?;

        Ok(Some((certs, key)))
    }

    fn expand_tilde(path: &Path) -> PathBuf {
        let s = path.to_string_lossy();
        if s == "~" || s.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(s.strip_prefix("~/").unwrap_or(""));
            }
        }
        path.to_path_buf()
    }

    /// Try connecting to a single WebSocket URL with retries.
    ///
    /// Returns `Some(stream)` on success, `None` after exhausting attempts.
    async fn try_connect_url(
        url: &str,
        max_attempts: u32,
        pki: Option<&PkiConfig>,
    ) -> Option<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let connector = Self::tls_connector(pki);
        for attempt in 1..=max_attempts {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio_tungstenite::connect_async_tls_with_config(
                    url,
                    None,
                    false,
                    connector.clone(),
                ),
            )
            .await
            {
                Ok(Ok((stream, _))) => {
                    info!("WebSocket connected to {url} (attempt {attempt})");
                    return Some(stream);
                }
                Ok(Err(e)) => {
                    debug!("WebSocket connect to {url} attempt {attempt} failed: {e}");
                }
                Err(_) => {
                    debug!("WebSocket connect to {url} attempt {attempt} timed out");
                }
            }
            if attempt < max_attempts {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
        }
        None
    }

    /// Internal connection loop with protocol fallback and retry.
    async fn run_connection(
        url: String,
        ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>>,
        event_tx: broadcast::Sender<GatewayEvent>,
        connected: Arc<AtomicBool>,
        pending_api_requests: Arc<Mutex<HashMap<String, oneshot::Sender<ApiResponse>>>>,
        pki: Option<PkiConfig>,
    ) {
        // Build candidate URLs: primary first, then fallback(s)
        #[allow(unused_mut)]
        let mut candidates = vec![url.clone()];

        // If the primary URL is wss://, add a ws:// fallback when http-insecure is enabled
        #[cfg(feature = "http-insecure")]
        if url.starts_with("wss://") {
            let fallback = format!("ws://{}", &url[6..]);
            candidates.push(fallback);
        }

        let mut ws_stream = None;
        for candidate in &candidates {
            let attempts = if candidates.len() > 1 { 3 } else { 6 };
            if let Some(stream) = Self::try_connect_url(candidate, attempts, pki.as_ref()).await {
                ws_stream = Some(stream);
                break;
            }
        }

        let ws_stream = match ws_stream {
            Some(s) => s,
            None => {
                let tried = candidates.join(", ");
                debug!("WebSocket gave up after trying: {tried}");
                let _ = event_tx.send(GatewayEvent::Disconnected {
                    reason: format!("Connection failed (tried: {tried})"),
                });
                return;
            }
        };

        connected.store(true, Ordering::Relaxed);
        let _ = event_tx.send(GatewayEvent::Connected);

        let (mut sink, mut source) = ws_stream.split();
        let (send_tx, mut send_rx) = mpsc::unbounded_channel::<String>();

        // Store the send channel
        {
            let mut guard = ws_tx.write().await;
            *guard = Some(send_tx);
        }

        loop {
            tokio::select! {
                outbound = send_rx.recv() => {
                    match outbound {
                        Some(text) => {
                            use tokio_tungstenite::tungstenite::Message as WsMsg;
                            if sink.send(WsMsg::Text(text)).await.is_err() {
                                warn!("WebSocket send failed, disconnecting");
                                break;
                            }
                        }
                        None => break,
                    }
                }
                inbound = source.next() => {
                    match inbound {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            Self::parse_and_emit(&event_tx, &pending_api_requests, &text).await;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                            info!("WebSocket closed by server");
                            break;
                        }
                        Some(Err(e)) => {
                            debug!("WebSocket error: {e}");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }

        connected.store(false, Ordering::Relaxed);
        {
            let mut guard = ws_tx.write().await;
            *guard = None;
        }
        let _ = event_tx.send(GatewayEvent::Disconnected {
            reason: "Connection closed".to_string(),
        });
        pending_api_requests.lock().await.clear();
        info!("WebSocket disconnected");
    }

    /// Parse a WebSocket JSON message and emit the corresponding event.
    async fn parse_and_emit(
        event_tx: &broadcast::Sender<GatewayEvent>,
        pending_api_requests: &Arc<Mutex<HashMap<String, oneshot::Sender<ApiResponse>>>>,
        text: &str,
    ) {
        let json: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warn!("Invalid WebSocket JSON from gateway: {e}");
                return;
            }
        };

        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let event = match msg_type {
            "stream_chunk" => {
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let delta = json
                    .get("delta")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if session_key.is_empty() || delta.is_empty() {
                    return;
                }
                GatewayEvent::StreamChunk { session_key, delta }
            }
            "tool_call" => {
                let tool_name = json
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let locality = json
                    .get("locality")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from);
                GatewayEvent::ToolCall {
                    tool_name,
                    locality,
                }
            }
            "tool_result" => {
                let tool_name = json
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let success = json
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let duration_ms = json
                    .get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                GatewayEvent::ToolResult {
                    session_key,
                    tool_name,
                    result: json
                        .get("result")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    success,
                    duration_ms,
                    locality: json
                        .get("locality")
                        .and_then(serde_json::Value::as_str)
                        .map(String::from),
                }
            }
            "agent_response" => {
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = json
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_calls: Vec<ToolCallSummary> = json
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|tc| {
                                Some(ToolCallSummary {
                                    tool_name: tc.get("tool_name")?.as_str()?.to_string(),
                                    result: tc
                                        .get("result")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                    success: tc
                                        .get("success")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(true),
                                    duration_ms: tc
                                        .get("duration_ms")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0),
                                    locality: tc
                                        .get("locality")
                                        .and_then(serde_json::Value::as_str)
                                        .map(String::from),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let tokens_used = json.get("tokens_used").and_then(|tokens| {
                    Some(TokenUsageInfo {
                        prompt_tokens: tokens.get("prompt_tokens")?.as_u64()?,
                        completion_tokens: tokens.get("completion_tokens")?.as_u64()?,
                        total_tokens: tokens.get("total_tokens")?.as_u64()?,
                    })
                });
                let processing_time_ms = json.get("processing_time_ms").and_then(|v| v.as_u64());

                GatewayEvent::AgentResponse {
                    session_key,
                    content,
                    tool_calls,
                    tokens_used,
                    processing_time_ms,
                }
            }
            "agent_error" => {
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let error = json
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                GatewayEvent::AgentError { session_key, error }
            }
            "token_usage" => {
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if session_key.is_empty() {
                    return;
                }
                GatewayEvent::TokenUsage {
                    session_key,
                    prompt_tokens: json
                        .get("prompt_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    completion_tokens: json
                        .get("completion_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    total_tokens: json
                        .get("total_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    cumulative_total: json
                        .get("cumulative_total")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                }
            }
            "thinking_status" => {
                let session_key = json
                    .get("session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if session_key.is_empty() {
                    return;
                }
                GatewayEvent::ThinkingStatus {
                    session_key,
                    phase: json
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    tool_name: json
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    iteration: json
                        .get("iteration")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize),
                }
            }
            "pong" => GatewayEvent::Pong,
            "client_identity_assigned" => GatewayEvent::ClientIdentityAssigned {
                client_uuid: json
                    .get("client_uuid")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                hostname: json
                    .get("hostname")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                label: json
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(String::from),
            },
            "health_status" => {
                if let Some(status) = json.get("status") {
                    GatewayEvent::HealthStatus {
                        connected: true,
                        version: status
                            .get("version")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        uptime_secs: status.get("uptime_seconds").and_then(|v| v.as_u64()),
                        active_connections: status
                            .get("active_connections")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                        active_sessions: status
                            .get("active_sessions")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                        pending_agents: status
                            .get("pending_agents")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                    }
                } else {
                    return;
                }
            }
            "error" => {
                let message = json
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                warn!("Gateway WebSocket error: {message}");
                GatewayEvent::Error { message }
            }
            "noise_handshake" => {
                let step = json.get("step").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                let payload = json
                    .get("payload")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                GatewayEvent::NoiseHandshakeStep { step, payload }
            }
            "remote_capabilities_ack" => {
                let accepted = json
                    .get("accepted")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let message = json
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                GatewayEvent::RemoteCapabilitiesAck { accepted, message }
            }
            "remote_tool_request" => {
                let request_id = json
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tool_name = json
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let params = json
                    .get("params")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}")
                    .to_string();
                let agent_id = json
                    .get("agent_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let session_id = json
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let workspace_path = json
                    .get("workspace_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".")
                    .to_string();
                GatewayEvent::RemoteToolRequest {
                    request_id,
                    tool_name,
                    params,
                    agent_id,
                    session_id,
                    workspace_path,
                }
            }
            "api_response" => {
                let request_id = json
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if request_id.is_empty() {
                    return;
                }
                let response = ApiResponse {
                    status: json.get("status").and_then(|v| v.as_u64()).unwrap_or(500) as u16,
                    body: json
                        .get("body")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    content_type: json
                        .get("content_type")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                };
                if let Some(tx) = pending_api_requests.lock().await.remove(&request_id) {
                    let _ = tx.send(response.clone());
                }
                GatewayEvent::ApiResponse {
                    request_id,
                    status: response.status,
                    body: response.body,
                    content_type: response.content_type,
                }
            }
            other => {
                debug!("Unhandled WebSocket message type: {other}");
                return;
            }
        };

        let _ = event_tx.send(event);
    }
}

/// Clonable handle for sending raw WebSocket messages.
///
/// Can be passed to spawned tasks for outbound WS communication.
#[derive(Clone)]
pub struct GatewaySender {
    ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>>,
}

impl GatewaySender {
    /// Send a raw JSON string over the WebSocket.
    pub async fn send(&self, text: String) -> Result<(), ClientError> {
        let guard = self.ws_tx.read().await;
        match guard.as_ref() {
            Some(tx) => tx.send(text).map_err(|_| ClientError::Disconnected),
            None => Err(ClientError::Disconnected),
        }
    }
}

/// Client errors.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("WebSocket disconnected")]
    Disconnected,
    #[error("WebSocket request timed out")]
    Timeout,
    #[error("TLS configuration error: {0}")]
    TlsConfig(String),
}

/// Derive the HTTP base URL from a WebSocket URL.
///
/// `wss://host:port/ws` → `https://host:port`
/// `ws://host:port/ws`  → `http://host:port`
pub fn ws_url_to_http(ws_url: &str) -> String {
    let base = ws_url.trim_end_matches("/ws").trim_end_matches('/');
    if base.starts_with("wss://") {
        format!("https://{}", &base[6..])
    } else if base.starts_with("ws://") {
        format!("http://{}", &base[5..])
    } else {
        base.to_string()
    }
}

/// Normalize a raw gateway address into a fully-qualified WebSocket URL.
///
/// Accepts:
///   - `ws://host:port/ws` or `wss://host:port/ws` — returned as-is
///   - `http://host:port` — converted to `ws://host:port/ws`
///   - `https://host:port` — converted to `wss://host:port/ws`
///   - `host:port` or `host` — protocol probing will be attempted
///
/// When no scheme is given and `http-insecure` is **not** enabled, the
/// connection defaults to `wss://`.  With `http-insecure`, it tries WSS
/// first then falls back to WS.
pub fn normalize_gateway_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');

    // Already has a WS scheme — use as-is (append /ws if missing)
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        return if trimmed.ends_with("/ws") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/ws")
        };
    }

    // HTTP(S) scheme — convert to WS equivalent
    if trimmed.starts_with("https://") {
        let host_port = trimmed.strip_prefix("https://").unwrap_or(trimmed);
        let host_port = host_port.trim_end_matches("/ws");
        return format!("wss://{host_port}/ws");
    }
    if trimmed.starts_with("http://") {
        let host_port = trimmed.strip_prefix("http://").unwrap_or(trimmed);
        let host_port = host_port.trim_end_matches("/ws");
        #[cfg(not(feature = "http-insecure"))]
        {
            tracing::warn!(
                "Plain HTTP not available (build with http-insecure feature). Upgrading to wss://"
            );
            return format!("wss://{host_port}/ws");
        }
        #[cfg(feature = "http-insecure")]
        return format!("ws://{host_port}/ws");
    }

    // Bare host:port — default to wss, will fall back to ws if http-insecure
    format!("wss://{trimmed}/ws")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[tokio::test]
    async fn test_parse_stream_chunk() {
        let (tx, mut rx) = broadcast::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let json = r#"{"type":"stream_chunk","session_key":"s1","delta":"hello"}"#;
        GatewayClient::parse_and_emit(&tx, &pending, json).await;
        let event = rx.try_recv().unwrap();
        match event {
            GatewayEvent::StreamChunk { session_key, delta } => {
                assert_eq!(session_key, "s1");
                assert_eq!(delta, "hello");
            }
            _ => panic!("unexpected event"),
        }
    }

    #[tokio::test]
    async fn test_parse_agent_response() {
        let (tx, mut rx) = broadcast::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let json = r#"{"type":"agent_response","session_key":"s1","content":"hi","tool_calls":[]}"#;
        GatewayClient::parse_and_emit(&tx, &pending, json).await;
        let event = rx.try_recv().unwrap();
        match event {
            GatewayEvent::AgentResponse {
                session_key,
                content,
                tool_calls,
                ..
            } => {
                assert_eq!(session_key, "s1");
                assert_eq!(content, "hi");
                assert!(tool_calls.is_empty());
            }
            _ => panic!("unexpected event"),
        }
    }

    #[tokio::test]
    async fn test_parse_pong() {
        let (tx, mut rx) = broadcast::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        GatewayClient::parse_and_emit(&tx, &pending, r#"{"type":"pong"}"#).await;
        match rx.try_recv().unwrap() {
            GatewayEvent::Pong => {}
            _ => panic!("expected Pong"),
        }
    }

    #[tokio::test]
    async fn test_parse_unknown_type() {
        let (tx, mut rx) = broadcast::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        GatewayClient::parse_and_emit(&tx, &pending, r#"{"type":"something_new"}"#).await;
        assert!(rx.try_recv().is_err()); // No event emitted
    }

    #[tokio::test]
    async fn test_parse_invalid_json() {
        let (tx, mut rx) = broadcast::channel(16);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        GatewayClient::parse_and_emit(&tx, &pending, "not json").await;
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_normalize_bare_host_port() {
        assert_eq!(
            normalize_gateway_url("172.30.200.146:18181"),
            "wss://172.30.200.146:18181/ws"
        );
    }

    #[test]
    fn test_normalize_https() {
        assert_eq!(
            normalize_gateway_url("https://example.com:8080"),
            "wss://example.com:8080/ws"
        );
    }

    #[test]
    fn test_normalize_wss_passthrough() {
        assert_eq!(
            normalize_gateway_url("wss://host:1234/ws"),
            "wss://host:1234/ws"
        );
    }

    #[test]
    fn test_normalize_wss_appends_path() {
        assert_eq!(
            normalize_gateway_url("wss://host:1234"),
            "wss://host:1234/ws"
        );
    }

    #[test]
    fn test_normalize_trailing_slash() {
        assert_eq!(
            normalize_gateway_url("172.30.200.146:18181/"),
            "wss://172.30.200.146:18181/ws"
        );
    }

    #[test]
    fn test_ws_url_to_http_wss() {
        assert_eq!(ws_url_to_http("wss://host:1234/ws"), "https://host:1234");
    }

    #[test]
    fn test_ws_url_to_http_ws() {
        assert_eq!(ws_url_to_http("ws://host:1234/ws"), "http://host:1234");
    }
}
