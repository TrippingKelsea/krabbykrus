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

pub use cron::{CronExecutor, CronJob, CronPayload, CronSchedule, CronScheduler};
pub use error::{Result, RockBotError};
pub use gateway::{convert_security_config, convert_tool_config};
pub use gateway::{AgentFactory, Gateway, GatewayInvoker, PendingAgent, ProviderStatus};
pub use routing::{MatchedByType, ResolvedAgentRoute, RoutingEngine, SessionScope};
