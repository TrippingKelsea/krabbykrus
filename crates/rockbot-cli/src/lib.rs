//! Command-line interface for RockBot

use anyhow::Result;
use clap::{Parser, Subcommand};
use rockbot_core::Config;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

pub mod commands;
pub mod tui;

/// RockBot: Next-generation AI agent framework
#[derive(Parser)]
#[command(name = "rockbot")]
#[command(about = "Next-generation AI agent framework")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    /// Configuration file path
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Verbose output
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Subcommand
    #[command(subcommand)]
    pub command: Commands,
}

/// Available CLI commands
#[derive(Subcommand)]
pub enum Commands {
    /// Gateway server management
    Gateway {
        #[command(subcommand)]
        command: GatewayCommands,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },

    /// Session management
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },

    /// Agent management
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },

    /// Tool management
    Tool {
        #[command(subcommand)]
        command: ToolCommands,
    },

    /// Credential management
    Credentials {
        #[command(subcommand)]
        command: CredentialsCommands,
    },

    /// TLS certificate management
    Cert {
        #[command(subcommand)]
        command: CertCommands,
    },

    /// Health and diagnostics
    Doctor {
        #[command(subcommand)]
        command: Option<DoctorCommands>,
    },

    /// Interactive TUI dashboard
    Tui {
        /// Gateway address (e.g. 172.30.200.146:18181, https://host:port)
        #[arg(short, long, default_value = "127.0.0.1:18080")]
        gateway: String,
    },

    /// Migration from OpenClaw
    Migrate {
        #[command(subcommand)]
        command: MigrateCommands,
    },
}

/// Gateway server commands
#[derive(Subcommand)]
pub enum GatewayCommands {
    /// Run the gateway server (foreground)
    Run,
    /// Start the gateway service (if installed)
    Start,
    /// Stop the gateway service
    Stop,
    /// Restart the gateway service
    Restart,
    /// Show gateway service status
    Status,
    /// Install gateway as a system service
    Install {
        /// Install as system service (requires root) vs user service
        #[arg(long)]
        system: bool,
        /// Custom service name
        #[arg(long, default_value = "rockbot-gateway")]
        name: String,
    },
    /// Remove gateway service
    Remove {
        /// Remove system service (requires root) vs user service
        #[arg(long)]
        system: bool,
        /// Custom service name
        #[arg(long, default_value = "rockbot-gateway")]
        name: String,
    },
    /// Show service logs
    Logs {
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
}

/// Configuration commands
#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Validate configuration
    Validate,
    /// Generate default configuration
    Init {
        /// Output file path
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Overwrite existing file
        #[arg(short, long)]
        force: bool,
    },
}

/// Certificate management commands
#[derive(Subcommand)]
pub enum CertCommands {
    /// Certificate Authority management
    Ca {
        #[command(subcommand)]
        command: CaCertCommands,
    },
    /// Client certificate management (gateway, agent, TUI instances)
    Client {
        #[command(subcommand)]
        command: ClientCertCommands,
    },
    /// Sign a CSR with the CA (offline signing)
    Sign {
        /// Path to PEM-encoded CSR
        #[arg(long)]
        csr: PathBuf,
        /// Client name for the signed certificate
        #[arg(long)]
        name: String,
        /// Role: gateway, agent, or tui
        #[arg(long, default_value = "agent")]
        role: String,
        /// Certificate validity in days
        #[arg(long, default_value = "365")]
        days: u32,
        /// Output path for signed certificate
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Install certificates into the app config (updates rockbot.toml)
    Install {
        /// Client name whose cert to install
        #[arg(long)]
        name: String,
    },
    /// Verify certificate chain, key match, and revocation status
    Verify {
        /// Path to PEM certificate file (defaults to config value)
        #[arg(long)]
        cert: Option<PathBuf>,
        /// Path to PEM private key file (defaults to config value)
        #[arg(long)]
        key: Option<PathBuf>,
        /// Path to CA certificate for chain verification
        #[arg(long)]
        ca: Option<PathBuf>,
    },
    /// Show details of any PEM certificate file
    Info {
        /// Path to PEM certificate file
        cert: PathBuf,
    },
    /// Manage enrollment tokens for remote CSR signing
    Enroll {
        #[command(subcommand)]
        command: EnrollCommands,
    },
}

/// CA certificate commands
#[derive(Subcommand)]
pub enum CaCertCommands {
    /// Initialize a new Certificate Authority
    Generate {
        /// CA certificate validity in days
        #[arg(long, default_value = "3650")]
        days: u32,
        /// PKI directory (default: ~/.config/rockbot/pki/)
        #[arg(long)]
        pki_dir: Option<PathBuf>,
        /// Overwrite existing CA
        #[arg(short, long)]
        force: bool,
    },
    /// Show CA certificate details
    Info,
    /// Rotate the CA (generates new CA, re-signs active client certs)
    Rotate {
        /// New CA validity in days
        #[arg(long, default_value = "3650")]
        days: u32,
        /// Back up old CA files
        #[arg(long)]
        backup: bool,
    },
    /// Publish CA cert to S3 and register DNS records
    Publish,
}

/// Client certificate commands
#[derive(Subcommand)]
pub enum ClientCertCommands {
    /// Generate a new client certificate signed by the CA
    Generate {
        /// Client name (becomes CN and filename)
        name: String,
        /// Additional Subject Alternative Names (hostnames or IPs)
        #[arg(long)]
        san: Vec<String>,
        /// Certificate validity in days
        #[arg(long, default_value = "365")]
        days: u32,
        /// Role: gateway, agent, or tui
        #[arg(long, default_value = "agent")]
        role: String,
    },
    /// List all issued client certificates
    List,
    /// Show details of a specific client certificate
    Info {
        /// Client name
        name: String,
    },
    /// Revoke a client certificate
    Revoke {
        /// Client name
        name: String,
    },
    /// Rotate a client certificate (revoke old, generate new)
    Rotate {
        /// Client name
        name: String,
        /// Additional Subject Alternative Names
        #[arg(long)]
        san: Vec<String>,
        /// Certificate validity in days
        #[arg(long, default_value = "365")]
        days: u32,
        /// Back up old certificate files
        #[arg(long)]
        backup: bool,
    },
}

/// Enrollment token commands
#[derive(Subcommand)]
pub enum EnrollCommands {
    /// Create a new enrollment token for remote CSR signing
    Create {
        /// Role for certificates signed with this token
        #[arg(long, default_value = "agent")]
        role: String,
        /// Maximum number of uses (omit for unlimited)
        #[arg(long)]
        uses: Option<u32>,
        /// Token expiry duration (e.g. "1h", "24h", "7d")
        #[arg(long)]
        expires: Option<String>,
    },
    /// List active enrollment tokens
    List,
    /// Revoke an enrollment token
    Revoke {
        /// Token ID
        id: String,
    },
    /// Enroll this client with a remote gateway using a PSK
    Submit {
        /// Gateway address (e.g. https://host:port)
        #[arg(long)]
        gateway: String,
        /// Pre-shared enrollment key
        #[arg(long)]
        psk: String,
        /// Client name
        #[arg(long)]
        name: String,
        /// Role: gateway, agent, or tui
        #[arg(long, default_value = "tui")]
        role: String,
    },
}

/// Session commands
#[derive(Subcommand)]
pub enum SessionCommands {
    /// List sessions
    List {
        /// Filter by agent ID
        #[arg(short, long)]
        agent: Option<String>,
        /// Show only active sessions
        #[arg(short, long)]
        active: bool,
    },
    /// Show session details
    Show {
        /// Session ID
        session_id: String,
    },
    /// Show session message history
    History {
        /// Session ID
        session_id: String,
        /// Number of messages to show
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Archive a session
    Archive {
        /// Session ID
        session_id: String,
    },
    /// Delete a session
    Delete {
        /// Session ID
        session_id: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
}

/// Agent commands
#[derive(Subcommand)]
pub enum AgentCommands {
    /// List configured agents
    List,
    /// Show agent status
    Status {
        /// Agent ID
        agent_id: String,
    },
    /// Send message to agent
    Message {
        /// Agent ID
        agent_id: String,
        /// Session key
        #[arg(short, long, default_value = "cli")]
        session: String,
        /// Message text
        message: String,
    },
    /// Create new agent
    Create {
        /// Agent ID
        agent_id: String,
        /// Workspace directory
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        /// Model to use
        #[arg(short, long)]
        model: Option<String>,
    },
    /// Run an interactive agent session via a remote gateway
    Run {
        /// Agent ID to interact with
        agent_id: String,
        /// Gateway address (e.g. 172.30.200.146:18181, https://host:port)
        #[arg(short, long, default_value = "127.0.0.1:18080")]
        gateway: String,
        /// Register as remote tool executor
        #[arg(long)]
        exec: bool,
    },
}

/// Tool commands
#[derive(Subcommand)]
pub enum ToolCommands {
    /// List available tools
    List,
    /// Show tool details
    Info {
        /// Tool name
        tool_name: String,
    },
    /// Test a tool
    Test {
        /// Tool name
        tool_name: String,
        /// Tool parameters (JSON)
        #[arg(short, long)]
        params: Option<String>,
    },
}

/// Credential management commands
#[derive(Subcommand)]
pub enum CredentialsCommands {
    /// Initialize the credential vault (first-time setup)
    Init {
        /// Force re-initialization (destroys existing vault)
        #[arg(short, long)]
        force: bool,
        /// Use password-based encryption instead of keyfile
        #[arg(short, long)]
        password: bool,
        /// Use a specific key file (default: ~/.config/rockbot/vault.key)
        #[arg(short, long, value_name = "PATH")]
        keyfile: Option<PathBuf>,
        /// Use an Age public key for encryption
        #[arg(long, value_name = "AGE_PUBKEY")]
        age: Option<String>,
        /// Use an SSH public key for encryption
        #[arg(long, value_name = "PATH")]
        ssh_key: Option<PathBuf>,
    },
    /// Add a new credential endpoint
    Add {
        /// Endpoint name
        name: String,
        /// Endpoint type (home_assistant, gmail, spotify, generic_rest, generic_oauth2)
        #[arg(short = 't', long)]
        endpoint_type: String,
        /// Base URL for the endpoint
        #[arg(short, long)]
        url: String,
        /// Secret value (will be encrypted)
        #[arg(short, long)]
        secret: Option<String>,
        /// Credential type (bearer_token, basic_auth, api_key)
        #[arg(short, long, default_value = "bearer_token")]
        credential_type: String,
    },
    /// List configured endpoints (not secrets)
    List,
    /// Remove an endpoint and its credential
    Remove {
        /// Endpoint name or ID
        endpoint: String,
    },
    /// Manage permission rules
    Permissions {
        #[command(subcommand)]
        command: PermissionsCommands,
    },
    /// View or verify the audit log
    Audit {
        /// Verify audit log integrity
        #[arg(short, long)]
        verify: bool,
        /// Number of entries to show
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show vault status
    Status,
    /// Unlock the vault
    Unlock {
        /// Password (if not provided, will prompt for password-based vaults)
        #[arg(short, long)]
        password: Option<String>,
        /// Key file path (for keyfile-based vaults)
        #[arg(short, long, value_name = "PATH")]
        keyfile: Option<PathBuf>,
        /// Age identity/private key (AGE-SECRET-KEY-...)
        #[arg(long, value_name = "AGE_IDENTITY")]
        age: Option<String>,
        /// SSH private key path (for SSH-based vaults)
        #[arg(long, value_name = "PATH")]
        ssh_key: Option<PathBuf>,
        /// Passphrase for encrypted SSH key
        #[arg(long)]
        ssh_passphrase: Option<String>,
    },
    /// Lock the vault
    Lock,
    /// Interactive TUI for credential management
    Ui,
}

/// Permission management commands
#[derive(Subcommand)]
pub enum PermissionsCommands {
    /// Add a permission rule
    Add {
        /// Path pattern (glob-style: * and ** supported)
        pattern: String,
        /// Permission level (allow, allow_hil, allow_hil_2fa, deny)
        #[arg(short, long)]
        level: String,
        /// Description
        #[arg(short, long)]
        description: Option<String>,
    },
    /// List permission rules
    List,
    /// Remove a permission rule
    Remove {
        /// Rule ID
        rule_id: String,
    },
}

/// Doctor subcommands (AI-powered when `doctor-ai` feature is enabled)
#[derive(Subcommand)]
pub enum DoctorCommands {
    /// Run full health check (default when no subcommand given)
    Check,
    /// Diagnose a specific config error with AI explanation
    #[cfg(feature = "doctor-ai")]
    Diagnose {
        /// Path to config file (defaults to standard location)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Check config for deprecated/renamed fields across versions
    #[cfg(feature = "doctor-ai")]
    Migrate {
        /// Path to config file (defaults to standard location)
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Download/update the doctor AI model
    #[cfg(feature = "doctor-ai")]
    Download,
    /// Show doctor AI status (model loaded, version, etc.)
    #[cfg(feature = "doctor-ai")]
    Status,
    /// Launch interactive Doctor AI chat TUI
    #[cfg(feature = "doctor-ai")]
    Tui,
}

/// Migration commands
#[derive(Subcommand)]
pub enum MigrateCommands {
    /// Migrate configuration from OpenClaw
    Config {
        /// OpenClaw config file
        #[arg(short, long)]
        from: PathBuf,
        /// Output RockBot config file
        #[arg(short, long)]
        to: PathBuf,
    },
    /// Migrate sessions from OpenClaw
    Sessions {
        /// OpenClaw agents directory
        #[arg(short, long)]
        from: PathBuf,
        /// RockBot agents directory
        #[arg(short, long)]
        to: PathBuf,
    },
    /// Verify migration completeness
    Verify {
        /// OpenClaw config file
        openclaw_config: PathBuf,
        /// RockBot config file
        rockbot_config: Option<PathBuf>,
    },
}

/// Main CLI entry point
pub async fn run(cli: Cli) -> Result<()> {
    // Install the rustls crypto provider once, before any TLS operations
    // (gateway, TUI https requests, cert commands, etc.)
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Initialize logging based on verbosity and command
    let log_level = match cli.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    let is_tui = matches!(cli.command, Commands::Tui { .. }) || {
        #[cfg(feature = "doctor-ai")]
        {
            matches!(
                cli.command,
                Commands::Doctor {
                    command: Some(DoctorCommands::Tui)
                }
            )
        }
        #[cfg(not(feature = "doctor-ai"))]
        {
            false
        }
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("rockbot={log_level}")));

    if is_tui {
        // TUI mode: write logs to a file so they don't corrupt the terminal
        let log_dir = dirs::state_dir()
            .or_else(dirs::cache_dir)
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"))
            .join("rockbot")
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);

        // Clean up log files older than 7 days
        sweep_old_logs(&log_dir, 7);

        let log_file = log_dir.join(format!(
            "tui-{}.log",
            chrono::Local::now().format("%Y-%m-%d")
        ));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)?;

        // Print log path to stderr before the TUI takes over the terminal
        eprintln!("TUI logs: {}", log_file.display());

        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    } else {
        // CLI mode: write logs to stderr as usual
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    // Load configuration
    let config_path = cli.config.unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
            .join("rockbot")
            .join("rockbot.toml")
    });

    match &cli.command {
        Commands::Gateway { command } => commands::gateway::run(command, &config_path).await,
        Commands::Config { command } => commands::config::run(command, &config_path).await,
        Commands::Session { command } => commands::session::run(command, &config_path).await,
        Commands::Agent { command } => commands::agent::run(command, &config_path).await,
        Commands::Tool { command } => commands::tool::run(command, &config_path).await,
        Commands::Credentials { command } => {
            commands::credentials::run(command, &config_path).await
        }
        Commands::Cert { command } => commands::cert::run(command, &config_path).await,
        Commands::Doctor { command } => commands::doctor::run(command, &config_path).await,
        Commands::Tui { gateway } => {
            // Load config to get vault path
            let config = load_config(&config_path).await?;
            let gateway_url = rockbot_client::normalize_gateway_url(gateway);
            tui::run_app(
                config_path.clone(),
                config.credentials.vault_path,
                gateway_url,
            )
            .await
        }
        Commands::Migrate { command } => commands::migrate::run(command).await,
    }
}

/// Get default configuration path
pub fn get_default_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
        .join("rockbot")
        .join("rockbot.toml")
}

/// Load configuration from file.
///
/// When the `doctor-ai` feature is enabled and config parsing fails, the doctor
/// AI will attempt to diagnose the error and print a helpful explanation before
/// returning the error.
pub async fn load_config(path: &PathBuf) -> Result<Config> {
    if !path.exists() {
        anyhow::bail!("Configuration file not found: {}", path.display());
    }

    match Config::from_file(path).await {
        Ok(config) => {
            info!("Loaded configuration from {}", path.display());
            Ok(config)
        }
        Err(e) => {
            #[cfg(feature = "doctor-ai")]
            {
                doctor_intercept(path, &e).await;
            }
            Err(e.into())
        }
    }
}

/// When config loading fails, attempt to diagnose with the doctor AI.
#[cfg(feature = "doctor-ai")]
async fn doctor_intercept(path: &PathBuf, error: &dyn std::fmt::Display) {
    use rockbot_doctor::DoctorAi;
    use tracing::warn;

    let raw_toml = match tokio::fs::read_to_string(path).await {
        Ok(s) => s,
        Err(_) => return,
    };

    let doctor_config = commands::doctor::try_parse_doctor_config_from_raw(&raw_toml);

    match DoctorAi::init(doctor_config).await {
        Ok(mut doctor) => {
            let error_str = error.to_string();
            let diagnosis = doctor.diagnose_parse_error(&raw_toml, &error_str).await;

            eprintln!("\n--- Doctor AI Diagnosis ---");
            eprintln!("  Error Type: {}", diagnosis.kind.label());
            if let Some(ref field) = diagnosis.field_path {
                eprintln!("  Field:      {field}");
            }
            if let Some(line) = diagnosis.line {
                eprintln!("  Line:       {line}");
            }
            if !diagnosis.explanation.is_empty() {
                eprintln!("  Explanation: {}", diagnosis.explanation);
            }

            if let Some(fix) = doctor.suggest_fix(&raw_toml, &diagnosis).await {
                eprintln!("  Suggested:   {}", fix.describe());

                if doctor.auto_fix_enabled() {
                    eprintln!("  Applying auto-fix...");
                    match rockbot_doctor::repair::apply_fix(&raw_toml, &fix) {
                        Ok(patched) => match patched.parse::<toml::Value>() {
                            Ok(_) => {
                                if tokio::fs::write(path, &patched).await.is_ok() {
                                    eprintln!("  Config repaired and verified. Please restart.");
                                    doctor.record_successful_fix(&diagnosis, &fix);
                                }
                            }
                            Err(verify_err) => {
                                eprintln!("  Fix verification failed: {verify_err}");
                                eprintln!("  Original config preserved.");
                            }
                        },
                        Err(e) => {
                            eprintln!("  Auto-fix failed: {e}");
                        }
                    }
                } else {
                    eprintln!("  Tip: set [doctor] auto_fix = true to auto-repair on failure");
                }
            }
            eprintln!("---------------------------\n");
        }
        Err(e) => {
            warn!("Doctor AI could not initialize: {e}");
        }
    }
}

/// Remove log files older than `max_age_days` from the given directory.
fn sweep_old_logs(log_dir: &std::path::Path, max_age_days: u64) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(max_age_days * 86400);

    let entries = match std::fs::read_dir(log_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        if let Ok(meta) = path.metadata() {
            let modified = meta.modified().unwrap_or(std::time::SystemTime::now());
            if modified < cutoff {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}
