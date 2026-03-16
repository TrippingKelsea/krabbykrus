//! Core TUI slash commands: /exit, /help, /clear, /mode, /alerts

use rockbot_chat::{ChatCommand, CommandAction, CommandContext, CommandInfo, CommandResult};

/// Register TUI-local chat commands.
pub fn register_chat_commands(registry: &mut rockbot_chat::ChatCommandRegistry) {
    registry.register(Box::new(ExitCommand));
    registry.register(Box::new(HelpCommand));
    registry.register(Box::new(ClearCommand));
    registry.register(Box::new(ModeCommand));
    registry.register(Box::new(AlertsCommand));
}

struct ExitCommand;
impl ChatCommand for ExitCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "exit",
            aliases: &["quit", "q"],
            description: "Exit the TUI",
            usage: "/exit",
        }
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Action(CommandAction::Quit)
    }
}

struct HelpCommand;
impl ChatCommand for HelpCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "help",
            aliases: &["h", "?"],
            description: "Show available commands",
            usage: "/help",
        }
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        // The actual help listing is built by the registry — we signal the harness
        CommandResult::Action(CommandAction::ShowOverlay("help".to_string()))
    }
}

struct ClearCommand;
impl ChatCommand for ClearCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "clear",
            aliases: &["cls"],
            description: "Clear the chat history",
            usage: "/clear",
        }
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Action(CommandAction::ClearChat)
    }
}

struct ModeCommand;
impl ChatCommand for ModeCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "mode",
            aliases: &[],
            description: "Switch navigation mode",
            usage: "/mode <dashboard|agents|sessions|credentials|cron|models|settings>",
        }
    }
    fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
        let mode = args.trim();
        if mode.is_empty() {
            return CommandResult::Handled("Usage: /mode <name>".to_string());
        }
        CommandResult::Action(CommandAction::SwitchMode(mode.to_string()))
    }
}

struct AlertsCommand;
impl ChatCommand for AlertsCommand {
    fn info(&self) -> CommandInfo {
        CommandInfo {
            name: "alerts",
            aliases: &[],
            description: "Show alerts overlay",
            usage: "/alerts",
        }
    }
    fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
        CommandResult::Action(CommandAction::ShowOverlay("alerts".to_string()))
    }
}
