//! Configuration management commands

use anyhow::Result;
use rockbot_core::Config;
use rockbot_core::config::*;
use std::collections::HashMap;
use std::path::PathBuf;
use crate::{ConfigCommands, load_config};

/// Run configuration commands
pub async fn run(command: &ConfigCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        ConfigCommands::Show => show_config(config_path).await,
        ConfigCommands::Validate => validate_config(config_path).await,
        ConfigCommands::Init { output, force } => {
            init_config(output.as_ref().unwrap_or(config_path), *force).await
        }
    }
}

/// Show current configuration
async fn show_config(config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path).await?;
    
    let toml_string = toml::to_string_pretty(&config)?;
    println!("{toml_string}");
    
    Ok(())
}

/// Validate configuration
async fn validate_config(config_path: &PathBuf) -> Result<()> {
    match load_config(config_path).await {
        Ok(config) => {
            println!("✅ Configuration is valid");
            println!("   Gateway: {}:{}", config.gateway.bind_host, config.gateway.port);
            println!("   Agents: {} configured", config.agents.list.len());
            println!("   Tools: {} profile", config.tools.profile);
            println!("   Security: {} sandbox", config.security.sandbox.mode);
        }
        Err(e) => {
            println!("❌ Configuration is invalid: {e}");
            std::process::exit(1);
        }
    }
    
    Ok(())
}

/// Generate default configuration
async fn init_config(output_path: &PathBuf, force: bool) -> Result<()> {
    if output_path.exists() && !force {
        anyhow::bail!(
            "Configuration file already exists: {}\nUse --force to overwrite",
            output_path.display()
        );
    }
    
    // Create default configuration
    let default_config = create_default_config();
    
    // Create parent directory if it doesn't exist
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    
    // Write configuration
    let toml_string = toml::to_string_pretty(&default_config)?;
    tokio::fs::write(output_path, toml_string).await?;
    
    println!("✅ Default configuration created at {}", output_path.display());
    println!("   Edit the file to customize your setup");
    
    Ok(())
}

/// Create a default configuration
fn create_default_config() -> Config {
    Config {
        gateway: GatewayConfig {
            bind_host: "127.0.0.1".to_string(),
            port: 18080,
            max_connections: 1000,
            request_timeout: 30,
            require_api_key: None, // Auto-detect: false for localhost, true otherwise
        },
        agents: AgentConfig {
            defaults: AgentDefaults {
                workspace: dirs::config_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                    .join("rockbot")
                    .join("workspace"),
                model: "anthropic/claude-sonnet-4-20250514".to_string(),
                heartbeat_interval: "5m".to_string(),
                max_context_tokens: 128000,
            },
            list: vec![
                AgentInstance {
                    id: "main".to_string(),
                    workspace: None,
                    model: None,
                    max_tool_calls: None,
                    temperature: Some(0.3),
                    max_tokens: Some(16000),
                    parent_id: None,
                    system_prompt: None,
                    enabled: true,
                    mcp_servers: HashMap::new(),
                    config: HashMap::new(),
                    max_context_tokens: 128000,
                    guardrails: Vec::new(),
                    reflection_enabled: false,
                    breakpoint_tools: Vec::new(),
                    planning_mode: "never".to_string(),
                    expose_as_tool: None,
                    episodic_memory: false,
                    workflow: None,
                },
            ],
        },
        tools: ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        },
        security: SecurityConfig {
            sandbox: SandboxConfig {
                mode: "tools".to_string(),
                scope: "session".to_string(),
                image: None,
            },
            capabilities: CapabilityConfig::default(),
        },
        credentials: CredentialsConfig::default(),
        providers: rockbot_core::ProvidersConfig::default(),
    }
}