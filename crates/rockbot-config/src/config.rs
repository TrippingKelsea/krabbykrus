//! Configuration system for RockBot
//!
//! This module provides TOML-based configuration with validation, environment variable
//! substitution, and hot reloading capabilities.

use crate::error::ConfigError;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Result type for config operations
pub type Result<T> = std::result::Result<T, ConfigError>;

/// Main configuration structure for RockBot
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
    /// Overseer configuration (requires `overseer` feature).
    /// Stored as raw Value so the config always deserializes even without the feature.
    #[serde(default)]
    pub overseer: Option<serde_json::Value>,
    /// Doctor AI configuration (requires `doctor-ai` feature).
    /// Stored as raw Value so the config always deserializes even without the feature.
    #[serde(default)]
    pub doctor: Option<serde_json::Value>,
    /// Deploy configuration (requires `bedrock-deploy` feature).
    /// Stored as raw Value so the config always deserializes even without the feature.
    #[serde(default)]
    pub deploy: Option<serde_json::Value>,
    /// TUI display preferences
    #[serde(default)]
    pub tui: TuiConfig,
    /// Shared local model configuration for butler, doctor, and overseer.
    /// Provides a single `[seed_model]` section to avoid duplicating model
    /// coordinates across components.
    #[serde(default)]
    pub seed_model: SeedModelConfig,
}

/// TUI display preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Show the top navigation bar as a floating overlay (default: true)
    #[serde(default = "default_true")]
    pub floating_bar: bool,
    /// Enable animated transitions and effects (default: true)
    #[serde(default = "default_true")]
    pub animations: bool,
    /// Color theme for the TUI
    #[serde(default)]
    pub color_theme: ColorTheme,
    /// Animation style for modal transitions
    #[serde(default)]
    pub animation_style: AnimationStyle,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            floating_bar: true,
            animations: true,
            color_theme: ColorTheme::default(),
            animation_style: AnimationStyle::default(),
        }
    }
}

/// Color theme for the TUI accent colors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorTheme {
    #[default]
    Purple,
    Blue,
    Green,
    Rose,
    Amber,
    Mono,
}

impl ColorTheme {
    pub fn all() -> &'static [Self] {
        &[
            Self::Purple,
            Self::Blue,
            Self::Green,
            Self::Rose,
            Self::Amber,
            Self::Mono,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Purple => "Purple",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Rose => "Rose",
            Self::Amber => "Amber",
            Self::Mono => "Mono",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Purple => Self::Blue,
            Self::Blue => Self::Green,
            Self::Green => Self::Rose,
            Self::Rose => Self::Amber,
            Self::Amber => Self::Mono,
            Self::Mono => Self::Purple,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Purple => Self::Mono,
            Self::Blue => Self::Purple,
            Self::Green => Self::Blue,
            Self::Rose => Self::Green,
            Self::Amber => Self::Rose,
            Self::Mono => Self::Amber,
        }
    }
}

/// Animation style for modal and page transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AnimationStyle {
    #[default]
    Coalesce,
    Fade,
    Slide,
    None,
}

impl AnimationStyle {
    pub fn all() -> &'static [Self] {
        &[Self::Coalesce, Self::Fade, Self::Slide, Self::None]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Coalesce => "Coalesce",
            Self::Fade => "Fade",
            Self::Slide => "Slide",
            Self::None => "None",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Coalesce => Self::Fade,
            Self::Fade => Self::Slide,
            Self::Slide => Self::None,
            Self::None => Self::Coalesce,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::Coalesce => Self::None,
            Self::Fade => Self::Coalesce,
            Self::Slide => Self::Fade,
            Self::None => Self::Slide,
        }
    }
}

/// PKI and TLS configuration shared across gateway, client, and agent consumers.
///
/// When nested inside `[gateway]` it configures the server-side TLS listener.
/// When nested inside a client or agent section it configures outbound mTLS identity.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PkiConfig {
    /// Path to TLS certificate file (PEM) — server cert for gateway, client cert for agents/TUI.
    #[serde(default)]
    pub tls_cert: Option<std::path::PathBuf>,
    /// Path to TLS private key file (PEM).
    #[serde(default)]
    pub tls_key: Option<std::path::PathBuf>,
    /// Path to CA certificate for peer verification (enables mTLS).
    #[serde(default)]
    pub tls_ca: Option<std::path::PathBuf>,
    /// Require valid client certificate (mTLS) — only meaningful on the gateway/server side.
    /// When false + tls_ca is set: optional client auth (accepts but doesn't require).
    /// When true + tls_ca is set: mandatory mTLS.
    #[serde(default)]
    pub require_client_cert: bool,
    /// Path to the PKI directory (default: ~/.config/rockbot/pki/).
    #[serde(default)]
    pub pki_dir: Option<std::path::PathBuf>,
    /// Pre-shared key for CSR enrollment endpoint.
    /// If set, enables POST /api/cert/sign with PSK auth.
    #[serde(default)]
    pub enrollment_psk: Option<String>,
}

impl PkiConfig {
    /// Check if mTLS client verification is configured.
    pub fn has_mtls(&self) -> bool {
        self.tls_ca.is_some()
    }
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
    /// PKI / TLS settings for the gateway listener and mTLS.
    #[serde(default, flatten)]
    pub pki: PkiConfig,
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

    /// Check if mTLS client verification is configured (delegates to `pki`).
    pub fn has_mtls(&self) -> bool {
        self.pki.has_mtls()
    }
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Default settings for all agents
    pub defaults: AgentDefaults,
    /// List of configured agents.
    ///
    /// **Deprecated:** Agent configs should be stored in the vault instead.
    /// On first startup with a non-empty list and an empty vault, agents are
    /// auto-migrated. This field defaults to empty and will be removed in a
    /// future version.
    #[serde(default)]
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
    /// Maximum number of tool calls per turn (dynamic by default)
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: Option<u32>,
    /// LLM temperature (default: 0.3)
    #[serde(default = "default_temperature")]
    pub temperature: Option<f32>,
    /// LLM max response tokens (default: 16000)
    #[serde(default = "default_max_tokens")]
    pub max_tokens: Option<u32>,
    /// Parent agent ID (for subagents)
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Custom system prompt override
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Whether this agent is enabled (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// MCP server configurations for this agent
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
    /// Agent-specific configuration
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
    /// Maximum context window in tokens (default: 128000, inherited from defaults)
    #[serde(default = "default_max_context_tokens")]
    pub max_context_tokens: usize,
    /// Guardrails to enable (e.g. ["pii", "prompt_injection"])
    #[serde(default)]
    pub guardrails: Vec<String>,
    /// Enable reflection/self-critique after tool loop completes
    #[serde(default)]
    pub reflection_enabled: bool,
    /// Tool names that always require human approval (breakpoints)
    #[serde(default)]
    pub breakpoint_tools: Vec<String>,
    /// Planning mode: "never" (default), "auto", "always", "approval_required"
    #[serde(default = "default_planning_mode")]
    pub planning_mode: String,
    /// Expose this agent as a callable tool for other agents
    #[serde(default)]
    pub expose_as_tool: Option<AgentToolConfig>,
    /// Enable episodic memory (long-term cross-session recall)
    #[serde(default)]
    pub episodic_memory: bool,
    /// Optional workflow definition — if present, this agent acts as a DAG workflow
    /// dispatcher rather than a standard LLM-driven agent.
    #[serde(default)]
    pub workflow: Option<WorkflowDefinition>,
    /// Timeout in seconds for each LLM API call (default: 45)
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,
    /// Timeout in seconds for each individual tool execution (default: 120)
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

/// Configuration for exposing an agent as a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolConfig {
    /// Tool name visible to other agents
    pub tool_name: String,
    /// Description of what this agent-tool does
    pub description: String,
}

fn default_planning_mode() -> String {
    "never".to_string()
}

/// MCP server entry in agent config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Command to run the MCP server
    pub command: String,
    /// Command arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// Anthropic provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicProviderConfig {
    /// Authentication mode: "auto", "api", or "oauth"
    #[serde(default = "default_anthropic_auth_mode")]
    pub auth_mode: String,
    /// API endpoint URL override for API key auth
    #[serde(default)]
    pub api_url: Option<String>,
    /// API endpoint URL override for OAuth auth
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
    /// Authentication mode
    #[serde(default = "default_bedrock_auth_mode")]
    pub auth_mode: String,
    /// AgentCore credential provider name
    #[serde(default)]
    pub credential_provider_name: Option<String>,
    /// AgentCore OAuth2 auth flow
    #[serde(default)]
    pub agentcore_auth_flow: Option<String>,
    /// AgentCore OAuth2 scopes
    #[serde(default)]
    pub agentcore_scopes: Option<String>,
    /// AWS Secrets Manager ARN for AgentCore OAuth2 client credentials
    #[serde(default)]
    pub credentials_secret_arn: Option<String>,
}

impl Default for BedrockProviderConfig {
    fn default() -> Self {
        Self {
            region: default_aws_region(),
            enabled: true,
            auth_mode: default_bedrock_auth_mode(),
            credential_provider_name: None,
            agentcore_auth_flow: None,
            agentcore_scopes: None,
            credentials_secret_arn: None,
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

/// Shared seed model configuration for local GGUF model inference.
///
/// Used by butler, doctor, and overseer components. Provides a single
/// `[seed_model]` section so you don't need to repeat model coordinates
/// in `[overseer]`, `[doctor]`, and `[butler]` separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedModelConfig {
    /// HuggingFace model repo ID (default: "Qwen/Qwen2.5-1.5B-Instruct-GGUF")
    #[serde(default = "default_seed_model_id")]
    pub model_id: String,
    /// GGUF filename within the repo (default: "qwen2.5-1.5b-instruct-q4_k_m.gguf")
    #[serde(default = "default_seed_model_filename")]
    pub model_filename: String,
    /// HuggingFace repo ID for the tokenizer (default: "Qwen/Qwen2.5-1.5B-Instruct")
    #[serde(default = "default_seed_tokenizer")]
    pub tokenizer_repo: String,
}

fn default_seed_model_id() -> String {
    "Qwen/Qwen2.5-1.5B-Instruct-GGUF".to_string()
}

fn default_seed_model_filename() -> String {
    "qwen2.5-1.5b-instruct-q4_k_m.gguf".to_string()
}

fn default_seed_tokenizer() -> String {
    "Qwen/Qwen2.5-1.5B-Instruct".to_string()
}

impl Default for SeedModelConfig {
    fn default() -> Self {
        Self {
            model_id: default_seed_model_id(),
            model_filename: default_seed_model_filename(),
            tokenizer_repo: default_seed_tokenizer(),
        }
    }
}

fn default_anthropic_auth_mode() -> String {
    "auto".to_string()
}

fn default_aws_region() -> String {
    "us-east-1".to_string()
}

fn default_bedrock_auth_mode() -> String {
    "aws_credentials".to_string()
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
        let content =
            tokio::fs::read_to_string(path)
                .await
                .map_err(|_| ConfigError::FileNotFound {
                    path: path.to_path_buf(),
                })?;

        Self::from_toml(&content)
    }

    /// Parse configuration from TOML string
    pub fn from_toml(content: &str) -> Result<Self> {
        let expanded_content = expand_env_vars(content)?;
        let config: Config =
            toml::from_str(&expanded_content).map_err(|e| ConfigError::Invalid {
                message: e.to_string(),
            })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        if self.gateway.port == 0 {
            return Err(ConfigError::Invalid {
                message: "Gateway port cannot be 0".to_string(),
            });
        }

        let mut agent_ids = std::collections::HashSet::new();
        for agent in &self.agents.list {
            if !agent_ids.insert(&agent.id) {
                return Err(ConfigError::Invalid {
                    message: format!("Duplicate agent ID: {}", agent.id),
                });
            }
        }

        match self.tools.profile.as_str() {
            "minimal" | "standard" | "full" => {}
            _ => {
                return Err(ConfigError::Invalid {
                    message: format!("Invalid tool profile: {}", self.tools.profile),
                });
            }
        }

        match self.security.sandbox.mode.as_str() {
            "disabled" | "tools" | "all" => {}
            _ => {
                return Err(ConfigError::Invalid {
                    message: format!("Invalid sandbox mode: {}", self.security.sandbox.mode),
                });
            }
        }

        Ok(())
    }

    /// Create a configuration watcher for hot reloading
    pub fn watch<P: AsRef<Path>>(path: P) -> std::result::Result<ConfigWatcher, notify::Error> {
        let path = path.as_ref().to_path_buf();
        let path_for_closure = path.clone();
        let (tx, rx) = mpsc::channel(16);

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<Event>| match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_))
                        && event.paths.contains(&path_for_closure)
                    {
                        debug!("Config file changed, reloading...");

                        match std::fs::read_to_string(&path_for_closure) {
                            Ok(content) => match Config::from_toml(&content) {
                                Ok(config) => {
                                    info!("Configuration reloaded successfully");
                                    if tx.try_send(config).is_err() {
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
                    });
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
        .join("rockbot")
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
    None
}

fn default_temperature() -> Option<f32> {
    Some(0.3)
}

fn default_max_tokens() -> Option<u32> {
    Some(16000)
}

fn default_tool_profile() -> String {
    "standard".to_string()
}

fn default_llm_timeout_secs() -> u64 {
    45
}

fn default_tool_timeout_secs() -> u64 {
    120
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
        .join("rockbot")
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

// ---------------------------------------------------------------------------
// Workflow DAG types (pure data, used by AgentInstance.workflow)
// ---------------------------------------------------------------------------

/// A declarative workflow definition (DAG of agent nodes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    /// Nodes in the workflow (each maps to an agent)
    pub nodes: Vec<WorkflowNode>,
    /// Edges connecting nodes (data flow + conditions)
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
    /// Node IDs that receive the initial input
    pub entry_nodes: Vec<String>,
    /// Node IDs whose outputs form the final result
    #[serde(default)]
    pub exit_nodes: Vec<String>,
}

/// A single node in the workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    /// Unique identifier for this node within the workflow
    pub id: String,
    /// The agent ID to invoke for this node
    pub agent_id: String,
    /// Optional message template with `{input}` and `{output:node_id}` placeholders
    #[serde(default)]
    pub message_template: Option<String>,
}

/// An edge connecting two workflow nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    /// Source node ID
    pub from: String,
    /// Target node ID
    pub to: String,
    /// Condition for traversing this edge
    #[serde(default)]
    pub condition: EdgeCondition,
}

/// Condition for following a workflow edge.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EdgeCondition {
    /// Always follow this edge
    #[default]
    Always,
    /// Follow if the source node's output contains the given keyword
    Contains { keyword: String },
    /// Follow if the source node's output matches the given regex pattern
    Pattern { regex: String },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
                pki: PkiConfig::default(),
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
            overseer: None,
            doctor: None,
            deploy: None,
            tui: TuiConfig::default(),
            seed_model: SeedModelConfig::default(),
        };

        assert!(config.validate().is_ok());

        config.gateway.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_seed_model_defaults() {
        let config = SeedModelConfig::default();
        assert_eq!(config.model_id, "Qwen/Qwen2.5-1.5B-Instruct-GGUF");
        assert_eq!(config.model_filename, "qwen2.5-1.5b-instruct-q4_k_m.gguf");
        assert_eq!(config.tokenizer_repo, "Qwen/Qwen2.5-1.5B-Instruct");
    }

    #[test]
    fn test_seed_model_in_config_toml() {
        let toml_content = r#"
            [gateway]
            bind_host = "127.0.0.1"
            port = 8080

            [agents.defaults]
            workspace = "/tmp/workspace"
            model = "test-model"

            [[agents.list]]
            id = "main"

            [tools]
            profile = "standard"

            [security.sandbox]
            mode = "tools"

            [seed_model]
            model_id = "custom/model-GGUF"
            model_filename = "custom.gguf"
            tokenizer_repo = "custom/model"
        "#;
        let config = Config::from_toml(toml_content).unwrap();
        assert_eq!(config.seed_model.model_id, "custom/model-GGUF");
        assert_eq!(config.seed_model.model_filename, "custom.gguf");
        assert_eq!(config.seed_model.tokenizer_repo, "custom/model");
    }
}
