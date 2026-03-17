//! RockBot Configuration, Message, and Error types
//!
//! This is a leaf crate providing shared types used across the RockBot workspace.

pub mod chat_commands;
pub mod config;
pub mod error;
pub mod message;

// Re-export primary types at crate root for convenience
pub use config::{
    AgentConfig, AgentDefaults, AgentInstance, AgentToolConfig, AnimationStyle,
    AnthropicProviderConfig, BedrockProviderConfig, CapabilityConfig, ColorTheme, Config,
    ConfigWatcher, CredentialsConfig, EdgeCondition, FilesystemCapabilities, GatewayConfig,
    McpServerEntry, NetworkCapabilities, OllamaProviderConfig, OpenAiProviderConfig, PkiConfig,
    ProcessCapabilities, ProvidersConfig, SandboxConfig, SecurityConfig, SeedModelConfig,
    ToolConfig, TuiConfig, WorkflowDefinition, WorkflowEdge, WorkflowNode,
};

pub use error::{
    AgentError, ConfigError, GatewayError, MemoryError, SecurityError, SessionError, ToolError,
};

pub use message::{
    Attachment, ContentBlock, ContentPart, Message, MessageBuilder, MessageContent, MessageId,
    MessageMetadata, MessageRole, RichContent, SystemLevel, TextFormatting, ToolResult,
};
