//! /butler slash command handlers.
//!
//! These commands are intercepted before agent processing and return
//! formatted text directly without LLM involvement.

/// Result of a slash command — either rendered output or "not my command".
pub enum CommandResult {
    /// Command was handled; return this text to the user.
    Handled(String),
    /// Not a butler command — pass through to normal processing.
    NotHandled,
}

/// Dispatch a `/butler <subcommand>` message.
///
/// Returns `CommandResult::Handled(output)` if the message is a butler
/// command, or `CommandResult::NotHandled` if it should be passed through.
pub fn dispatch(input: &str) -> CommandResult {
    let trimmed = input.trim();
    if !trimmed.starts_with("/butler") {
        return CommandResult::NotHandled;
    }

    let rest = trimmed.trim_start_matches("/butler").trim();
    let subcmd = rest.split_whitespace().next().unwrap_or("");

    match subcmd {
        "status" => CommandResult::Handled(status()),
        "mood" => CommandResult::Handled(mood()),
        "help" | "" => CommandResult::Handled(help()),
        other => CommandResult::Handled(format!(
            "Darling, I don't know `{other}`. Here's what I do know:\n\n{}",
            help()
        )),
    }
}

fn status() -> String {
    "Butler is here, darling. What do you need?".to_string()
}

fn help() -> String {
    "## Butler Commands\n\n\
     | Command | Description |\n\
     |---------|-------------|\n\
     | `/butler status` | Check if Butler is awake |\n\
     | `/butler mood` | Ask how Butler is feeling |\n\
     | `/butler help` | This help message |"
        .to_string()
}

fn mood() -> String {
    "Feeling fabulous, thanks for asking.".to_string()
}
