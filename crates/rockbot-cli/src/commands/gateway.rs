//! Gateway server command implementation

use anyhow::Result;
use rockbot_core::{Config, Gateway, Agent, VaultCredentialAccessor};
use rockbot_core::config::AgentInstance;
use rockbot_core::session::SessionManager;
use rockbot_tools::ToolRegistry;
use rockbot_memory::MemoryManager;
use rockbot_security::SecurityManager;
use rockbot_llm::LlmProviderRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::process::Command;
use tracing::{debug, error, info, warn};

use crate::GatewayCommands;
use crate::commands::vault_unlock::unlock_vault_for_gateway;

/// Run gateway commands
pub async fn run(command: &GatewayCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        GatewayCommands::Run => run_server(config_path).await,
        GatewayCommands::Start => start_service().await,
        GatewayCommands::Stop => stop_service().await,
        GatewayCommands::Restart => restart_service().await,
        GatewayCommands::Status => show_status().await,
        GatewayCommands::Install { system, name } => install_service(*system, name, config_path).await,
        GatewayCommands::Remove { system, name } => remove_service(*system, name).await,
        GatewayCommands::Logs { lines, follow } => show_logs(*lines, *follow).await,
    }
}

/// Run the gateway server in foreground
async fn run_server(config_path: &PathBuf) -> Result<()> {
    // Load configuration
    let config = Config::from_file(config_path).await?;
    
    // Initialize core components
    info!("Initializing RockBot gateway...");
    
    // Determine vault path from config
    let vault_path = config.credentials.vault_path.clone();
    
    // Check if we're running interactively (TTY)
    let interactive = atty::is(atty::Stream::Stdin);
    
    // Only unlock vault if credentials are enabled
    let vault_result = if config.credentials.enabled {
        match unlock_vault_for_gateway(
            &vault_path,
            interactive,
            Some(config.credentials.password_env_var.as_str()),
        ).await {
            Ok(result) => {
                info!("Credential vault ready");
                Some(result)
            }
            Err(e) => {
                warn!("Could not unlock credential vault: {}. Continuing with environment variables only.", e);
                None
            }
        }
    } else {
        debug!("Credential management disabled in config");
        None
    };
    
    // Create credential accessor if vault is available
    let credential_accessor: Option<Arc<dyn rockbot_tools::CredentialAccessor>> = 
        vault_result.as_ref().map(|r| {
            Arc::new(VaultCredentialAccessor::new(r.manager.clone())) 
                as Arc<dyn rockbot_tools::CredentialAccessor>
        });
    
    // Create session manager
    let db_path = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("rockbot")
        .join("data")
        .join("sessions.db");
    
    tokio::fs::create_dir_all(db_path.parent().unwrap()).await?;
    let session_manager = Arc::new(SessionManager::new(&db_path, 1000).await?);
    
    // Create gateway
    let mut gateway = Gateway::new(config.clone(), session_manager.clone()).await?;
    gateway.set_config_path(config_path.clone());
    
    // Initialize other components
    let tool_registry = Arc::new(ToolRegistry::new(rockbot_core::gateway::convert_tool_config(config.tools.clone())).await?);
    let security_manager = Arc::new(SecurityManager::new(rockbot_core::gateway::convert_security_config(config.security.clone())).await?);
    // Create LLM registry (Anthropic uses Claude Code OAuth automatically)
    let llm_registry = Arc::new(LlmProviderRegistry::new().await?);
    
    // Create agent factory for hot reload
    let defaults = config.agents.defaults.clone();
    let tr = tool_registry.clone();
    let sm = security_manager.clone();
    let sess = session_manager.clone();
    let llm = llm_registry.clone();
    let cred_accessor = credential_accessor.clone();
    
    let agent_factory: rockbot_core::gateway::AgentFactory = Arc::new(move |agent_config: AgentInstance| {
        let defaults = defaults.clone();
        let tr = tr.clone();
        let sm = sm.clone();
        let sess = sess.clone();
        let llm = llm.clone();
        let cred_accessor = cred_accessor.clone();
        
        Box::pin(async move {
            let model = agent_config.model.as_ref()
                .unwrap_or(&defaults.model);
            
            let llm_provider = llm.get_provider_for_model(model).await
                .map_err(|e| rockbot_core::error::GatewayError::InvalidRequest {
                    message: e.to_string(),
                })?;
            
            let workspace = agent_config.workspace.as_ref()
                .unwrap_or(&defaults.workspace);
            let memory_manager = Arc::new(MemoryManager::new(workspace.clone()).await
                .map_err(|e| rockbot_core::error::GatewayError::InvalidRequest {
                    message: e.to_string(),
                })?);
            
            let agent = Agent::new(
                agent_config,
                llm_provider,
                tr,
                memory_manager,
                sm,
                sess,
                cred_accessor,
                None,
                None,
            ).await.map_err(|e| rockbot_core::error::GatewayError::InvalidRequest {
                message: e.to_string(),
            })?;
            
            Ok(Arc::new(agent))
        })
    });
    
    gateway.set_agent_factory(agent_factory);
    gateway.set_llm_registry(llm_registry.clone()).await;

    // Create agents (gracefully handle missing API keys)
    let mut agents_created = 0;
    let mut agents_pending = 0;
    
    for agent_config in &config.agents.list {
        let agent_id = &agent_config.id;
        let model = agent_config.model.as_ref()
            .unwrap_or(&config.agents.defaults.model);
        
        // Try to get LLM provider - mark as pending if API key missing
        let llm_provider = match llm_registry.get_provider_for_model(model).await {
            Ok(provider) => provider,
            Err(e) => {
                let reason = format!("{e}");
                tracing::warn!(
                    "Agent '{}' pending: {} (use POST /api/gateway/reload after adding credentials)",
                    agent_id, reason
                );
                gateway.add_pending_agent(agent_config.clone(), reason).await;
                agents_pending += 1;
                continue;
            }
        };
        
        info!("Creating agent: {}", agent_id);
        
        // Create memory manager for this agent
        let workspace = agent_config.workspace.as_ref()
            .unwrap_or(&config.agents.defaults.workspace);
        let memory_manager = Arc::new(MemoryManager::new(workspace.clone()).await?);
        
        // Create agent
        let invoker = gateway.agent_invoker();
        let agent = Arc::new(Agent::new(
            agent_config.clone(),
            llm_provider,
            tool_registry.clone(),
            memory_manager,
            security_manager.clone(),
            session_manager.clone(),
            credential_accessor.clone(),
            None,
            Some(invoker),
        ).await?);

        // Register with gateway
        gateway.register_agent(agent).await;
        agents_created += 1;
    }
    
    if agents_pending > 0 {
        info!(
            "Gateway initialized: {} agent(s) active, {} pending (missing API keys)",
            agents_created, agents_pending
        );
        info!("Add credentials, then POST /api/gateway/reload to activate pending agents");
    } else {
        info!("Gateway initialized: {} agent(s) active", agents_created);
    }

    // Register agent-as-tool entries for agents with expose_as_tool config
    gateway.register_agent_tools().await;

    // Start the cron scheduler background loop
    gateway.start_cron_scheduler().await;

    // Set up signal handling
    let gateway_clone = gateway.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(_) => {
                info!("Received shutdown signal");
                if let Err(e) = gateway_clone.shutdown().await {
                    error!("Error during shutdown: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to listen for shutdown signal: {}", e);
            }
        }
    });
    
    // Start the gateway
    gateway.start().await?;
    
    Ok(())
}

/// Get the service name based on whether it's user or system level
#[allow(dead_code)]
fn get_service_name(_system: bool) -> &'static str {
    "rockbot-gateway"
}

/// Start the gateway service
async fn start_service() -> Result<()> {
    // Try user service first, then system
    let status = Command::new("systemctl")
        .args(["--user", "start", "rockbot-gateway"])
        .status();
    
    match status {
        Ok(s) if s.success() => {
            println!("✅ Gateway service started (user)");
            Ok(())
        }
        _ => {
            // Try system service
            let status = Command::new("systemctl")
                .args(["start", "rockbot-gateway"])
                .status()?;
            
            if status.success() {
                println!("✅ Gateway service started (system)");
                Ok(())
            } else {
                anyhow::bail!("Failed to start gateway service. Is it installed? Run 'rockbot gateway install'")
            }
        }
    }
}

/// Stop the gateway service
async fn stop_service() -> Result<()> {
    // Try user service first, then system
    let status = Command::new("systemctl")
        .args(["--user", "stop", "rockbot-gateway"])
        .status();
    
    match status {
        Ok(s) if s.success() => {
            println!("✅ Gateway service stopped (user)");
            Ok(())
        }
        _ => {
            // Try system service
            let status = Command::new("systemctl")
                .args(["stop", "rockbot-gateway"])
                .status()?;
            
            if status.success() {
                println!("✅ Gateway service stopped (system)");
                Ok(())
            } else {
                anyhow::bail!("Failed to stop gateway service")
            }
        }
    }
}

/// Restart the gateway service
async fn restart_service() -> Result<()> {
    // Try user service first, then system
    let status = Command::new("systemctl")
        .args(["--user", "restart", "rockbot-gateway"])
        .status();
    
    match status {
        Ok(s) if s.success() => {
            println!("✅ Gateway service restarted (user)");
            Ok(())
        }
        _ => {
            // Try system service
            let status = Command::new("systemctl")
                .args(["restart", "rockbot-gateway"])
                .status()?;
            
            if status.success() {
                println!("✅ Gateway service restarted (system)");
                Ok(())
            } else {
                anyhow::bail!("Failed to restart gateway service")
            }
        }
    }
}

/// Show gateway service status
async fn show_status() -> Result<()> {
    println!("Checking gateway service status...\n");
    
    // Check user service
    println!("=== User Service ===");
    let _ = Command::new("systemctl")
        .args(["--user", "status", "rockbot-gateway", "--no-pager"])
        .status();
    
    println!("\n=== System Service ===");
    let _ = Command::new("systemctl")
        .args(["status", "rockbot-gateway", "--no-pager"])
        .status();
    
    // Also try to hit the health endpoint
    println!("\n=== Gateway Health ===");
    match reqwest::Client::new()
        .get("http://127.0.0.1:18080/health")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let health: serde_json::Value = resp.json().await?;
            println!("Gateway is running:");
            println!("  Version: {}", health.get("version").and_then(|v| v.as_str()).unwrap_or("unknown"));
            println!("  Agents: {}", health.get("agents").and_then(|v| v.as_array()).map_or(0, std::vec::Vec::len));
            println!("  Active sessions: {}", health.get("active_sessions").and_then(serde_json::Value::as_u64).unwrap_or(0));
        }
        Ok(resp) => {
            println!("Gateway responded with status: {}", resp.status());
        }
        Err(_) => {
            println!("Gateway is not responding on http://127.0.0.1:18080");
        }
    }
    
    Ok(())
}

/// Install the gateway as a systemd service
async fn install_service(system: bool, name: &str, config_path: &Path) -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let config_path = config_path.canonicalize().unwrap_or_else(|_| config_path.to_path_buf());
    
    let service_content = if system {
        // System service
        format!(r#"[Unit]
Description=RockBot Gateway Server
After=network.target

[Service]
Type=simple
ExecStart={exe} --config {config} gateway run
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#,
            exe = exe_path.display(),
            config = config_path.display(),
        )
    } else {
        // User service
        format!(r#"[Unit]
Description=RockBot Gateway Server
After=default.target

[Service]
Type=simple
ExecStart={exe} --config {config} gateway run
Restart=on-failure
RestartSec=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
"#,
            exe = exe_path.display(),
            config = config_path.display(),
        )
    };

    let service_path = if system {
        PathBuf::from(format!("/etc/systemd/system/{name}.service"))
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("systemd/user")
            .join(format!("{name}.service"))
    };

    // Create parent directory if needed
    if let Some(parent) = service_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write service file
    std::fs::write(&service_path, &service_content)?;
    println!("✅ Created service file: {}", service_path.display());

    // Reload systemd
    if system {
        let status = Command::new("systemctl")
            .args(["daemon-reload"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Failed to reload systemd (are you root?)");
        }
    } else {
        let status = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Failed to reload systemd user daemon");
        }
    }

    // Enable the service
    if system {
        let status = Command::new("systemctl")
            .args(["enable", name])
            .status()?;
        if status.success() {
            println!("✅ Enabled system service: {name}");
        }
    } else {
        let status = Command::new("systemctl")
            .args(["--user", "enable", name])
            .status()?;
        if status.success() {
            println!("✅ Enabled user service: {name}");
        }
        
        // Enable lingering so service runs without login
        let username = std::env::var("USER").unwrap_or_default();
        if !username.is_empty() {
            let _ = Command::new("loginctl")
                .args(["enable-linger", &username])
                .status();
            println!("✅ Enabled lingering for user: {username}");
        }
    }

    println!();
    println!("Service installed successfully!");
    println!();
    println!("Commands:");
    if system {
        println!("  Start:   sudo systemctl start {name}");
        println!("  Stop:    sudo systemctl stop {name}");
        println!("  Status:  sudo systemctl status {name}");
        println!("  Logs:    sudo journalctl -u {name} -f");
    } else {
        println!("  Start:   systemctl --user start {name}");
        println!("  Stop:    systemctl --user stop {name}");
        println!("  Status:  systemctl --user status {name}");
        println!("  Logs:    journalctl --user -u {name} -f");
    }
    println!();
    println!("Or use: rockbot gateway start/stop/status/logs");

    Ok(())
}

/// Remove the gateway service
async fn remove_service(system: bool, name: &str) -> Result<()> {
    // Stop the service first
    if system {
        let _ = Command::new("systemctl")
            .args(["stop", name])
            .status();
        let _ = Command::new("systemctl")
            .args(["disable", name])
            .status();
    } else {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", name])
            .status();
        let _ = Command::new("systemctl")
            .args(["--user", "disable", name])
            .status();
    }

    // Remove service file
    let service_path = if system {
        PathBuf::from(format!("/etc/systemd/system/{name}.service"))
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
            .join("systemd/user")
            .join(format!("{name}.service"))
    };

    if service_path.exists() {
        std::fs::remove_file(&service_path)?;
        println!("✅ Removed service file: {}", service_path.display());
    } else {
        println!("Service file not found: {}", service_path.display());
    }

    // Reload systemd
    if system {
        let _ = Command::new("systemctl")
            .args(["daemon-reload"])
            .status();
    } else {
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }

    println!("✅ Service removed");
    Ok(())
}

/// Show service logs
async fn show_logs(lines: usize, follow: bool) -> Result<()> {
    let mut args = vec![
        "--user".to_string(),
        "-u".to_string(),
        "rockbot-gateway".to_string(),
        "-n".to_string(),
        lines.to_string(),
    ];
    
    if follow {
        args.push("-f".to_string());
    }

    // Try user logs first
    let status = Command::new("journalctl")
        .args(&args)
        .status();
    
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            // Try system logs
            args.remove(0); // Remove --user
            let status = Command::new("journalctl")
                .args(&args)
                .status()?;
            
            if !status.success() {
                anyhow::bail!("Failed to retrieve logs");
            }
            Ok(())
        }
    }
}
