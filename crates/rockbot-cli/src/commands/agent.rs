//! Agent management commands

use anyhow::Result;
use std::path::PathBuf;
use crate::{AgentCommands, load_config};

/// Run agent commands
pub async fn run(command: &AgentCommands, config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path).await?;
    
    match command {
        AgentCommands::List => {
            println!("🤖 Configured Agents:");
            for agent in &config.agents.list {
                println!("   • {} (model: {})", 
                    agent.id, 
                    agent.model.as_ref().unwrap_or(&config.agents.defaults.model)
                );
                if let Some(workspace) = &agent.workspace {
                    println!("     workspace: {}", workspace.display());
                }
            }
        }
        AgentCommands::Status { agent_id } => {
            println!("📊 Agent Status: {}", agent_id);
            println!("   Agent status coming soon...");
        }
        AgentCommands::Message { agent_id, session, message } => {
            println!("💬 Sending message to agent '{}' (session: {})", agent_id, session);
            println!("   Message: {}", message);
            println!("   Agent messaging coming soon...");
        }
        AgentCommands::Create { agent_id, workspace, model } => {
            println!("➕ Creating agent: {}", agent_id);
            if let Some(workspace) = workspace {
                println!("   Workspace: {}", workspace.display());
            }
            if let Some(model) = model {
                println!("   Model: {}", model);
            }
            println!("   Agent creation coming soon...");
        }
    }
    
    Ok(())
}