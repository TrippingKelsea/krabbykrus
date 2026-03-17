//! Chat commands for agent and session management: /agent, /session

use rockbot_chat::{ChatCommand, CommandAction, CommandContext, CommandInfo, CommandResult};

/// Register agent-related chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(AgentCommand));
    registry.register(Box::new(SessionCommand));
}

struct AgentCommand;

impl ChatCommand for AgentCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "agent",
            aliases: &[],
            description: "Switch active agent",
            usage: "/agent <id>",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let agent_id = args.trim();
        if agent_id.is_empty() {
            return CommandResult::Handled("Usage: /agent <id>".to_string());
        }
        CommandResult::Action(CommandAction::SendToAgent(
            agent_id.to_string(),
            String::new(),
        ))
    }
}

struct SessionCommand;

impl ChatCommand for SessionCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "session",
            aliases: &[],
            description: "Switch active session",
            usage: "/session <key>",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let key = args.trim();
        if key.is_empty() {
            return CommandResult::Handled("Usage: /session <key>".to_string());
        }
        CommandResult::Handled(format!(
            "Session switch to '{key}' — use Sessions view to manage sessions."
        ))
    }
}
