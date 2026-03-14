//! RockBot Core Framework
//!
//! This crate provides the core functionality for the RockBot AI agent framework,
//! including the gateway server, session management, and agent execution engine.
//!
//! # Modules
//!
//! - [`config`] - Configuration loading and validation
//! - [`gateway`] - HTTP/WebSocket server
//! - [`agent`] - Agent execution engine
//! - [`session`] - Session persistence
//! - [`message`] - Message types
//! - [`credential_bridge`] - Credential injection for tools
//! - [`skills`] - Skill discovery, loading, and context injection
//! - [`cron`] - Scheduled job execution (SPEC Section 13)
//! - [`web_ui`] - Embedded web dashboard

pub mod config;
pub mod credential_bridge;
pub mod cron;
pub mod error;
pub mod gateway;
pub mod agent;
pub mod routing;
pub mod session;
pub mod skills;
pub mod message;
pub mod web_ui;
pub mod metrics;

pub use config::{
    Config, GatewayConfig, AgentConfig, ProvidersConfig, 
    AnthropicProviderConfig, OpenAiProviderConfig, BedrockProviderConfig, OllamaProviderConfig
};
pub use credential_bridge::VaultCredentialAccessor;
pub use error::{RockBotError, Result};
pub use gateway::Gateway;
pub use agent::Agent;
pub use session::{Session, SessionManager};
pub use message::{Message, MessageContent, MessageMetadata};
pub use routing::{RoutingEngine, ResolvedAgentRoute, SessionScope, MatchedByType};
pub use skills::{SkillManager, Skill, SkillMetadata, SkillInvocationPolicy};
pub use cron::{CronJob, CronSchedule, CronPayload, CronScheduler, CronExecutor};