//! Error types for the agent crate.

pub use rockbot_config::error::AgentError;
use thiserror::Error;

/// Result type for agent operations
pub type Result<T> = std::result::Result<T, Error>;

/// Agent crate error — wraps domain errors and external crate errors
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Agent(#[from] AgentError),
    #[error("Session error: {0}")]
    Session(#[from] rockbot_session::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("LLM error: {0}")]
    Llm(#[from] rockbot_llm::LlmError),
    #[error("Tool error: {0}")]
    Tool(#[from] rockbot_tools::ToolError),
    #[error("Security error: {0}")]
    Security(#[from] rockbot_security::SecurityError),
    #[error("Credential error: {0}")]
    Credential(#[from] rockbot_credentials::CredentialError),
    #[error("Config error: {0}")]
    Config(#[from] rockbot_config::ConfigError),
}

// Allow ? from AgentError in functions returning Result<T, Error>
// (already covered by #[from] above)

// Allow converting Error back into AgentError for places that need it
impl From<Error> for AgentError {
    fn from(e: Error) -> Self {
        match e {
            Error::Agent(a) => a,
            other => AgentError::ExecutionFailed {
                message: other.to_string(),
            },
        }
    }
}
