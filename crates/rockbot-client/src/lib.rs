//! Gateway client library for RockBot.
//!
//! Provides `GatewayClient` for WebSocket communication with a RockBot gateway,
//! the ACP (Agent Client Protocol) for IDE integration, and remote execution
//! types for the Noise Protocol encrypted tool dispatch.

pub mod acp;
pub mod client;
#[cfg(feature = "remote-exec")]
pub mod remote_exec;

pub use client::{
    normalize_gateway_url, ws_url_to_http, ClientError, GatewayClient, GatewayEvent, GatewaySender,
    TokenUsageInfo, ToolCallSummary,
};
