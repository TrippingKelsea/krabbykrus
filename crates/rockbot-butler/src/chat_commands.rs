//! Chat command for butler companion: /butler [message]

use rockbot_chat::{ChatCommand, CommandContext, CommandInfo, CommandResult};

/// Register butler chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(ButlerChatCommand));
}

struct ButlerChatCommand;

impl ChatCommand for ButlerChatCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "butler",
            aliases: &[],
            description: "Talk to Butler companion",
            usage: "/butler [message]",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let message = args.trim();
        if message.is_empty() {
            return CommandResult::Handled(
                "Butler is here! Try: /butler status, /butler mood, or /butler <message>"
                    .to_string(),
            );
        }
        // Sub-commands handled by Butler's own dispatch; this signals the TUI
        // to route through the Butler model.
        CommandResult::Action(rockbot_chat::CommandAction::SendToButler(
            message.to_string(),
        ))
    }
}
