//! Agent management commands

use crate::{load_config, AgentCommands};
use anyhow::Result;
use std::path::PathBuf;

/// Run agent commands
pub async fn run(command: &AgentCommands, config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path).await?;

    match command {
        AgentCommands::List => {
            println!("🤖 Configured Agents:");
            for agent in &config.agents.list {
                println!(
                    "   • {} (model: {})",
                    agent.id,
                    agent
                        .model
                        .as_ref()
                        .unwrap_or(&config.agents.defaults.model)
                );
                if let Some(workspace) = &agent.workspace {
                    println!("     workspace: {}", workspace.display());
                }
            }
        }
        AgentCommands::Status { agent_id } => {
            println!("📊 Agent Status: {agent_id}");
            println!("   Agent status coming soon...");
        }
        AgentCommands::Message {
            agent_id,
            session,
            message,
        } => {
            println!("💬 Sending message to agent '{agent_id}' (session: {session})");
            println!("   Message: {message}");
            println!("   Agent messaging coming soon...");
        }
        AgentCommands::Create {
            agent_id,
            workspace,
            model,
        } => {
            println!("➕ Creating agent: {agent_id}");
            if let Some(workspace) = workspace {
                println!("   Workspace: {}", workspace.display());
            }
            if let Some(model) = model {
                println!("   Model: {model}");
            }
            println!("   Agent creation coming soon...");
        }
        AgentCommands::Run {
            agent_id,
            gateway,
            exec,
        } => {
            let gateway_target = gateway.clone().unwrap_or_else(|| {
                format!(
                    "{}:{}",
                    config.client.gateway_host, config.client.client_port
                )
            });
            run_agent_session(agent_id, &gateway_target, config.effective_pki(), *exec).await?;
        }
        AgentCommands::Extract { agent_id, out } => {
            let runtime =
                rockbot_storage_runtime::StorageRuntime::new(config_path, &config).await?;
            let out_root = out.clone().unwrap_or_else(|| {
                rockbot_storage_utility::default_agent_extract_dir(config_path)
            });
            let manifest =
                rockbot_storage_utility::extract_agent_vdisk(&runtime, agent_id, &out_root).await?;
            println!("📦 Extracted agent '{}' to {}", agent_id, out_root.join(agent_id).display());
            println!("   Source vdisk: {}", manifest.source_vdisk);
            println!("   Files:");
            for file in manifest.files {
                println!("   • {} ({} bytes)", file.name, file.size_bytes);
            }
        }
    }

    Ok(())
}

/// Run an interactive agent session via a remote gateway.
async fn run_agent_session(
    agent_id: &str,
    gateway_url: &str,
    pki: rockbot_config::PkiConfig,
    _exec: bool,
) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let ws_url = rockbot_client::normalize_gateway_url(gateway_url);

    println!("Connecting to gateway at {ws_url}...");
    let client = rockbot_client::GatewayClient::connect_with_pki(&ws_url, Some(&pki));
    let mut events = client.subscribe();

    // Wait for connection
    let mut connected = false;
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if client.is_connected() {
            connected = true;
            break;
        }
    }
    if !connected {
        anyhow::bail!("Failed to connect to gateway at {gateway_url}");
    }
    println!("Connected. Type messages to send to agent '{agent_id}'. Press Ctrl+D to exit.\n");

    let session_key = format!("cli-{}", uuid::Uuid::new_v4());
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        tokio::select! {
            // Read user input
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let msg = line.trim().to_string();
                        line.clear();
                        if msg.is_empty() {
                            continue;
                        }
                        if let Err(e) = client.send_agent_message(agent_id, &session_key, &msg).await {
                            eprintln!("Send error: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("Input error: {e}");
                        break;
                    }
                }
            }
            // Process gateway events
            event = events.recv() => {
                match event {
                    Ok(rockbot_client::GatewayEvent::StreamChunk { delta, .. }) => {
                        print!("{delta}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                    Ok(rockbot_client::GatewayEvent::AgentResponse { content, .. }) => {
                        if !content.is_empty() {
                            println!("{content}");
                        }
                        println!();
                    }
                    Ok(rockbot_client::GatewayEvent::AgentError { error, .. }) => {
                        eprintln!("Error: {error}");
                    }
                    Ok(rockbot_client::GatewayEvent::Disconnected { reason }) => {
                        eprintln!("Disconnected: {reason}");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }

    println!("\nSession ended.");
    Ok(())
}
