//! Chat commands for config management: /config [show|reload]

use rockbot_chat::{ChatCommand, CommandContext, CommandInfo, CommandResult};

/// Register config-related chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(ConfigCommand));
}

struct ConfigCommand;

impl ChatCommand for ConfigCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "config",
            aliases: &[],
            description: "Configuration operations",
            usage: "/config [show|reload]",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let sub = args.trim();
        match sub {
            "show" => CommandResult::Handled(
                "Config display: use the Settings view to browse configuration.".to_string(),
            ),
            "reload" => CommandResult::Action(rockbot_chat::CommandAction::SetStatus(
                "Config reload requested".to_string(),
                false,
            )),
            "" => CommandResult::Handled("Usage: /config [show|reload]".to_string()),
            other => CommandResult::Handled(format!("Unknown config subcommand: {other}")),
        }
    }
}
