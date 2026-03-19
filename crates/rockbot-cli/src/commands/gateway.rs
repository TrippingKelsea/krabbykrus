//! Gateway server command implementation

use anyhow::Result;
use chrono::Utc;
use rockbot_agent::{Agent, VaultCredentialAccessor};
use rockbot_config::{config::AgentInstance, Config};
use rockbot_credentials::{ClusterNodeRole, RegisteredNodeKey};
use rockbot_gateway::{convert_security_config, convert_tool_config, Gateway};
use rockbot_llm::LlmProviderRegistry;
use rockbot_memory::MemoryManager;
use rockbot_pki::PkiManager;
use rockbot_security::SecurityManager;
use rockbot_session::SessionManager;
#[cfg(feature = "overseer")]
use rockbot_storage::Store;
use rockbot_storage_runtime::{StorageRuntime, StoreKind};
use rockbot_tools::ToolRegistry;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::commands::vault_unlock::unlock_vault_for_gateway;
use crate::GatewayCommands;

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" || s.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(s.strip_prefix("~/").unwrap_or(""));
        }
    }
    path.to_path_buf()
}

/// Run gateway commands
pub async fn run(command: &GatewayCommands, config_path: &PathBuf) -> Result<()> {
    match command {
        GatewayCommands::Run => run_server(config_path).await,
        GatewayCommands::Start => start_service().await,
        GatewayCommands::Stop => stop_service().await,
        GatewayCommands::Restart => restart_service().await,
        GatewayCommands::Status => show_status(config_path).await,
        GatewayCommands::Install { system, name } => {
            install_service(*system, name, config_path).await
        }
        GatewayCommands::Remove { system, name } => remove_service(*system, name).await,
        GatewayCommands::Logs { lines, follow } => show_logs(*lines, *follow).await,
    }
}

/// Run the gateway server in foreground
async fn run_server(config_path: &PathBuf) -> Result<()> {
    // Load configuration
    #[allow(unused_mut)]
    let mut config = Config::from_file(config_path).await?;

    // Initialize core components
    info!("Initializing RockBot gateway v{}...", env!("CARGO_PKG_VERSION"));

    // Determine vault path from config
    let vault_path = config.credentials.vault_path.clone();

    // Check if we're running interactively (TTY)
    let interactive = std::io::stdin().is_terminal();

    // Only unlock vault if credentials are enabled
    let vault_result = if config.credentials.enabled {
        match unlock_vault_for_gateway(
            &vault_path,
            interactive,
            Some(config.credentials.password_env_var.as_str()),
        )
        .await
        {
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

    let storage_runtime = StorageRuntime::new(config_path, &config).await?;
    if let Ok(plan) = storage_runtime.plan() {
        info!(
            "Resolved storage plan for {} store(s) under {}",
            plan.stores.len(),
            plan.storage_root.display()
        );
        for store in &plan.stores {
            info!(
                "Storage plan: {} -> {:?} ({})",
                store.label,
                store.resolution,
                store.descriptor
            );
        }
    }
    let pki_manager = storage_runtime.pki_manager();
    if let (Some(vault_result), Some(pki_manager)) = (vault_result.as_ref(), pki_manager) {
        bootstrap_local_vault_node(&config, pki_manager, &vault_result.manager).await;
    }
    let opened_sessions = if probe_store(config_path, StoreKind::Sessions).unwrap_or(false) {
        storage_runtime.open_sessions_store().await?
    } else {
        warn!("Sessions store probe failed. Using recovery session store.");
        storage_runtime.open_sessions_recovery_store().await?
    };
    let session_manager = Arc::new(
        SessionManager::new_with_store(
            opened_sessions.store,
            1000,
            &opened_sessions.descriptor,
        )
        .await?,
    );

    let mut vault_store: Option<Arc<rockbot_storage::Store>> = None;
    if vault_result.is_some() {
        let agents_probe_ok = probe_store(config_path, StoreKind::Agents).unwrap_or(false);
        match if agents_probe_ok {
            storage_runtime.open_agents_store(&vault_path).await
        } else {
            warn!("Agents store probe failed. Skipping virtual-disk agent store; agent config remains bootstrap-only and runtime changes will not persist.");
            Err(anyhow::anyhow!("agent store probe failed"))
        } {
            Ok(opened_agents) => {
                let store = opened_agents.store;
                #[cfg(feature = "overseer")]
                ensure_default_overseer_config(&mut config, store.as_ref())?;
                info!("Vault store opened for agent persistence via {}", opened_agents.descriptor);
                vault_store = Some(store);
            }
            Err(e) => {
                warn!("Could not open vault store: {e}. Agent config remains bootstrap-only and runtime changes will not persist.");
            }
        }
    }

    #[cfg(feature = "overseer")]
    if config.overseer.is_none() {
        config.overseer = Some(serde_json::to_value(
            rockbot_overseer::OverseerConfig::default(),
        )?);
    }

    // Create gateway
    let mut gateway = Gateway::new(
        config.clone(),
        session_manager.clone(),
        vault_result.as_ref().map(|result| result.manager.clone()),
    )
    .await?;
    gateway.set_config_path(config_path.clone());

    // Initialize other components
    let tool_config = convert_tool_config(config.tools.clone());
    let tool_registry = Arc::new(ToolRegistry::new_core_only(tool_config.clone()).await?);
    rockbot_tools_system::register_profile_tools(tool_registry.as_ref(), &tool_config).await?;
    let security_manager =
        Arc::new(SecurityManager::new(convert_security_config(config.security.clone())).await?);
    let mut llm_registry = LlmProviderRegistry::new().await?;
    register_compiled_llm_providers(
        &mut llm_registry,
        vault_result
            .as_ref()
            .map(|result| &result.llm_credentials),
    )
    .await?;
    let llm_registry = Arc::new(llm_registry);

    // Create agent factory for hot reload
    let defaults = config.agents.defaults.clone();
    let tr = tool_registry.clone();
    let sm = security_manager.clone();
    let sess = session_manager.clone();
    let llm = llm_registry.clone();
    let cred_accessor = credential_accessor.clone();
    let agent_storage_root = storage_runtime.storage_root().to_path_buf();

    let agent_factory: rockbot_gateway::gateway::AgentFactory =
        Arc::new(move |agent_config: AgentInstance| {
            let defaults = defaults.clone();
            let tr = tr.clone();
            let sm = sm.clone();
            let sess = sess.clone();
            let llm = llm.clone();
            let cred_accessor = cred_accessor.clone();
            let agent_storage_root = agent_storage_root.clone();

            Box::pin(async move {
                let model = agent_config.model.as_ref().unwrap_or(&defaults.model);

                let llm_provider = llm.get_provider_for_model(model).await.map_err(|e| {
                    rockbot_gateway::error::GatewayError::InvalidRequest {
                        message: e.to_string(),
                    }
                })?;

                let memory_root = agent_storage_root.join("runtime").join("memory");
                let memory_manager =
                    Arc::new(MemoryManager::new(memory_root).await.map_err(|e| {
                        rockbot_gateway::error::GatewayError::InvalidRequest {
                            message: e.to_string(),
                        }
                    })?);

                let mut agent = Agent::new(
                    agent_config,
                    llm_provider,
                    tr,
                    memory_manager,
                    sm,
                    sess,
                    cred_accessor,
                    None,
                    None,
                )
                .await
                .map_err(|e| {
                    rockbot_gateway::error::GatewayError::InvalidRequest {
                        message: e.to_string(),
                    }
                })?;
                agent.set_storage_root(agent_storage_root);

                Ok(Arc::new(agent))
            })
        });

    gateway.set_agent_factory(agent_factory);
    gateway.set_llm_registry(llm_registry.clone()).await;

    // Open vault store for agent persistence (if vault is available)
    if let Some(store) = vault_store.clone() {
        gateway.set_store(store);
    }

    // Auto-migrate agents from TOML to vault if needed
    gateway.auto_migrate_agents_to_store().await;

    // Load agents: prefer vault store, fall back to TOML config
    let agent_configs: Vec<AgentInstance> = {
        let vault_agents = gateway.load_agents_from_store();
        if vault_agents.is_empty() {
            config.agents.list.clone()
        } else {
            info!("Loaded {} agent(s) from vault store", vault_agents.len());
            vault_agents
        }
    };

    // Create agents (gracefully handle missing API keys)
    let mut agents_created = 0;
    let mut agents_pending = 0;

    for agent_config in &agent_configs {
        let agent_id = &agent_config.id;
        let model = agent_config
            .model
            .as_ref()
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
                gateway
                    .add_pending_agent(agent_config.clone(), reason)
                    .await;
                agents_pending += 1;
                continue;
            }
        };

        info!("Creating agent: {}", agent_id);

        // Create memory manager for this agent
        let memory_root = storage_runtime.storage_root().join("runtime").join("memory");
        let memory_manager = Arc::new(MemoryManager::new(memory_root).await?);

        // Create agent
        let invoker = gateway.agent_invoker();
        let mut agent = Agent::new(
                agent_config.clone(),
                llm_provider,
                tool_registry.clone(),
                memory_manager,
                security_manager.clone(),
                session_manager.clone(),
                credential_accessor.clone(),
                None,
                Some(invoker),
            )
            .await?;
        agent.set_storage_root(storage_runtime.storage_root().to_path_buf());
        let agent = Arc::new(agent);

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
    } else if agents_created == 0 {
        info!("Gateway initialized: 0 agent(s) active");
        info!(
            "No agents are currently configured in the replicated store. \
             Providers are ready, but the gateway will not serve model requests until an agent is created."
        );
        info!("Create an agent from the TUI or via POST /api/agents.");
    } else {
        info!("Gateway initialized: {} agent(s) active", agents_created);
    }

    // Register agent-as-tool entries for agents with expose_as_tool config
    gateway.register_agent_tools().await;

    // Start the cron scheduler background loop
    gateway.start_cron_scheduler().await;

    // Publish CA cert to S3 if deploy is configured
    #[cfg(feature = "bedrock-deploy")]
    gateway.publish_ca_to_s3().await;

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

fn probe_store(config_path: &Path, kind: StoreKind) -> Result<bool> {
    let exe = std::env::current_exe()?;
    let store = match kind {
        StoreKind::Vault => "vault",
        StoreKind::Agents => "agents",
        StoreKind::Sessions => "sessions",
        StoreKind::Cron => "cron",
        StoreKind::Routing => "routing",
        StoreKind::Topology => "topology",
    };
    let status = Command::new(exe)
        .arg("storage")
        .arg("probe")
        .arg("--config")
        .arg(config_path)
        .arg(store)
        .status()?;
    Ok(status.success())
}

async fn bootstrap_local_vault_node(
    config: &Config,
    pki_manager: &PkiManager,
    manager: &Arc<rockbot_credentials::CredentialManager>,
) {
    let node_id = local_node_id(config);
    match pki_manager.ensure_vault_keypair(&node_id) {
        Ok(keypair) => {
            let record = RegisteredNodeKey {
                node_id: node_id.clone(),
                identity_fingerprint: None,
                vault_public_key: keypair.public_key,
                roles: local_node_roles(config),
                active: true,
                created_at: Utc::now(),
                rotated_at: None,
                revoked_at: None,
            };
            if let Err(e) = manager.register_node_key(record).await {
                warn!("Failed to register local vault node '{}': {}", node_id, e);
            } else {
                info!("Registered local vault node '{}'", node_id);
            }
        }
        Err(e) => {
            warn!(
                "Failed to ensure local vault keypair for '{}': {}",
                node_id, e
            );
        }
    }
}

fn local_node_id(config: &Config) -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if config.security.roles.gateway {
                "gateway".to_string()
            } else {
                "node".to_string()
            }
        })
}

fn local_node_roles(config: &Config) -> Vec<ClusterNodeRole> {
    let mut roles = Vec::new();
    if config.security.roles.gateway {
        roles.push(ClusterNodeRole::Gateway);
    }
    if config.security.roles.vault_provider {
        roles.push(ClusterNodeRole::VaultProvider);
    }
    if config.security.roles.client {
        roles.push(ClusterNodeRole::Client);
    }
    if config.security.roles.admin {
        roles.push(ClusterNodeRole::Admin);
    }
    roles
}

async fn register_compiled_llm_providers(
    registry: &mut LlmProviderRegistry,
    #[allow(unused_variables)]
    llm_credentials: Option<&std::collections::HashMap<String, String>>,
) -> Result<()> {
    #[cfg(feature = "bedrock")]
    {
        match rockbot_llm_bedrock::BedrockProvider::from_env().await {
            Ok(provider) => {
                tracing::info!("Registered AWS Bedrock provider");
                registry.register_provider(Arc::new(provider)).await;
            }
            Err(e) => {
                tracing::debug!("Bedrock provider not available: {}", e);
            }
        }
    }

    #[cfg(feature = "anthropic")]
    {
        let provider = llm_credentials
            .and_then(|creds| creds.get("anthropic").cloned())
            .map(rockbot_llm_anthropic::AnthropicProvider::with_api_key)
            .map(Ok)
            .unwrap_or_else(rockbot_llm_anthropic::AnthropicProvider::new);
        if let Ok(provider) = provider {
            tracing::info!("Registered Anthropic provider");
            registry.register_provider(Arc::new(provider)).await;
        }
    }

    #[cfg(feature = "openai")]
    {
        let provider = llm_credentials
            .and_then(|creds| creds.get("openai").cloned())
            .map(rockbot_llm_openai::OpenAiProvider::with_api_key)
            .map(Ok)
            .unwrap_or_else(rockbot_llm_openai::OpenAiProvider::new);
        if let Ok(provider) = provider {
            tracing::info!("Registered OpenAI provider");
            registry.register_provider(Arc::new(provider)).await;
        }
    }

    #[cfg(feature = "ollama")]
    {
        let base_url =
            std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".to_string());
        let provider = rockbot_llm_ollama::OllamaProvider::with_base_url(base_url);
        tracing::info!("Registered Ollama provider");
        registry.register_provider(Arc::new(provider)).await;
    }

    Ok(())
}

#[cfg(feature = "overseer")]
fn ensure_default_overseer_config(config: &mut Config, store: &Store) -> Result<()> {
    const NS: &str = "app_config";
    const KEY: &str = "overseer";

    if let Some(bytes) = store.kv_get(NS, KEY)? {
        let stored: rockbot_overseer::OverseerConfig = serde_json::from_slice(&bytes)?;
        config.overseer = Some(serde_json::to_value(stored)?);
        return Ok(());
    }

    let overseer = config
        .overseer
        .clone()
        .map(serde_json::from_value::<rockbot_overseer::OverseerConfig>)
        .transpose()?
        .unwrap_or_default();
    store.kv_put(NS, KEY, &serde_json::to_vec(&overseer)?)?;
    config.overseer = Some(serde_json::to_value(overseer)?);
    Ok(())
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
async fn show_status(config_path: &Path) -> Result<()> {
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
    let config = Config::from_file(config_path).await.unwrap_or_default();
    let gateway_url = format!("https://127.0.0.1:{}", config.gateway.port);
    let client = if let Some(ca_path) = config
        .effective_pki()
        .tls_ca
        .as_ref()
        .map(|path| expand_tilde(path.as_path()))
    {
        if ca_path.exists() {
            let ca_pem = tokio::fs::read(&ca_path).await?;
            let ca_cert = reqwest::Certificate::from_pem(&ca_pem)?;
            reqwest::Client::builder()
                .add_root_certificate(ca_cert)
                .build()?
        } else {
            println!(
                "Gateway health check skipped: configured CA file not found at {}",
                ca_path.display()
            );
            return Ok(());
        }
    } else {
        println!("Gateway health check skipped: no CA configured in [pki].tls_ca");
        return Ok(());
    };
    match client
        .get(format!("{gateway_url}/health"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let health: serde_json::Value = resp.json().await?;
            println!("Gateway is running:");
            println!(
                "  Version: {}",
                health
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
            );
            println!(
                "  Agents: {}",
                health
                    .get("agents")
                    .and_then(|v| v.as_array())
                    .map_or(0, std::vec::Vec::len)
            );
            println!(
                "  Active sessions: {}",
                health
                    .get("active_sessions")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            );
        }
        Ok(resp) => {
            println!("Gateway responded with status: {}", resp.status());
        }
        Err(_) => {
            println!("Gateway is not responding on {gateway_url}");
        }
    }

    Ok(())
}

/// Install the gateway as a systemd service
async fn install_service(system: bool, name: &str, config_path: &Path) -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let config_path = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());

    let service_content = if system {
        // System service
        format!(
            r#"[Unit]
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
        format!(
            r#"[Unit]
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
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            })
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
        let status = Command::new("systemctl").args(["daemon-reload"]).status()?;
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
        let status = Command::new("systemctl").args(["enable", name]).status()?;
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
        let _ = Command::new("systemctl").args(["stop", name]).status();
        let _ = Command::new("systemctl").args(["disable", name]).status();
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
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config")
            })
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
        let _ = Command::new("systemctl").args(["daemon-reload"]).status();
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
    let status = Command::new("journalctl").args(&args).status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            // Try system logs
            args.remove(0); // Remove --user
            let status = Command::new("journalctl").args(&args).status()?;

            if !status.success() {
                anyhow::bail!("Failed to retrieve logs");
            }
            Ok(())
        }
    }
}
