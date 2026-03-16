//! Chat commands for credential vault management: /vault [lock|unlock|init|status]

use rockbot_chat::{ChatCommand, CommandContext, CommandInfo, CommandResult};

/// Register vault-related chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(VaultCommand));
}

struct VaultCommand;

impl ChatCommand for VaultCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "vault",
            aliases: &[],
            description: "Credential vault operations",
            usage: "/vault [status|lock|unlock|init]",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let sub = args.trim();
        match sub {
            "status" => CommandResult::Handled(
                "Vault status: use the Credentials view to see full details.".to_string(),
            ),
            "lock" => CommandResult::Action(rockbot_chat::CommandAction::SetStatus(
                "Vault locked".to_string(),
                false,
            )),
            "unlock" => CommandResult::Handled(
                "Use the Credentials view to unlock the vault interactively.".to_string(),
            ),
            "init" => CommandResult::Handled(
                "Use the Credentials view to initialize a new vault.".to_string(),
            ),
            "" => CommandResult::Handled("Usage: /vault [status|lock|unlock|init]".to_string()),
            other => CommandResult::Handled(format!("Unknown vault subcommand: {other}")),
        }
    }
}
