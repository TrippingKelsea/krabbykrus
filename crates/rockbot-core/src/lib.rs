//! RockBot Core — Re-export Facade
//!
//! This crate re-exports types from the focused subcrates for backward
//! compatibility. New code should depend on the specific subcrate directly:
//!
//! - `rockbot-config` — Config, message, and error sub-enum types
//! - `rockbot-session` — Session management and persistence
//! - `rockbot-agent` — Agent execution engine, hooks, guardrails, tools
//! - `rockbot-client` — Gateway WS client, ACP, remote execution
//! - `rockbot-gateway` — HTTP/WS server, A2A, cron, routing
//! - `rockbot-webui` — Embedded web dashboard HTML

// Module re-exports (thin wrappers for backward compat)
pub mod config {
    pub use rockbot_config::config::*;
}
pub mod message {
    pub use rockbot_agent::from_llm_message;
    pub use rockbot_config::message::*;
}
pub mod error {
    pub use rockbot_gateway::error::*;
}
pub mod session {
    pub use rockbot_session::*;
}
pub mod agent {
    pub use rockbot_agent::agent::*;
}
pub mod gateway {
    pub use rockbot_gateway::gateway::*;
}
pub mod routing {
    pub use rockbot_gateway::routing::*;
}
pub mod hooks {
    pub use rockbot_agent::hooks::*;
}
pub mod guardrails {
    pub use rockbot_agent::guardrails::*;
}
pub mod trajectory {
    pub use rockbot_agent::trajectory::*;
}
pub mod orchestration {
    pub use rockbot_agent::orchestration::*;
}
pub mod metrics {
    pub use rockbot_agent::metrics::*;
}
pub mod skills {
    pub use rockbot_agent::skills::*;
}
pub mod credential_bridge {
    pub use rockbot_agent::credential_bridge::*;
}
pub mod tokenizer {
    pub use rockbot_agent::tokenizer::*;
}
pub mod indexer {
    pub use rockbot_agent::indexer::*;
}
pub mod sandbox {
    pub use rockbot_agent::sandbox::*;
}
pub mod telemetry {
    pub use rockbot_agent::telemetry::*;
}
pub mod a2a {
    pub use rockbot_gateway::a2a::*;
}
pub mod acp {
    pub use rockbot_client::acp::*;
}
pub mod cron {
    pub use rockbot_gateway::cron::*;
}
pub mod chat_commands;
// slash_commands: pub(crate) in rockbot-gateway, not re-exported
pub mod web_ui {
    pub use rockbot_webui::*;
}
#[cfg(feature = "noise")]
pub mod remote_exec {
    pub use rockbot_client::remote_exec::*;
}

// Top-level convenience re-exports
pub use rockbot_agent::agent::{Agent, HandoffSignal};
pub use rockbot_agent::credential_bridge::VaultCredentialAccessor;
pub use rockbot_agent::guardrails::{
    Guardrail, GuardrailPipeline, GuardrailResult, PiiGuardrail, PromptInjectionGuardrail,
};
pub use rockbot_agent::hooks::{Hook, HookEvent, HookRegistry, HookResult};
pub use rockbot_agent::orchestration::{SwarmBlackboard, WorkflowExecutor};
pub use rockbot_agent::skills::{
    Skill, SkillInvocationPolicy, SkillManager, SkillMetadata, SlashCommandInfo,
};
pub use rockbot_agent::telemetry::{init_telemetry, TelemetryConfig};
pub use rockbot_agent::trajectory::{Trajectory, TrajectoryEntry, TrajectoryEvent};
pub use rockbot_config::{
    AgentConfig, AnimationStyle, AnthropicProviderConfig, BedrockProviderConfig, ColorTheme,
    Config, EdgeCondition, GatewayConfig, McpServerEntry, OllamaProviderConfig,
    OpenAiProviderConfig, PkiConfig, ProvidersConfig, RgbaColor, SeedModelConfig, TuiConfig,
    TuiFontPreferences, TuiThemeConfig, WorkflowDefinition, WorkflowEdge, WorkflowNode,
};
pub use rockbot_config::{ContentPart, Message, MessageContent, MessageMetadata};
pub use rockbot_gateway::{CronExecutor, CronJob, CronPayload, CronSchedule, CronScheduler};
pub use rockbot_gateway::{Gateway, GatewayInvoker, Result, RockBotError};
pub use rockbot_gateway::{MatchedByType, ResolvedAgentRoute, RoutingEngine, SessionScope};
pub use rockbot_session::{Session, SessionManager};
