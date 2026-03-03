//! Krabbykrus Core Framework
//! 
//! This crate provides the core functionality for the Krabbykrus AI agent framework,
//! including the gateway server, session management, and agent execution engine.

pub mod config;
pub mod error;
pub mod gateway;
pub mod agent;
pub mod session;
pub mod message;
pub mod web_ui;

pub use config::{Config, GatewayConfig, AgentConfig};
pub use error::{KrabbykrusError, Result};
pub use gateway::Gateway;
pub use agent::Agent;
pub use session::{Session, SessionManager};
pub use message::{Message, MessageContent, MessageMetadata};