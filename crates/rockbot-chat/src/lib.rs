//! Shared chat command API — leaf crate for slash command extensibility.
//!
//! Consumed by both `rockbot-tui` (terminal) and `rockbot-webui` (browser).
//! Each domain crate registers its own commands via `register_chat_commands()`.

use serde::{Deserialize, Serialize};

/// Result of executing a chat slash command.
#[derive(Debug, Clone)]
pub enum CommandResult {
    /// Display this text in the active chat as a system message.
    Handled(String),
    /// Side effect only — the harness should perform the action.
    Action(CommandAction),
    /// Not handled by this command — try next in registry.
    NotHandled,
}

/// Actions a command can request from the chat harness (TUI or WebUI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandAction {
    /// Exit the application.
    Quit,
    /// Set the status bar message. `(message, is_error)`
    SetStatus(String, bool),
    /// Switch the active navigation mode by name.
    SwitchMode(String),
    /// Show an overlay with the given markdown content.
    ShowOverlay(String),
    /// Clear the current chat history.
    ClearChat,
    /// Send a message to a specific agent. `(agent_id, message)`
    SendToAgent(String, String),
    /// Route a message to the built-in Butler companion.
    SendToButler(String),
    /// The command has spawned async work — no immediate display.
    SpawnAsync,
}

/// Metadata for `/help` display.
#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub usage: &'static str,
}

/// Minimal context passed to command handlers.
///
/// Commands don't get direct UI state — they work through this interface.
pub struct CommandContext {
    pub tx: tokio::sync::mpsc::UnboundedSender<String>,
    pub gateway_url: String,
    pub active_agent_id: Option<String>,
    pub active_session_key: Option<String>,
}

/// Trait for chat slash commands — implemented by each domain crate.
pub trait ChatCommand: Send + Sync {
    /// Return metadata about this command (name, aliases, description).
    fn info(&self) -> CommandInfo;

    /// Execute the command with the given arguments and context.
    fn execute(&self, args: &str, ctx: &CommandContext) -> CommandResult;
}

/// Registry collecting all slash commands from across crates.
pub struct ChatCommandRegistry {
    commands: Vec<Box<dyn ChatCommand>>,
}

fn is_valid_agent_route_id(agent_id: &str) -> bool {
    !agent_id.is_empty()
        && agent_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

impl Default for ChatCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatCommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Register a new chat command.
    pub fn register(&mut self, cmd: Box<dyn ChatCommand>) {
        self.commands.push(cmd);
    }

    /// Dispatch an input line (starting with `/`) to the first matching command.
    ///
    /// Returns `CommandResult::NotHandled` if no command matches.
    pub fn dispatch(&self, input: &str, ctx: &CommandContext) -> CommandResult {
        let input = input.trim();
        if !input.starts_with('/') {
            return CommandResult::NotHandled;
        }

        let (cmd_name, args) = match input[1..].split_once(char::is_whitespace) {
            Some((name, rest)) => (name, rest.trim()),
            None => (&input[1..], ""),
        };

        for cmd in &self.commands {
            let info = cmd.info();
            if info.name == cmd_name || info.aliases.contains(&cmd_name) {
                return cmd.execute(args, ctx);
            }
        }

        CommandResult::NotHandled
    }

    /// List all registered commands (for `/help` display).
    pub fn list_commands(&self) -> Vec<CommandInfo> {
        self.commands.iter().map(|c| c.info()).collect()
    }

    /// Find commands matching the current slash-command prefix.
    pub fn matching_commands(&self, input: &str) -> Vec<CommandInfo> {
        let input = input.trim_start();
        if !input.starts_with('/') {
            return Vec::new();
        }

        let command_part = input[1..]
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();

        self.list_commands()
            .into_iter()
            .filter(|info| {
                command_part.is_empty()
                    || info.name.to_ascii_lowercase().starts_with(&command_part)
                    || info
                        .aliases
                        .iter()
                        .any(|alias| alias.to_ascii_lowercase().starts_with(&command_part))
            })
            .collect()
    }
}

/// Parse `$@agent-id message` syntax for direct agent routing.
///
/// Returns `Some((agent_id, message))` if the input matches, `None` otherwise.
pub fn parse_agent_route(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();
    if !input.starts_with("$@") {
        return None;
    }

    let rest = &input[2..];
    match rest.split_once(char::is_whitespace) {
        Some((agent_id, message)) if is_valid_agent_route_id(agent_id) => {
            Some((agent_id, message.trim()))
        }
        None if is_valid_agent_route_id(rest) => Some((rest, "")),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_agent_route() {
        assert_eq!(
            parse_agent_route("$@main hello world"),
            Some(("main", "hello world"))
        );
        assert_eq!(parse_agent_route("$@agent-1"), Some(("agent-1", "")));
        assert_eq!(parse_agent_route("$@../secrets nope"), None);
        assert_eq!(parse_agent_route("hello"), None);
        assert_eq!(parse_agent_route("/help"), None);
        assert_eq!(parse_agent_route("$@"), None);
    }

    struct TestCmd;
    impl ChatCommand for TestCmd {
        fn info(&self) -> CommandInfo {
            CommandInfo {
                name: "test",
                aliases: &["t"],
                description: "A test command",
                usage: "/test [args]",
            }
        }
        fn execute(&self, args: &str, _ctx: &CommandContext) -> CommandResult {
            CommandResult::Handled(format!("test: {args}"))
        }
    }

    #[test]
    fn test_registry_dispatch() {
        let mut registry = ChatCommandRegistry::new();
        registry.register(Box::new(TestCmd));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = CommandContext {
            tx,
            gateway_url: String::new(),
            active_agent_id: None,
            active_session_key: None,
        };

        match registry.dispatch("/test hello", &ctx) {
            CommandResult::Handled(msg) => assert_eq!(msg, "test: hello"),
            _ => panic!("expected Handled"),
        }

        match registry.dispatch("/t world", &ctx) {
            CommandResult::Handled(msg) => assert_eq!(msg, "test: world"),
            _ => panic!("expected Handled via alias"),
        }

        assert!(matches!(
            registry.dispatch("/unknown", &ctx),
            CommandResult::NotHandled
        ));
        assert!(matches!(
            registry.dispatch("not a command", &ctx),
            CommandResult::NotHandled
        ));
    }

    #[test]
    fn test_matching_commands() {
        struct MixedCaseCmd;
        impl ChatCommand for MixedCaseCmd {
            fn info(&self) -> CommandInfo {
                CommandInfo {
                    name: "Test",
                    aliases: &["T"],
                    description: "A test command",
                    usage: "/test [args]",
                }
            }

            fn execute(&self, _args: &str, _ctx: &CommandContext) -> CommandResult {
                CommandResult::Handled("ok".to_string())
            }
        }

        let mut registry = ChatCommandRegistry::new();
        registry.register(Box::new(MixedCaseCmd));

        let matches = registry.matching_commands("/te");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Test");

        let alias_matches = registry.matching_commands("/t");
        assert_eq!(alias_matches.len(), 1);
        assert_eq!(alias_matches[0].name, "Test");
    }
}
