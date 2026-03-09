//! Configuration system for Krabbykrus
//! 
//! This module provides TOML-based configuration with validation, environment variable
//! substitution, and hot reloading capabilities.

use crate::error::{ConfigError, Result};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Main configuration structure for Krabbykrus
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Gateway server configuration
    pub gateway: GatewayConfig,
    /// Agent configurations
    pub agents: AgentConfig,
    /// Tool configurations
    pub tools: ToolConfig,
    /// Security settings
    pub security: SecurityConfig,
    /// Credential management settings
    #[serde(default)]
    pub credentials: CredentialsConfig,
    /// LLM provider settings
    #[serde(default)]
    pub providers: ProvidersConfig,
}

/// Gateway server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Host to bind to (default: 127.0.0.1)
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    /// Port to bind to (default: 18080)
    #[serde(default = "default_port")]
    pub port: u16,
    /// Maximum concurrent connections
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,
    /// Require API key for programmatic access (default: false for localhost, true otherwise)
    #[serde(default)]
    pub require_api_key: Option<bool>,
}

impl GatewayConfig {
    /// Check if this gateway binds to localhost only
    pub fn is_localhost(&self) -> bool {
        matches!(self.bind_host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }
    
    /// Check if API key authentication is required
    pub fn requires_api_key(&self) -> bool {
        self.require_api_key.unwrap_or_else(|| !self.is_localhost())
    }
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Default settings for all agents
    pub defaults: AgentDefaults,
    /// List of configured agents
    pub list: Vec<AgentInstance>,
}

/// Default agent settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    /// Default workspace directory
    #[serde(default = "default_workspace")]
    pub workspace: PathBuf,
    /// Default model to use
    #[serde(default = "default_model")]
    pub model: String,
    /// Heartbeat interval in human-readable format (e.g., "5m", "30s")
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: String,
    /// Maximum context size in tokens
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
}

/// Individual agent instance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInstance {
    /// Agent identifier
    pub id: String,
    /// Workspace directory (optional, uses default if not specified)
    pub workspace: Option<PathBuf>,
    /// Model override
    pub model: Option<String>,
    /// Maximum number of tool calls per turn (default: 10)
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: Option<u32>,
    /// Parent agent ID (for subagents)
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Custom system prompt override
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Whether this agent is enabled (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Agent-specific configuration
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Tool system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Tool profile: "minimal", "standard", "full"
    #[serde(default = "default_tool_profile")]
    pub profile: String,
    /// Tools to explicitly deny
    #[serde(default)]
    pub deny: Vec<String>,
    /// Tool-specific configurations
    #[serde(default)]
    pub configs: HashMap<String, HashMap<String, serde_json::Value>>,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Sandbox configuration
    pub sandbox: SandboxConfig,
    /// Capability restrictions
    #[serde(default)]
    pub capabilities: CapabilityConfig,
}

/// Credentials configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialsConfig {
    /// Enable credential management
    #[serde(default = "default_credentials_enabled")]
    pub enabled: bool,
    /// Path to the vault directory
    #[serde(default = "default_vault_path")]
    pub vault_path: PathBuf,
    /// Unlock method: "password", "env", "keyring"
    #[serde(default = "default_unlock_method")]
    pub unlock_method: String,
    /// Environment variable name for password (when unlock_method = "env")
    #[serde(default = "default_password_env_var")]
    pub password_env_var: String,
    /// Auto-lock timeout in seconds (0 = never)
    #[serde(default)]
    pub auto_lock_timeout: u64,
    /// Default permission level for unspecified paths
    #[serde(default = "default_default_permission")]
    pub default_permission: String,
}

impl Default for CredentialsConfig {
    fn default() -> Self {
        Self {
            enabled: default_credentials_enabled(),
            vault_path: default_vault_path(),
            unlock_method: default_unlock_method(),
            password_env_var: default_password_env_var(),
            auto_lock_timeout: 0,
            default_permission: default_default_permission(),
        }
    }
}

/// LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Anthropic provider settings
    #[serde(default)]
    pub anthropic: AnthropicProviderConfig,
    /// OpenAI provider settings
    #[serde(default)]
    pub openai: OpenAiProviderConfig,
    /// AWS Bedrock provider settings
    #[serde(default)]
    pub bedrock: BedrockProviderConfig,
    /// Ollama provider settings
    #[serde(default)]
    pub ollama: OllamaProviderConfig,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            anthropic: AnthropicProviderConfig::default(),
            openai: OpenAiProviderConfig::default(),
            bedrock: BedrockProviderConfig::default(),
            ollama: OllamaProviderConfig::default(),
        }
    }
}

/// Anthropic provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicProviderConfig {
    /// Authentication mode: "auto", "api", or "oauth"
    /// - "auto": Try OAuth first (Claude Code), fall back to API key
    /// - "api": Use API key only (ANTHROPIC_API_KEY env var or vault)
    /// - "oauth": Use OAuth only (Claude Code credentials)
    #[serde(default = "default_anthropic_auth_mode")]
    pub auth_mode: String,
    /// API endpoint URL override for API key auth (default: https://api.anthropic.com)
    #[serde(default)]
    pub api_url: Option<String>,
    /// API endpoint URL override for OAuth auth (default: https://api.claude.ai)
    #[serde(default)]
    pub oauth_api_url: Option<String>,
    /// OAuth token refresh URL override  
    #[serde(default)]
    pub oauth_token_url: Option<String>,
    /// Whether this provider is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for AnthropicProviderConfig {
    fn default() -> Self {
        Self {
            auth_mode: default_anthropic_auth_mode(),
            api_url: None,
            oauth_api_url: None,
            oauth_token_url: None,
            enabled: true,
        }
    }
}

/// OpenAI provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiProviderConfig {
    /// API endpoint URL override (for Azure OpenAI, etc.)
    #[serde(default)]
    pub api_url: Option<String>,
    /// Whether this provider is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for OpenAiProviderConfig {
    fn default() -> Self {
        Self {
            api_url: None,
            enabled: true,
        }
    }
}

/// AWS Bedrock provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BedrockProviderConfig {
    /// AWS region
    #[serde(default = "default_aws_region")]
    pub region: String,
    /// Whether this provider is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for BedrockProviderConfig {
    fn default() -> Self {
        Self {
            region: default_aws_region(),
            enabled: true,
        }
    }
}

/// Ollama provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaProviderConfig {
    /// Ollama server URL
    #[serde(default = "default_ollama_url")]
    pub url: String,
    /// Whether this provider is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for OllamaProviderConfig {
    fn default() -> Self {
        Self {
            url: default_ollama_url(),
            enabled: true,
        }
    }
}

fn default_anthropic_auth_mode() -> String {
    "auto".to_string()
}

fn default_aws_region() -> String {
    "us-east-1".to_string()
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_true() -> bool {
    true
}

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Sandbox mode: "disabled", "tools", "all"
    #[serde(default = "default_sandbox_mode")]
    pub mode: String,
    /// Sandbox scope: "session", "tool", "none"
    #[serde(default = "default_sandbox_scope")]
    pub scope: String,
    /// Container image for sandboxing
    pub image: Option<String>,
}

/// Capability restrictions
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityConfig {
    /// Filesystem access restrictions
    pub filesystem: Option<FilesystemCapabilities>,
    /// Network access restrictions
    pub network: Option<NetworkCapabilities>,
    /// Process execution restrictions
    pub process: Option<ProcessCapabilities>,
}

/// Filesystem capability restrictions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemCapabilities {
    /// Allowed read paths
    pub read_paths: Vec<PathBuf>,
    /// Allowed write paths
    pub write_paths: Vec<PathBuf>,
    /// Explicitly forbidden paths
    pub forbidden_paths: Vec<PathBuf>,
}

/// Network capability restrictions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkCapabilities {
    /// Allowed domains
    pub allowed_domains: Vec<String>,
    /// Blocked domains
    pub blocked_domains: Vec<String>,
    /// Maximum request size in bytes
    pub max_request_size: Option<usize>,
}

/// Process execution capability restrictions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCapabilities {
    /// Allowed commands/executables
    pub allowed_commands: Vec<String>,
    /// Blocked commands/executables
    pub blocked_commands: Vec<String>,
    /// Maximum execution time in seconds
    pub max_execution_time: Option<u64>,
}

/// Configuration watcher for hot reloading
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<Config>,
}

impl Config {
    /// Load configuration from a TOML file
    pub async fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let content = tokio::fs::read_to_string(path).await.map_err(|_| {
            ConfigError::FileNotFound {
                path: path.to_path_buf(),
            }
        })?;
        
        Self::from_toml(&content)
    }
    
    /// Parse configuration from TOML string
    pub fn from_toml(content: &str) -> Result<Self> {
        // Perform environment variable substitution
        let expanded_content = expand_env_vars(content)?;
        
        let config: Config = toml::from_str(&expanded_content)?;
        config.validate()?;
        Ok(config)
    }
    
    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        // Validate gateway config
        if self.gateway.port == 0 {
            return Err(ConfigError::Invalid {
                message: "Gateway port cannot be 0".to_string(),
            }
            .into());
        }
        
        // Validate agent IDs are unique
        let mut agent_ids = std::collections::HashSet::new();
        for agent in &self.agents.list {
            if !agent_ids.insert(&agent.id) {
                return Err(ConfigError::Invalid {
                    message: format!("Duplicate agent ID: {}", agent.id),
                }
                .into());
            }
        }
        
        // Validate tool profile
        match self.tools.profile.as_str() {
            "minimal" | "standard" | "full" => {}
            _ => {
                return Err(ConfigError::Invalid {
                    message: format!("Invalid tool profile: {}", self.tools.profile),
                }
                .into());
            }
        }
        
        // Validate sandbox mode
        match self.security.sandbox.mode.as_str() {
            "disabled" | "tools" | "all" => {}
            _ => {
                return Err(ConfigError::Invalid {
                    message: format!("Invalid sandbox mode: {}", self.security.sandbox.mode),
                }
                .into());
            }
        }
        
        Ok(())
    }
    
    /// Create a configuration watcher for hot reloading
    pub fn watch<P: AsRef<Path>>(path: P) -> Result<ConfigWatcher> {
        let path = path.as_ref().to_path_buf();
        let path_for_closure = path.clone();
        let (tx, rx) = mpsc::channel(16);
        
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_)) && event.paths.contains(&path_for_closure) {
                        debug!("Config file changed, reloading...");
                        
                        // Reload configuration
                        match std::fs::read_to_string(&path_for_closure) {
                            Ok(content) => match Config::from_toml(&content) {
                                Ok(config) => {
                                    info!("Configuration reloaded successfully");
                                    if let Err(_) = tx.try_send(config) {
                                        warn!("Failed to send config update (channel full)");
                                    }
                                }
                                Err(e) => {
                                    warn!("Failed to reload config: {}", e);
                                }
                            },
                            Err(e) => {
                                warn!("Failed to read config file: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Config watcher error: {}", e);
                }
            }
        })?;
        
        watcher.watch(&path, RecursiveMode::NonRecursive)?;
        
        Ok(ConfigWatcher {
            _watcher: watcher,
            rx,
        })
    }
}

impl ConfigWatcher {
    /// Get the next configuration update
    pub async fn next_update(&mut self) -> Option<Config> {
        self.rx.recv().await
    }
}

/// Expand environment variables in configuration strings
fn expand_env_vars(content: &str) -> Result<String> {
    let mut result = content.to_string();
    
    // Simple environment variable expansion: ${VAR} or ${VAR:default}
    let re = regex::Regex::new(r"\$\{([^}:]+)(?::([^}]*))?\}").unwrap();
    
    while let Some(caps) = re.captures(&result) {
        let full_match = caps.get(0).unwrap().as_str();
        let var_name = caps.get(1).unwrap().as_str();
        let default_value = caps.get(2).map(|m| m.as_str());
        
        let replacement = match std::env::var(var_name) {
            Ok(value) => value,
            Err(_) => {
                if let Some(default) = default_value {
                    default.to_string()
                } else {
                    return Err(ConfigError::EnvVarNotFound {
                        var: var_name.to_string(),
                    }
                    .into());
                }
            }
        };
        
        result = result.replace(full_match, &replacement);
    }
    
    Ok(result)
}

// Default value functions
fn default_bind_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    18080
}

fn default_max_connections() -> usize {
    1000
}

fn default_request_timeout() -> u64 {
    30
}

fn default_workspace() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("krabbykrus")
        .join("workspace")
}

fn default_model() -> String {
    "anthropic/claude-sonnet-4-20250514".to_string()
}

fn default_heartbeat_interval() -> String {
    "5m".to_string()
}

fn default_max_context_tokens() -> usize {
    128000
}

fn default_max_tool_calls() -> Option<u32> {
    Some(10)
}

fn default_tool_profile() -> String {
    "standard".to_string()
}

fn default_sandbox_mode() -> String {
    "tools".to_string()
}

fn default_sandbox_scope() -> String {
    "session".to_string()
}

fn default_credentials_enabled() -> bool {
    true
}

fn default_vault_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("krabbykrus")
        .join("vault")
}

fn default_unlock_method() -> String {
    "password".to_string()
}

fn default_password_env_var() -> String {
    "RUSTCLAW_VAULT_PASSWORD".to_string()
}

fn default_default_permission() -> String {
    "deny".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_parsing() {
        let toml_content = r#"
            [gateway]
            bind_host = "0.0.0.0"
            port = 8080
            
            [agents.defaults]
            workspace = "/tmp/workspace"
            model = "test-model"
            
            [[agents.list]]
            id = "main"
            
            [tools]
            profile = "full"
            
            [security.sandbox]
            mode = "all"
        "#;
        
        let config = Config::from_toml(toml_content).unwrap();
        assert_eq!(config.gateway.bind_host, "0.0.0.0");
        assert_eq!(config.gateway.port, 8080);
        assert_eq!(config.agents.list.len(), 1);
        assert_eq!(config.agents.list[0].id, "main");
        assert_eq!(config.tools.profile, "full");
    }
    
    #[test]
    fn test_env_var_expansion() {
        std::env::set_var("TEST_VAR", "test_value");
        
        let content = "value = \"${TEST_VAR}\"";
        let expanded = expand_env_vars(content).unwrap();
        assert_eq!(expanded, "value = \"test_value\"");
        
        let content_with_default = "value = \"${NONEXISTENT:default_val}\"";
        let expanded = expand_env_vars(content_with_default).unwrap();
        assert_eq!(expanded, "value = \"default_val\"");
    }
    
    #[test]
    fn test_config_validation() {
        let mut config = Config {
            gateway: GatewayConfig {
                bind_host: "127.0.0.1".to_string(),
                port: 8080,
                max_connections: 1000,
                request_timeout: 30,
                require_api_key: None,
            },
            agents: AgentConfig {
                defaults: AgentDefaults {
                    workspace: PathBuf::from("/tmp"),
                    model: "test".to_string(),
                    heartbeat_interval: "5m".to_string(),
                    max_context_tokens: 128000,
                },
                list: vec![],
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
            providers: ProvidersConfig::default(),
        };

        assert!(config.validate().is_ok());
        
        // Test invalid port
        config.gateway.port = 0;
        assert!(config.validate().is_err());
    }
}

// Conversion traits for interoperability with subcrate types

impl From<ToolConfig> for krabbykrus_tools::ToolConfig {
    fn from(config: ToolConfig) -> Self {
        Self {
            profile: config.profile,
            deny: config.deny,
            configs: config.configs,
        }
    }
}

impl From<SecurityConfig> for krabbykrus_security::SecurityConfig {
    fn from(config: SecurityConfig) -> Self {
        Self {
            sandbox: krabbykrus_security::SandboxConfig {
                mode: config.sandbox.mode,
                scope: config.sandbox.scope,
                image: config.sandbox.image,
            },
            capabilities: krabbykrus_security::CapabilityConfig {
                filesystem: config.capabilities.filesystem.map(|fs| {
                    krabbykrus_security::FilesystemCapabilities {
                        read_paths: fs.read_paths,
                        write_paths: fs.write_paths,
                        forbidden_paths: fs.forbidden_paths,
                    }
                }),
                network: config.capabilities.network.map(|net| {
                    krabbykrus_security::NetworkCapabilities {
                        allowed_domains: net.allowed_domains,
                        blocked_domains: net.blocked_domains,
                        max_request_size: net.max_request_size,
                    }
                }),
                process: config.capabilities.process.map(|proc| {
                    krabbykrus_security::ProcessCapabilities {
                        allowed_commands: proc.allowed_commands,
                        blocked_commands: proc.blocked_commands,
                        max_execution_time: proc.max_execution_time,
                    }
                }),
            },
        }
    }
}