//! RockBot Gateway Server
//!
//! Provides the HTTP/WebSocket gateway server, A2A protocol,
//! cron scheduling, and request routing for the RockBot framework.

pub mod a2a;
pub mod cron;
pub mod error;
pub mod gateway;
pub mod routing;
pub mod slash_commands;

pub use error::{RockBotError, Result};
pub use gateway::{Gateway, GatewayInvoker, PendingAgent, ProviderStatus, AgentFactory};
pub use routing::{RoutingEngine, ResolvedAgentRoute, SessionScope, MatchedByType};
pub use cron::{CronJob, CronSchedule, CronPayload, CronScheduler, CronExecutor};
pub use gateway::{convert_tool_config, convert_security_config};
