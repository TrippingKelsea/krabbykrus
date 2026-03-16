//! Session management commands

use crate::{load_config, SessionCommands};
use anyhow::Result;
use std::path::PathBuf;

/// Run session commands
pub async fn run(command: &SessionCommands, config_path: &PathBuf) -> Result<()> {
    let _config = load_config(config_path).await?;

    match command {
        SessionCommands::List { agent, active } => {
            println!("📋 Sessions (agent: {agent:?}, active: {active})");
            println!("   Session management coming soon...");
        }
        SessionCommands::Show { session_id } => {
            println!("📄 Session: {session_id}");
            println!("   Session details coming soon...");
        }
        SessionCommands::History { session_id, limit } => {
            println!("💬 History for session {session_id} (limit: {limit})");
            println!("   Message history coming soon...");
        }
        SessionCommands::Archive { session_id } => {
            println!("📦 Archiving session: {session_id}");
            println!("   Session archiving coming soon...");
        }
        SessionCommands::Delete { session_id, force } => {
            if *force || confirm_delete(session_id)? {
                println!("🗑️  Deleting session: {session_id}");
                println!("   Session deletion coming soon...");
            }
        }
    }

    Ok(())
}

fn confirm_delete(session_id: &str) -> Result<bool> {
    println!("Are you sure you want to delete session '{session_id}'? [y/N]");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    Ok(input == "y" || input == "yes")
}
