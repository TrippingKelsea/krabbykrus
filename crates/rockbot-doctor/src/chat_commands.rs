//! Chat command for doctor diagnostics: /doctor

use rockbot_chat::{ChatCommand, CommandContext, CommandInfo, CommandResult};

/// Register doctor chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(DoctorCommand));
}

struct DoctorCommand;

impl ChatCommand for DoctorCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "doctor",
            aliases: &[],
            description: "Run config diagnostics",
            usage: "/doctor",
        }
    }

    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Action(rockbot_chat::CommandAction::ShowOverlay(
            "doctor".to_string(),
        ))
    }
}
