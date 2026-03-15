//! Gateway WebSocket client with structured event emission.
//!
//! `GatewayClient` manages a persistent WebSocket connection to a RockBot
//! gateway and emits parsed `GatewayEvent`s over a broadcast channel.
//! Consumers (TUI, CLI agents, etc.) subscribe and map events into their
//! own message types.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

/// Summary of a tool call returned in an agent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub tool_name: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
}

/// Events emitted by a `GatewayClient`.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    /// Incremental streaming text from an agent.
    StreamChunk {
        session_key: String,
        delta: String,
    },
    /// Complete agent response (final message after streaming).
    AgentResponse {
        session_key: String,
        content: String,
        tool_calls: Vec<ToolCallSummary>,
        tokens_used: Option<TokenUsageInfo>,
        processing_time_ms: Option<u64>,
    },
    /// Agent processing error.
    AgentError {
        session_key: String,
        error: String,
    },
    /// A tool call has started.
    ToolCall {
        tool_name: String,
    },
    /// A tool call has completed.
    ToolResult {
        tool_name: String,
        success: bool,
        duration_ms: u64,
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
        active_sessions: usize,
        pending_agents: usize,
    },
    /// Keepalive response.
    Pong,
    /// WebSocket connection established.
    Connected,
    /// WebSocket disconnected.
    Disconnected {
        reason: String,
    },
    /// Gateway-level error message.
    Error {
        message: String,
    },
    /// Noise Protocol handshake step (remote-exec).
    NoiseHandshakeStep {
        step: u8,
        payload: String,
    },
    /// Server acknowledged remote capabilities (remote-exec).
    RemoteCapabilitiesAck {
        accepted: bool,
        message: String,
    },
    /// Tool execution request from the gateway (remote-exec).
    RemoteToolRequest {
        request_id: String,
        tool_name: String,
        params: String,
        agent_id: String,
        session_id: String,
        workspace_path: String,
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
pub struct GatewayClient {
    /// Send raw text over the WebSocket.
    ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>>,
    /// Broadcast channel for events.
    event_tx: broadcast::Sender<GatewayEvent>,
    /// Whether the WS is currently connected.
    connected: Arc<AtomicBool>,
}

impl GatewayClient {
    /// Create and connect a new gateway client.
    ///
    /// Spawns a background task that connects (with retry) to the gateway
    /// WebSocket at `ws_url` and emits `GatewayEvent`s.
    pub fn connect(ws_url: &str) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>> =
            Arc::new(tokio::sync::RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));

        let client = Self {
            ws_tx: Arc::clone(&ws_tx),
            event_tx: event_tx.clone(),
            connected: Arc::clone(&connected),
        };

        let url = ws_url.to_string();
        tokio::spawn(async move {
            Self::run_connection(url, ws_tx, event_tx, connected).await;
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
        self.send_raw(r#"{"type":"health_check"}"#.to_string()).await
    }

    /// Send a ping keepalive.
    pub async fn send_ping(&self) -> Result<(), ClientError> {
        self.send_raw(r#"{"type":"ping"}"#.to_string()).await
    }

    /// Get a clonable sender handle for raw WS messages.
    pub fn sender(&self) -> GatewaySender {
        GatewaySender {
            ws_tx: Arc::clone(&self.ws_tx),
        }
    }

    /// Internal connection loop with retry.
    async fn run_connection(
        url: String,
        ws_tx: Arc<tokio::sync::RwLock<Option<mpsc::UnboundedSender<String>>>>,
        event_tx: broadcast::Sender<GatewayEvent>,
        connected: Arc<AtomicBool>,
    ) {
        let mut attempt = 0u32;
        let ws_stream = loop {
            attempt += 1;
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                tokio_tungstenite::connect_async(&url),
            )
            .await
            {
                Ok(Ok((stream, _))) => {
                    info!("WebSocket connected to gateway (attempt {attempt})");
                    break stream;
                }
                Ok(Err(e)) => {
                    debug!("WebSocket connect attempt {attempt} failed: {e}");
                }
                Err(_) => {
                    debug!("WebSocket connect attempt {attempt} timed out");
                }
            }
            if attempt >= 6 {
                debug!("WebSocket gave up after {attempt} attempts");
                let _ = event_tx.send(GatewayEvent::Disconnected {
                    reason: "Connection failed after 6 attempts".to_string(),
                });
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
                            Self::parse_and_emit(&event_tx, &text);
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
        info!("WebSocket disconnected");
    }

    /// Parse a WebSocket JSON message and emit the corresponding event.
    fn parse_and_emit(event_tx: &broadcast::Sender<GatewayEvent>, text: &str) {
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
                GatewayEvent::ToolCall { tool_name }
            }
            "tool_result" => {
                let tool_name = json
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
                let duration_ms = json.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                GatewayEvent::ToolResult {
                    tool_name,
                    success,
                    duration_ms,
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
                let processing_time_ms = json
                    .get("processing_time_ms")
                    .and_then(|v| v.as_u64());

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
            "health_status" => {
                if let Some(status) = json.get("status") {
                    GatewayEvent::HealthStatus {
                        connected: true,
                        version: status
                            .get("version")
                            .and_then(|v| v.as_str())
                            .map(String::from),
                        uptime_secs: status.get("uptime_seconds").and_then(|v| v.as_u64()),
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_parse_stream_chunk() {
        let (tx, mut rx) = broadcast::channel(16);
        let json = r#"{"type":"stream_chunk","session_key":"s1","delta":"hello"}"#;
        GatewayClient::parse_and_emit(&tx, json);
        let event = rx.try_recv().unwrap();
        match event {
            GatewayEvent::StreamChunk { session_key, delta } => {
                assert_eq!(session_key, "s1");
                assert_eq!(delta, "hello");
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn test_parse_agent_response() {
        let (tx, mut rx) = broadcast::channel(16);
        let json = r#"{"type":"agent_response","session_key":"s1","content":"hi","tool_calls":[]}"#;
        GatewayClient::parse_and_emit(&tx, json);
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

    #[test]
    fn test_parse_pong() {
        let (tx, mut rx) = broadcast::channel(16);
        GatewayClient::parse_and_emit(&tx, r#"{"type":"pong"}"#);
        match rx.try_recv().unwrap() {
            GatewayEvent::Pong => {}
            _ => panic!("expected Pong"),
        }
    }

    #[test]
    fn test_parse_unknown_type() {
        let (tx, mut rx) = broadcast::channel(16);
        GatewayClient::parse_and_emit(&tx, r#"{"type":"something_new"}"#);
        assert!(rx.try_recv().is_err()); // No event emitted
    }

    #[test]
    fn test_parse_invalid_json() {
        let (tx, mut rx) = broadcast::channel(16);
        GatewayClient::parse_and_emit(&tx, "not json");
        assert!(rx.try_recv().is_err());
    }
}
