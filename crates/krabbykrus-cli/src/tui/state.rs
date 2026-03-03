//! Shared application state and message types
//!
//! This module defines the centralized state that all components can read,
//! and the message types used for async updates.

use std::path::PathBuf;
use tokio::sync::mpsc;

/// Messages for async state updates
#[derive(Debug, Clone)]
pub enum Message {
    // Navigation
    Navigate(MenuItem),
    ToggleSidebar,
    
    // Gateway status
    GatewayStatus(GatewayStatus),
    GatewayStatusError(String),
    
    // Agents
    AgentsLoaded(Vec<AgentInfo>),
    AgentsError(String),
    ReloadAgents,
    
    // Sessions
    SessionsLoaded(Vec<SessionInfo>),
    SessionsError(String),
    
    // Vault/Credentials
    VaultStatus(VaultStatus),
    VaultUnlocked,
    VaultLocked,
    VaultError(String),
    EndpointsLoaded(Vec<EndpointInfo>),
    
    // Models
    ModelsLoaded(Vec<ModelProvider>),
    
    // UI feedback
    SetStatus(String, bool), // (message, is_error)
    ClearStatus,
    
    // Tick for animations/refresh
    Tick,
    
    // Exit
    Quit,
}

/// Main menu items - unified between TUI and Web UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuItem {
    #[default]
    Dashboard,
    Credentials,
    Agents,
    Sessions,
    Models,
    Settings,
}

impl MenuItem {
    pub fn all() -> Vec<Self> {
        vec![
            Self::Dashboard,
            Self::Credentials,
            Self::Agents,
            Self::Sessions,
            Self::Models,
            Self::Settings,
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Credentials => "Credentials",
            Self::Agents => "Agents",
            Self::Sessions => "Sessions",
            Self::Models => "Models",
            Self::Settings => "Settings",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Dashboard => "📊",
            Self::Credentials => "🔐",
            Self::Agents => "🤖",
            Self::Sessions => "💬",
            Self::Models => "🧠",
            Self::Settings => "⚙️",
        }
    }
    
    pub fn index(&self) -> usize {
        match self {
            Self::Dashboard => 0,
            Self::Credentials => 1,
            Self::Agents => 2,
            Self::Sessions => 3,
            Self::Models => 4,
            Self::Settings => 5,
        }
    }
    
    pub fn from_index(idx: usize) -> Self {
        match idx % 6 {
            0 => Self::Dashboard,
            1 => Self::Credentials,
            2 => Self::Agents,
            3 => Self::Sessions,
            4 => Self::Models,
            _ => Self::Settings,
        }
    }
}

/// Gateway connection status
#[derive(Debug, Clone, Default)]
pub struct GatewayStatus {
    pub connected: bool,
    pub version: Option<String>,
    pub uptime_secs: Option<u64>,
    pub active_sessions: usize,
    pub pending_agents: usize,
}

/// Agent information
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub model: Option<String>,
    pub status: AgentStatus,
    pub session_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Active,
    Pending,
    Error,
    Disabled,
}

impl AgentStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Pending => "Pending",
            Self::Error => "Error",
            Self::Disabled => "Disabled",
        }
    }
}

/// Session information
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub key: String,
    pub agent_id: String,
    pub channel: Option<String>,
    pub started_at: Option<String>,
    pub message_count: usize,
}

/// Vault unlock method
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UnlockMethod {
    #[default]
    Unknown,
    Password,
    Keyfile { path: Option<String> },
    Age { public_key: Option<String> },
    SshKey { path: Option<String> },
}

impl UnlockMethod {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Password => "Password",
            Self::Keyfile { .. } => "Keyfile",
            Self::Age { .. } => "Age",
            Self::SshKey { .. } => "SSH Key",
        }
    }
    
    pub fn requires_input(&self) -> bool {
        matches!(self, Self::Password | Self::Age { .. })
    }
}

/// Vault status
#[derive(Debug, Clone, Default)]
pub struct VaultStatus {
    pub enabled: bool,
    pub initialized: bool,
    pub locked: bool,
    pub endpoint_count: usize,
    pub unlock_method: UnlockMethod,
}

/// Endpoint information (matches credentials module)
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    pub id: String,
    pub name: String,
    pub endpoint_type: String,
    pub base_url: String,
    pub has_credential: bool,
    pub expiration: Option<String>,
}

/// Model provider information
#[derive(Debug, Clone)]
pub struct ModelProvider {
    pub name: String,
    pub provider_type: String,
    pub configured: bool,
    pub models: Vec<String>,
    pub base_url: Option<String>,
}

/// Centralized application state
pub struct AppState {
    // Navigation
    pub menu_item: MenuItem,
    pub menu_index: usize,
    pub sidebar_focus: bool,
    
    // Paths
    pub config_path: PathBuf,
    pub vault_path: PathBuf,
    
    // Gateway
    pub gateway: GatewayStatus,
    pub gateway_loading: bool,
    pub gateway_error: Option<String>,
    
    // Agents
    pub agents: Vec<AgentInfo>,
    pub agents_loading: bool,
    pub agents_error: Option<String>,
    pub selected_agent: usize,
    
    // Sessions  
    pub sessions: Vec<SessionInfo>,
    pub sessions_loading: bool,
    pub sessions_error: Option<String>,
    pub selected_session: usize,
    
    // Vault/Credentials
    pub vault: VaultStatus,
    pub vault_loading: bool,
    pub endpoints: Vec<EndpointInfo>,
    pub selected_endpoint: usize,
    
    // Models
    pub providers: Vec<ModelProvider>,
    pub selected_provider: usize,
    
    // UI state
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub should_exit: bool,
    pub tick_count: usize,
    
    // Input modes (for modals, text input, etc.)
    pub input_mode: InputMode,
    pub input_buffer: String,
    
    // Message sender for async updates
    pub tx: mpsc::UnboundedSender<Message>,
}

/// Input modes for capturing text/modal interactions
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    /// Password input for vault
    PasswordInput { prompt: String, masked: bool, action: PasswordAction },
    /// Add credential modal
    AddCredential(AddCredentialState),
    /// Confirmation dialog
    Confirm { message: String, action: ConfirmAction },
    /// Chat input
    ChatInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordAction {
    InitVault,
    UnlockVault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteEndpoint(String), // endpoint id
    DeleteAgent(String),    // agent id
}

/// Add credential form state
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddCredentialState {
    pub field: AddCredentialField,
    pub name: String,
    pub endpoint_type: usize,
    pub url: String,
    pub secret: String,
    pub expiration: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddCredentialField {
    #[default]
    Name,
    EndpointType,
    Url,
    Secret,
    Expiration,
}

impl AddCredentialField {
    pub fn next(&self) -> Self {
        match self {
            Self::Name => Self::EndpointType,
            Self::EndpointType => Self::Url,
            Self::Url => Self::Secret,
            Self::Secret => Self::Expiration,
            Self::Expiration => Self::Name,
        }
    }
    
    pub fn prev(&self) -> Self {
        match self {
            Self::Name => Self::Expiration,
            Self::EndpointType => Self::Name,
            Self::Url => Self::EndpointType,
            Self::Secret => Self::Url,
            Self::Expiration => Self::Secret,
        }
    }
}

impl AppState {
    pub fn new(
        config_path: PathBuf,
        vault_path: PathBuf,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            menu_item: MenuItem::Dashboard,
            menu_index: 0,
            sidebar_focus: true,
            
            config_path,
            vault_path,
            
            gateway: GatewayStatus::default(),
            gateway_loading: true,
            gateway_error: None,
            
            agents: Vec::new(),
            agents_loading: true,
            agents_error: None,
            selected_agent: 0,
            
            sessions: Vec::new(),
            sessions_loading: false,
            sessions_error: None,
            selected_session: 0,
            
            vault: VaultStatus::default(),
            vault_loading: true,
            endpoints: Vec::new(),
            selected_endpoint: 0,
            
            providers: Vec::new(),
            selected_provider: 0,
            
            status_message: None,
            should_exit: false,
            tick_count: 0,
            
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            
            tx,
        }
    }
    
    /// Process a message and update state
    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Navigate(item) => {
                self.menu_item = item;
                self.menu_index = item.index();
                self.sidebar_focus = false;
            }
            Message::ToggleSidebar => {
                self.sidebar_focus = !self.sidebar_focus;
            }
            
            Message::GatewayStatus(status) => {
                self.gateway = status;
                self.gateway_loading = false;
                self.gateway_error = None;
            }
            Message::GatewayStatusError(err) => {
                self.gateway_loading = false;
                self.gateway_error = Some(err);
            }
            
            Message::AgentsLoaded(agents) => {
                self.agents = agents;
                self.agents_loading = false;
                self.agents_error = None;
            }
            Message::AgentsError(err) => {
                self.agents_loading = false;
                self.agents_error = Some(err);
            }
            Message::ReloadAgents => {
                self.agents_loading = true;
            }
            
            Message::SessionsLoaded(sessions) => {
                self.sessions = sessions;
                self.sessions_loading = false;
                self.sessions_error = None;
            }
            Message::SessionsError(err) => {
                self.sessions_loading = false;
                self.sessions_error = Some(err);
            }
            
            Message::VaultStatus(status) => {
                self.vault = status;
                self.vault_loading = false;
            }
            Message::VaultUnlocked => {
                self.vault.locked = false;
            }
            Message::VaultLocked => {
                self.vault.locked = true;
            }
            Message::VaultError(err) => {
                self.vault_loading = false;
                self.status_message = Some((err, true));
            }
            Message::EndpointsLoaded(endpoints) => {
                self.endpoints = endpoints;
            }
            
            Message::ModelsLoaded(providers) => {
                self.providers = providers;
            }
            
            Message::SetStatus(msg, is_error) => {
                self.status_message = Some((msg, is_error));
            }
            Message::ClearStatus => {
                self.status_message = None;
            }
            
            Message::Tick => {
                self.tick_count = self.tick_count.wrapping_add(1);
            }
            
            Message::Quit => {
                self.should_exit = true;
            }
        }
    }
    
    /// Check if we're in an input mode that should capture all keys
    pub fn is_capturing_input(&self) -> bool {
        !matches!(self.input_mode, InputMode::Normal)
    }
    
    /// Move selection up in current list
    pub fn select_prev(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {}
            MenuItem::Credentials => {
                if !self.endpoints.is_empty() {
                    self.selected_endpoint = if self.selected_endpoint == 0 {
                        self.endpoints.len() - 1
                    } else {
                        self.selected_endpoint - 1
                    };
                }
            }
            MenuItem::Agents => {
                if !self.agents.is_empty() {
                    self.selected_agent = if self.selected_agent == 0 {
                        self.agents.len() - 1
                    } else {
                        self.selected_agent - 1
                    };
                }
            }
            MenuItem::Sessions => {
                if !self.sessions.is_empty() {
                    self.selected_session = if self.selected_session == 0 {
                        self.sessions.len() - 1
                    } else {
                        self.selected_session - 1
                    };
                }
            }
            MenuItem::Models => {
                if !self.providers.is_empty() {
                    self.selected_provider = if self.selected_provider == 0 {
                        self.providers.len() - 1
                    } else {
                        self.selected_provider - 1
                    };
                }
            }
            MenuItem::Settings => {}
        }
    }
    
    /// Move selection down in current list
    pub fn select_next(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {}
            MenuItem::Credentials => {
                if !self.endpoints.is_empty() {
                    self.selected_endpoint = (self.selected_endpoint + 1) % self.endpoints.len();
                }
            }
            MenuItem::Agents => {
                if !self.agents.is_empty() {
                    self.selected_agent = (self.selected_agent + 1) % self.agents.len();
                }
            }
            MenuItem::Sessions => {
                if !self.sessions.is_empty() {
                    self.selected_session = (self.selected_session + 1) % self.sessions.len();
                }
            }
            MenuItem::Models => {
                if !self.providers.is_empty() {
                    self.selected_provider = (self.selected_provider + 1) % self.providers.len();
                }
            }
            MenuItem::Settings => {}
        }
    }
    
    /// Navigate to previous menu item
    pub fn menu_prev(&mut self) {
        self.menu_index = if self.menu_index == 0 { 5 } else { self.menu_index - 1 };
        self.menu_item = MenuItem::from_index(self.menu_index);
    }
    
    /// Navigate to next menu item
    pub fn menu_next(&mut self) {
        self.menu_index = (self.menu_index + 1) % 6;
        self.menu_item = MenuItem::from_index(self.menu_index);
    }
}
