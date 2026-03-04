//! Shared application state and message types for the TUI.
//!
//! This module defines the centralized state that all TUI components can read,
//! and the message types used for async state updates.
//!
//! # Architecture
//!
//! The TUI follows an Elm-like architecture:
//!
//! 1. **State** ([`AppState`]) - Single source of truth for all UI data
//! 2. **Messages** ([`Message`]) - Events that trigger state changes
//! 3. **Update** ([`AppState::update`]) - Pure function that applies messages to state
//! 4. **View** - Components that render state (in `components/` module)
//!
//! # Example
//!
//! ```no_run
//! use krabbykrus_cli::tui::state::{AppState, Message};
//! use tokio::sync::mpsc;
//!
//! let (tx, mut rx) = mpsc::unbounded_channel();
//! let state = AppState::new(config_path, vault_path, tx.clone());
//!
//! // Send a navigation message
//! tx.send(Message::Navigate(MenuItem::Credentials)).unwrap();
//!
//! // Process messages
//! while let Ok(msg) = rx.try_recv() {
//!     state.update(msg);
//! }
//! ```

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
    pub selected_category: usize,  // For Providers tab navigation
    pub credentials_tab: usize,    // Which tab is active (0=Endpoints, 1=Providers, etc.)
    
    // Models (5 known providers: Anthropic, OpenAI, Ollama, Bedrock, Google)
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

/// Definition of a form field for dynamic credential forms.
///
/// Different endpoint types require different fields. This struct defines
/// the metadata for each field so the UI can render appropriate inputs.
///
/// # Example
///
/// ```
/// use krabbykrus_cli::tui::state::FieldDef;
///
/// let field = FieldDef {
///     id: "api_key",
///     label: "API Key",
///     placeholder: "sk-...",
///     required: true,
///     masked: true,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    /// Unique identifier for this field (used for form data extraction).
    pub id: &'static str,
    /// Human-readable label displayed next to the input.
    pub label: &'static str,
    /// Placeholder text shown when the field is empty.
    pub placeholder: &'static str,
    /// Whether this field must be filled before submission.
    pub required: bool,
    /// Whether to mask input (for passwords/secrets).
    pub masked: bool,
}

/// Returns the form fields required for a given endpoint type.
///
/// Each endpoint type has different authentication requirements:
///
/// | Index | Type | Fields |
/// |-------|------|--------|
/// | 0 | Home Assistant | URL, Long-Lived Token |
/// | 1 | Generic REST | Base URL, Bearer Token |
/// | 2 | OAuth2 | Base URL, Auth URL, Token URL, Client ID/Secret, Scopes, Redirect URI |
/// | 3 | API Key | Base URL, API Key, Header Name |
/// | 4 | Basic Auth | Base URL, Username, Password |
/// | 5 | Bearer Token | Base URL, Token |
///
/// # Arguments
///
/// * `endpoint_type` - Index of the endpoint type (0-5)
///
/// # Returns
///
/// Vector of [`FieldDef`] describing the required form fields.
pub fn get_fields_for_endpoint_type(endpoint_type: usize) -> Vec<FieldDef> {
    match endpoint_type {
        0 => vec![ // Home Assistant
            FieldDef { id: "url", label: "Home Assistant URL", placeholder: "http://homeassistant.local:8123", required: true, masked: false },
            FieldDef { id: "token", label: "Long-Lived Access Token", placeholder: "eyJ0eXAi...", required: true, masked: true },
        ],
        1 => vec![ // Generic REST API
            FieldDef { id: "url", label: "Base URL", placeholder: "https://api.example.com", required: true, masked: false },
            FieldDef { id: "token", label: "Bearer Token", placeholder: "Your token", required: false, masked: true },
        ],
        2 => vec![ // OAuth2 Service
            FieldDef { id: "url", label: "API Base URL", placeholder: "https://api.example.com", required: true, masked: false },
            FieldDef { id: "auth_url", label: "Authorization URL", placeholder: "https://auth.example.com/authorize", required: true, masked: false },
            FieldDef { id: "token_url", label: "Token URL", placeholder: "https://auth.example.com/token", required: true, masked: false },
            FieldDef { id: "client_id", label: "Client ID", placeholder: "", required: true, masked: false },
            FieldDef { id: "client_secret", label: "Client Secret", placeholder: "", required: true, masked: true },
            FieldDef { id: "scopes", label: "Scopes", placeholder: "read write offline_access", required: false, masked: false },
            FieldDef { id: "redirect_uri", label: "Redirect URI", placeholder: "http://localhost:18080/oauth/callback", required: false, masked: false },
        ],
        3 => vec![ // API Key Service
            FieldDef { id: "url", label: "Base URL", placeholder: "https://api.example.com", required: true, masked: false },
            FieldDef { id: "api_key", label: "API Key", placeholder: "", required: true, masked: true },
            FieldDef { id: "header_name", label: "Header Name", placeholder: "X-API-Key", required: false, masked: false },
        ],
        4 => vec![ // Basic Auth Service
            FieldDef { id: "url", label: "Base URL", placeholder: "https://api.example.com", required: true, masked: false },
            FieldDef { id: "username", label: "Username", placeholder: "", required: true, masked: false },
            FieldDef { id: "password", label: "Password", placeholder: "", required: true, masked: true },
        ],
        5 => vec![ // Bearer Token
            FieldDef { id: "url", label: "Base URL", placeholder: "https://api.example.com", required: true, masked: false },
            FieldDef { id: "token", label: "Token", placeholder: "", required: true, masked: true },
        ],
        _ => vec![
            FieldDef { id: "url", label: "URL", placeholder: "", required: true, masked: false },
        ],
    }
}

/// State for the "Add Credential" modal with dynamic fields.
///
/// This state tracks form input for creating new credential endpoints.
/// Fields are dynamic based on the selected endpoint type—OAuth2 services
/// need more fields than simple bearer tokens.
///
/// # Field Indices
///
/// - `0` - Endpoint name (always present)
/// - `1` - Endpoint type selector (cycles through types)
/// - `2+` - Dynamic fields from [`get_fields_for_endpoint_type`]
///
/// # Example
///
/// ```
/// use krabbykrus_cli::tui::state::AddCredentialState;
///
/// let mut state = AddCredentialState::new();
///
/// // Navigate to type selector and change type
/// state.next_field(); // Now on type selector
/// state.endpoint_type = 2; // OAuth2
/// state.reset_fields_for_type(); // Updates field_values for OAuth2
///
/// // Fill in a field
/// state.next_field(); // First dynamic field
/// if let Some(value) = state.current_value_mut() {
///     value.push_str("https://api.example.com");
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddCredentialState {
    /// Current field index (0=name, 1=type, 2+=dynamic fields).
    pub field_index: usize,
    /// User-provided name for the endpoint.
    pub name: String,
    /// Selected endpoint type index (see [`get_fields_for_endpoint_type`]).
    pub endpoint_type: usize,
    /// Values for dynamic fields (parallel to fields from [`get_fields_for_endpoint_type`]).
    pub field_values: Vec<String>,
}

impl AddCredentialState {
    /// Create new state, initializing field values for the default endpoint type
    pub fn new() -> Self {
        let fields = get_fields_for_endpoint_type(0);
        Self {
            field_index: 0,
            name: String::new(),
            endpoint_type: 0,
            field_values: vec![String::new(); fields.len()],
        }
    }
    
    /// Reset field values when endpoint type changes
    pub fn reset_fields_for_type(&mut self) {
        let fields = get_fields_for_endpoint_type(self.endpoint_type);
        self.field_values = vec![String::new(); fields.len()];
        // Reset to endpoint_type selector when type changes
        self.field_index = 1;
    }
    
    /// Get total number of fields (name + endpoint_type + dynamic fields)
    pub fn total_fields(&self) -> usize {
        2 + get_fields_for_endpoint_type(self.endpoint_type).len()
    }
    
    /// Move to next field
    pub fn next_field(&mut self) {
        self.field_index = (self.field_index + 1) % self.total_fields();
    }
    
    /// Move to previous field
    pub fn prev_field(&mut self) {
        if self.field_index == 0 {
            self.field_index = self.total_fields() - 1;
        } else {
            self.field_index -= 1;
        }
    }
    
    /// Check if current field is the name field
    pub fn is_name_field(&self) -> bool {
        self.field_index == 0
    }
    
    /// Check if current field is the endpoint type selector
    pub fn is_type_field(&self) -> bool {
        self.field_index == 1
    }
    
    /// Get the current dynamic field index (if on a dynamic field)
    pub fn dynamic_field_index(&self) -> Option<usize> {
        if self.field_index >= 2 {
            Some(self.field_index - 2)
        } else {
            None
        }
    }
    
    /// Check if on last field (for submit)
    pub fn is_last_field(&self) -> bool {
        self.field_index == self.total_fields() - 1
    }
    
    /// Get current field value reference for editing
    pub fn current_value_mut(&mut self) -> Option<&mut String> {
        if self.field_index == 0 {
            Some(&mut self.name)
        } else if self.field_index >= 2 {
            let idx = self.field_index - 2;
            self.field_values.get_mut(idx)
        } else {
            None // Type selector doesn't have text input
        }
    }
    
    /// Validate required fields, returns error message if invalid
    pub fn validate(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Name is required".to_string());
        }
        
        let fields = get_fields_for_endpoint_type(self.endpoint_type);
        for (i, field) in fields.iter().enumerate() {
            if field.required && self.field_values.get(i).map(|v| v.trim().is_empty()).unwrap_or(true) {
                return Some(format!("{} is required", field.label));
            }
        }
        
        None
    }
    
    /// Get field value by id
    pub fn get_field_value(&self, id: &str) -> Option<&str> {
        let fields = get_fields_for_endpoint_type(self.endpoint_type);
        for (i, field) in fields.iter().enumerate() {
            if field.id == id {
                return self.field_values.get(i).map(|s| s.as_str());
            }
        }
        None
    }
}

// Keep the old enum for backwards compatibility but mark it deprecated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[deprecated(note = "Use AddCredentialState.field_index instead")]
pub enum AddCredentialField {
    #[default]
    Name,
    EndpointType,
    Url,
    Secret,
    Expiration,
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
            selected_category: 0,
            credentials_tab: 0,
            
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
    
    /// Number of known LLM providers (Anthropic, OpenAI, Ollama, Bedrock, Google)
    pub const MODEL_PROVIDER_COUNT: usize = 5;
    
    /// Number of credential categories (All, Model, Communication, Tools, OAuth2, Generic)
    pub const CREDENTIAL_CATEGORY_COUNT: usize = 6;
    
    /// Move selection up in current list
    pub fn select_prev(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {}
            MenuItem::Credentials => {
                // Navigate based on which tab is active
                if self.credentials_tab == 1 {
                    // Providers tab - navigate categories
                    self.selected_category = if self.selected_category == 0 {
                        Self::CREDENTIAL_CATEGORY_COUNT - 1
                    } else {
                        self.selected_category - 1
                    };
                } else if self.credentials_tab == 0 && !self.endpoints.is_empty() {
                    // Endpoints tab
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
                // Always allow navigation - we have 5 known providers
                self.selected_provider = if self.selected_provider == 0 {
                    Self::MODEL_PROVIDER_COUNT - 1
                } else {
                    self.selected_provider - 1
                };
            }
            MenuItem::Settings => {}
        }
    }
    
    /// Move selection down in current list
    pub fn select_next(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {}
            MenuItem::Credentials => {
                // Navigate based on which tab is active
                if self.credentials_tab == 1 {
                    // Providers tab - navigate categories
                    self.selected_category = (self.selected_category + 1) % Self::CREDENTIAL_CATEGORY_COUNT;
                } else if self.credentials_tab == 0 && !self.endpoints.is_empty() {
                    // Endpoints tab
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
                // Always allow navigation - we have 5 known providers
                self.selected_provider = (self.selected_provider + 1) % Self::MODEL_PROVIDER_COUNT;
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
