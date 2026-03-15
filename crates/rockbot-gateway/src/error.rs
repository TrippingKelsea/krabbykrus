//! Error handling for RockBot gateway.
//!
//! The `RockBotError` aggregator lives here because it references
//! types from heavy runtime crates that the gateway depends on.

pub use rockbot_config::error::{
    ConfigError, GatewayError, SessionError, AgentError, ToolError, MemoryError, SecurityError,
};

use thiserror::Error;

/// Main result type for RockBot operations
pub type Result<T> = std::result::Result<T, RockBotError>;

/// Hierarchical error system for RockBot
#[derive(Debug, Error)]
pub enum RockBotError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Gateway error: {0}")]
    Gateway(#[from] GatewayError),

    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    #[error("Agent error: {0}")]
    Agent(#[from] AgentError),

    #[error("Tool execution error: {0}")]
    Tool(#[from] ToolError),

    #[error("Memory error: {0}")]
    Memory(#[from] MemoryError),

    #[error("Security error: {0}")]
    Security(#[from] SecurityError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("External security error: {0}")]
    ExternalSecurity(#[from] rockbot_security::SecurityError),

    #[error("External tool error: {0}")]
    ExternalTool(#[from] rockbot_tools::ToolError),

    #[error("Credential error: {0}")]
    Credential(#[from] rockbot_credentials::CredentialError),

    #[error("Notify error: {0}")]
    Notify(#[from] notify::Error),

    #[error("LLM error: {0}")]
    Llm(#[from] rockbot_llm::LlmError),

    #[error("Session manager error: {0}")]
    SessionManager(#[from] rockbot_session::Error),

    #[error("Agent engine error: {0}")]
    AgentEngine(#[from] rockbot_agent::Error),
}
