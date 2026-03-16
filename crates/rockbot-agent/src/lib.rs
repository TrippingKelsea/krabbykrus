//! RockBot Agent Execution Engine
//!
//! This crate contains the core agent logic: LLM interaction, tool execution,
//! guardrails, hooks, trajectories, skills, and orchestration.

pub mod agent;
pub mod credential_bridge;
pub mod error;
pub mod guardrails;
pub mod hooks;
pub mod indexer;
pub mod metrics;
pub mod orchestration;
pub mod sandbox;
pub mod skills;
pub mod telemetry;
pub mod tokenizer;
pub mod trajectory;

// Re-export primary types
pub use agent::{Agent, AgentResponse, HandoffSignal};
pub use credential_bridge::VaultCredentialAccessor;
pub use error::{Error, Result};
pub use guardrails::{
    Guardrail, GuardrailPipeline, GuardrailResult, PiiGuardrail, PromptInjectionGuardrail,
};
pub use hooks::{Hook, HookEvent, HookRegistry, HookResult};
pub use orchestration::{SwarmBlackboard, WorkflowExecutor};
pub use skills::{Skill, SkillInvocationPolicy, SkillManager, SkillMetadata, SlashCommandInfo};
pub use telemetry::{init_telemetry, TelemetryConfig};
pub use trajectory::{Trajectory, TrajectoryEntry, TrajectoryEvent};

/// Create a Message from an LLM message.
///
/// This lives here because it depends on `rockbot_llm` types which `rockbot-config` doesn't have.
pub fn from_llm_message(
    llm_message: rockbot_llm::Message,
    session_id: &str,
    agent_id: &str,
) -> std::result::Result<rockbot_config::Message, rockbot_config::AgentError> {
    use rockbot_config::{Message, MessageContent, MessageRole};

    let role = match llm_message.role {
        rockbot_llm::MessageRole::User => MessageRole::User,
        rockbot_llm::MessageRole::Assistant => MessageRole::Assistant,
        rockbot_llm::MessageRole::System => MessageRole::System,
        rockbot_llm::MessageRole::Tool => MessageRole::Tool,
    };

    let content = MessageContent::Text {
        text: llm_message.content,
    };

    let mut msg = Message::new(content)
        .with_session_id(session_id)
        .with_agent_id(agent_id)
        .with_role(role);

    if let Some(ref tool_calls) = llm_message.tool_calls {
        if !tool_calls.is_empty() {
            if let Ok(tc_json) = serde_json::to_value(tool_calls) {
                msg.metadata.extra.insert("tool_calls".to_string(), tc_json);
            }
        }
    }

    if let Some(ref tool_call_id) = llm_message.tool_call_id {
        msg.metadata.extra.insert(
            "tool_call_id".to_string(),
            serde_json::Value::String(tool_call_id.clone()),
        );
    }

    Ok(msg)
}
