//! Configuration management commands

use crate::{load_config, ConfigCommands};
use anyhow::Result;
use rockbot_core::config::*;
use rockbot_core::Config;
use std::collections::HashMap;
use std::path::PathBuf;

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
            println!(
                "   Gateway: {}:{}",
                config.gateway.bind_host, config.gateway.port
            );
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

    // Create parent directory if it doesn't exist
    let config_dir = output_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    tokio::fs::create_dir_all(config_dir).await?;

    // Generate self-signed TLS certificate
    let cert_path = config_dir.join("gateway.crt");
    let key_path = config_dir.join("gateway.key");

    if !cert_path.exists() || force {
        super::cert::generate_self_signed_cert(&cert_path, &key_path, &[], 365).await?;
        println!("   TLS cert: {}", cert_path.display());
        println!("   TLS key:  {}", key_path.display());
    }

    // Create default configuration with TLS paths
    let mut default_config = create_default_config();
    default_config.gateway.pki.tls_cert = Some(cert_path);
    default_config.gateway.pki.tls_key = Some(key_path);

    // Write configuration
    let toml_string = toml::to_string_pretty(&default_config)?;
    tokio::fs::write(output_path, toml_string).await?;

    println!("Default configuration created at {}", output_path.display());
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
            pki: PkiConfig::default(),
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
            list: vec![AgentInstance {
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
                llm_timeout_secs: 45,
                tool_timeout_secs: 120,
            }],
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
        overseer: None,
    }
}
