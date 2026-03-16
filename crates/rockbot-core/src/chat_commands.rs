//! Aggregated chat commands from subcrates.
//!
//! rockbot-core is a facade — it re-exports chat command registration from
//! rockbot-config (config commands), rockbot-agent (agent/session commands),
//! and adds its own cron commands.

use rockbot_chat::{ChatCommand, CommandContext, CommandInfo, CommandResult};

/// Register all core chat commands (config + agent + cron).
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    // Config commands: /config
    rockbot_config::chat_commands::register_chat_commands(registry);
    // Agent commands: /agent, /session
    rockbot_agent::chat_commands::register_chat_commands(registry);
    // Cron commands: /cron
    registry.register(Box::new(CronCommand));
}

struct CronCommand;

impl ChatCommand for CronCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "cron",
            aliases: &[],
            description: "Cron job operations",
            usage: "/cron [list|enable|disable <id>]",
        }
    }

    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
        let sub = parts.first().copied().unwrap_or("");
        match sub {
            "list" => CommandResult::Handled(
                "Cron job listing: use the Cron Jobs view for details.".to_string(),
            ),
            "enable" => {
                let id = parts.get(1).copied().unwrap_or("").trim();
                if id.is_empty() {
                    CommandResult::Handled("Usage: /cron enable <id>".to_string())
                } else {
                    CommandResult::Action(rockbot_chat::CommandAction::SetStatus(
                        format!("Cron job '{id}' enabled"),
                        false,
                    ))
                }
            }
            "disable" => {
                let id = parts.get(1).copied().unwrap_or("").trim();
                if id.is_empty() {
                    CommandResult::Handled("Usage: /cron disable <id>".to_string())
                } else {
                    CommandResult::Action(rockbot_chat::CommandAction::SetStatus(
                        format!("Cron job '{id}' disabled"),
                        false,
                    ))
                }
            }
            "" => CommandResult::Handled("Usage: /cron [list|enable|disable <id>]".to_string()),
            other => CommandResult::Handled(format!("Unknown cron subcommand: {other}")),
        }
    }
}
