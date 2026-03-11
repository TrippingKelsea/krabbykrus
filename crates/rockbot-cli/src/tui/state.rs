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
//! ```ignore
//! use rockbot_cli::tui::state::{AppState, Message};
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
    AgentSaved(String),     // agent id
    AgentSaveError(String),
    
    // Sessions
    SessionsLoaded(Vec<SessionInfo>),
    SessionsError(String),
    
    // Vault/Credentials
    VaultStatus(VaultStatus),
    VaultUnlocked,
    VaultLocked,
    VaultError(String),
    EndpointsLoaded(Vec<EndpointInfo>),
    CredentialAdded(String),  // endpoint name
    CredentialAddError(String),
    
    // Models
    ModelsLoaded(Vec<ModelProvider>),

    // Credential schemas (from gateway)
    CredentialSchemasLoaded(Vec<CredentialSchemaInfo>),
    
    // Chat
    ChatResponse(String),       // AI response text
    ChatError(String),          // Chat error
    ChatStreamChunk(String),    // Streaming chunk (for future use)
    
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
    pub parent_id: Option<String>,
    pub system_prompt: Option<String>,
    pub workspace: Option<String>,
    pub max_tool_calls: Option<u32>,
    pub enabled: bool,
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

/// Chat message role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

/// A message in a chat session
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: Option<String>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
            timestamp: Some(chrono::Local::now().format("%H:%M:%S").to_string()),
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            timestamp: Some(chrono::Local::now().format("%H:%M:%S").to_string()),
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            role: ChatRole::System,
            content,
            timestamp: None,
        }
    }
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

/// Model provider information (populated from gateway API)
#[derive(Debug, Clone)]
pub struct ModelProvider {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub auth_type: String,
    pub models: Vec<ModelProviderModel>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
}

/// Model info within a provider
#[derive(Debug, Clone)]
pub struct ModelProviderModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
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
    
    // Chat state
    pub chat_messages: Vec<ChatMessage>,
    pub chat_loading: bool,
    pub chat_scroll: usize,  // Scroll position in chat view
    
    // Vault/Credentials
    pub vault: VaultStatus,
    pub vault_loading: bool,
    pub endpoints: Vec<EndpointInfo>,
    pub selected_endpoint: usize,
    pub selected_category: usize,      // For Providers tab - which category
    pub selected_provider_index: usize, // For Providers tab - which provider within category
    pub provider_list_focus: bool,     // true = right panel (provider list), false = left panel (categories)
    pub credentials_tab: usize,        // Which tab is active (0=Endpoints, 1=Providers, etc.)
    
    // Models (dynamically loaded from gateway)
    pub providers: Vec<ModelProvider>,
    pub selected_provider: usize,

    // Credential schemas (from gateway — drives Credentials->Providers forms)
    pub credential_schemas: Vec<CredentialSchemaInfo>,
    
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
    /// Edit credential modal (similar to add but pre-populated)
    EditCredential(EditCredentialState),
    /// Edit model provider modal
    EditProvider(EditProviderState),
    /// Add agent modal
    AddAgent(EditAgentState),
    /// Edit agent modal
    EditAgent(EditAgentState),
    /// Confirmation dialog
    Confirm { message: String, action: ConfirmAction },
    /// Chat input
    ChatInput,
    /// View session details
    ViewSession { session_key: String },
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
    KillSession(String),    // session key
    DisableAgent(String),   // agent id (different from delete - actually disables in config)
}

/// State for the "Edit Credential" modal.
/// Pre-populated with existing endpoint data for editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCredentialState {
    /// Endpoint UUID being edited
    pub endpoint_id: String,
    /// Current field index (0=name, 1+=dynamic fields, no type selector since we don't change type)
    pub field_index: usize,
    /// User-provided name for the endpoint
    pub name: String,
    /// Endpoint type index (read-only, for field definitions)
    pub endpoint_type: usize,
    /// Base URL (editable)
    pub base_url: String,
    /// Values for dynamic fields (parallel to fields from get_fields_for_endpoint_type)
    pub field_values: Vec<String>,
    /// Whether secret has been modified (to know if we need to rotate)
    pub secret_modified: bool,
    /// Original credential ID for rotation
    pub credential_id: Option<String>,
}

impl EditCredentialState {
    /// Create edit state from an existing endpoint
    pub fn from_endpoint(
        endpoint_id: &str,
        name: &str,
        endpoint_type: usize,
        base_url: &str,
        credential_id: Option<&str>,
    ) -> Self {
        let fields = get_fields_for_endpoint_type(endpoint_type);
        let mut field_values = vec![String::new(); fields.len()];
        
        // Pre-fill URL field
        if !field_values.is_empty() {
            field_values[0] = base_url.to_string();
        }
        
        Self {
            endpoint_id: endpoint_id.to_string(),
            field_index: 0,
            name: name.to_string(),
            endpoint_type,
            base_url: base_url.to_string(),
            field_values,
            secret_modified: false,
            credential_id: credential_id.map(String::from),
        }
    }
    
    /// Pre-fill secret fields with decrypted values
    pub fn set_secret(&mut self, field_id: &str, value: &str) {
        let fields = get_fields_for_endpoint_type(self.endpoint_type);
        for (i, field) in fields.iter().enumerate() {
            if field.id == field_id {
                if let Some(fv) = self.field_values.get_mut(i) {
                    *fv = value.to_string();
                }
                break;
            }
        }
    }
    
    /// Get total number of fields (name + dynamic fields, no type selector)
    pub fn total_fields(&self) -> usize {
        1 + get_fields_for_endpoint_type(self.endpoint_type).len()
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
    
    /// Get the current dynamic field index (if on a dynamic field)
    pub fn dynamic_field_index(&self) -> Option<usize> {
        if self.field_index >= 1 {
            Some(self.field_index - 1)
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
        } else if self.field_index >= 1 {
            let idx = self.field_index - 1;
            // Mark secret as modified if editing a masked field
            let fields = get_fields_for_endpoint_type(self.endpoint_type);
            if let Some(field) = fields.get(idx) {
                if field.masked {
                    self.secret_modified = true;
                }
            }
            self.field_values.get_mut(idx)
        } else {
            None
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

/// Authentication type for model providers (kept for backward compat with modals)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderAuthType {
    #[default]
    ApiKey,
    SessionKey,
    None,
    AwsCredentials,
}

impl ProviderAuthType {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ApiKey => "API Key",
            Self::SessionKey => "Session Key (Claude Code)",
            Self::None => "None",
            Self::AwsCredentials => "AWS Credentials",
        }
    }

    /// Map from schema auth method ID to ProviderAuthType
    pub fn from_auth_method_id(id: &str) -> Self {
        match id {
            "oauth" => Self::SessionKey,
            "aws_credentials" => Self::AwsCredentials,
            "api_key" | "personal_access_token" | "bot_token"
            | "long_lived_token" | "api_token" | "integration_token" => Self::ApiKey,
            _ if id.contains("none") => Self::None,
            _ => Self::ApiKey,
        }
    }

    /// Get all auth types for a provider from its credential schema
    pub fn all_for_provider(provider_index: usize) -> Vec<Self> {
        // Legacy fallback — prefer using all_for_schema() when schema is available
        match provider_index {
            0 => vec![Self::SessionKey, Self::ApiKey], // Anthropic
            1 => vec![Self::ApiKey],                   // OpenAI
            2 => vec![Self::None],                     // Ollama
            3 => vec![Self::AwsCredentials],           // Bedrock
            4 => vec![Self::ApiKey],                   // Google AI
            _ => vec![Self::ApiKey],
        }
    }

    /// Get auth types from a credential schema
    pub fn all_from_schema(schema: &CredentialSchemaInfo) -> Vec<Self> {
        schema
            .auth_methods
            .iter()
            .map(|m| Self::from_auth_method_id(&m.id))
            .collect()
    }
}

/// Credential schema info (loaded from gateway API)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSchemaInfo {
    pub provider_id: String,
    pub provider_name: String,
    pub category: String,
    pub auth_methods: Vec<AuthMethodInfo>,
}

/// Auth method within a credential schema
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethodInfo {
    pub id: String,
    pub label: String,
    pub fields: Vec<CredentialFieldInfo>,
    pub hint: Option<String>,
    pub docs_url: Option<String>,
}

/// A single field in a credential form
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialFieldInfo {
    pub id: String,
    pub label: String,
    pub secret: bool,
    pub default: Option<String>,
    pub placeholder: Option<String>,
    pub required: bool,
    pub env_var: Option<String>,
}

/// State for the "Edit Provider" modal — driven by credential schemas from the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditProviderState {
    /// Provider index (position in the credential schemas list)
    pub provider_index: usize,
    /// Provider ID (e.g. "bedrock", "anthropic")
    pub provider_id: String,
    /// Provider name (for display)
    pub provider_name: String,
    /// Current field index (0=auth_type, 1+=dynamic fields from schema)
    pub field_index: usize,
    /// Selected auth method index within the schema
    pub auth_method_index: usize,
    /// Selected auth type (derived from auth method)
    pub auth_type: ProviderAuthType,
    /// Dynamic field values keyed by field ID
    pub field_values: Vec<(String, String)>,
    /// The credential schema driving this form
    pub schema: Option<CredentialSchemaInfo>,
}

impl EditProviderState {
    /// Create from a credential schema (preferred — fully dynamic)
    pub fn from_schema(schema: &CredentialSchemaInfo, provider_index: usize) -> Self {
        let auth_type = schema
            .auth_methods
            .first()
            .map(|m| ProviderAuthType::from_auth_method_id(&m.id))
            .unwrap_or(ProviderAuthType::ApiKey);

        let field_values = schema
            .auth_methods
            .first()
            .map(|m| {
                m.fields
                    .iter()
                    .map(|f| (f.id.clone(), f.default.clone().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            provider_index,
            provider_id: schema.provider_id.clone(),
            provider_name: schema.provider_name.clone(),
            field_index: 0,
            auth_method_index: 0,
            auth_type,
            field_values,
            schema: Some(schema.clone()),
        }
    }

    /// Legacy constructor (fallback when no schema available)
    pub fn new(provider_index: usize) -> Self {
        let provider_names = ["Anthropic", "OpenAI", "Ollama", "AWS Bedrock", "Google AI"];
        let name = provider_names.get(provider_index).unwrap_or(&"Unknown");

        Self {
            provider_index,
            provider_id: name.to_lowercase().replace(' ', "_"),
            provider_name: name.to_string(),
            field_index: 0,
            auth_method_index: 0,
            auth_type: ProviderAuthType::ApiKey,
            field_values: vec![],
            schema: None,
        }
    }

    /// Get the currently selected auth method from the schema
    pub fn current_auth_method(&self) -> Option<&AuthMethodInfo> {
        self.schema
            .as_ref()
            .and_then(|s| s.auth_methods.get(self.auth_method_index))
    }

    /// Get total number of fields: 1 (auth type selector) + field count from current auth method
    pub fn total_fields(&self) -> usize {
        let field_count = self
            .current_auth_method()
            .map(|m| m.fields.len())
            .unwrap_or(0);
        1 + field_count // auth_type selector + fields
    }

    pub fn next_field(&mut self) {
        self.field_index = (self.field_index + 1) % self.total_fields();
    }

    pub fn prev_field(&mut self) {
        if self.field_index == 0 {
            self.field_index = self.total_fields() - 1;
        } else {
            self.field_index -= 1;
        }
    }

    pub fn is_auth_type_field(&self) -> bool {
        self.field_index == 0
    }

    /// Cycle through available auth methods
    pub fn cycle_auth_type(&mut self, forward: bool) {
        let count = self
            .schema
            .as_ref()
            .map(|s| s.auth_methods.len())
            .unwrap_or(1);
        if count <= 1 {
            return;
        }
        self.auth_method_index = if forward {
            (self.auth_method_index + 1) % count
        } else {
            if self.auth_method_index == 0 {
                count - 1
            } else {
                self.auth_method_index - 1
            }
        };

        // Update auth_type and rebuild field values
        if let Some(schema) = &self.schema {
            if let Some(method) = schema.auth_methods.get(self.auth_method_index) {
                self.auth_type = ProviderAuthType::from_auth_method_id(&method.id);
                self.field_values = method
                    .fields
                    .iter()
                    .map(|f| (f.id.clone(), f.default.clone().unwrap_or_default()))
                    .collect();
            }
        }
        self.field_index = 0;
    }

    /// Get mutable reference to current field value (for text input)
    pub fn current_value_mut(&mut self) -> Option<&mut String> {
        if self.field_index == 0 {
            return None; // auth_type selector
        }
        let field_idx = self.field_index - 1;
        self.field_values.get_mut(field_idx).map(|(_, v)| v)
    }

    /// Get field label for current field
    pub fn current_field_label(&self) -> String {
        if self.field_index == 0 {
            return "Auth Type".to_string();
        }
        let field_idx = self.field_index - 1;
        self.current_auth_method()
            .and_then(|m| m.fields.get(field_idx))
            .map(|f| f.label.clone())
            .unwrap_or_default()
    }

    /// Check if current field should be masked (password-style)
    pub fn is_current_field_masked(&self) -> bool {
        if self.field_index == 0 {
            return false;
        }
        let field_idx = self.field_index - 1;
        self.current_auth_method()
            .and_then(|m| m.fields.get(field_idx))
            .map(|f| f.secret)
            .unwrap_or(false)
    }

    /// Validate the form — check that all required fields are filled
    pub fn validate(&self) -> Option<String> {
        if let Some(method) = self.current_auth_method() {
            for (i, field) in method.fields.iter().enumerate() {
                if field.required {
                    let value = self
                        .field_values
                        .get(i)
                        .map(|(_, v)| v.as_str())
                        .unwrap_or("");
                    if value.trim().is_empty() {
                        return Some(format!("{} is required", field.label));
                    }
                }
            }
        }
        None
    }

    /// Get a field value by its ID
    pub fn get_field_value_by_id(&self, id: &str) -> Option<&str> {
        self.field_values
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v.as_str())
    }

    // Backward-compatible accessors for common fields

    /// Get the API key value (looks for "api_key" or first secret field)
    pub fn api_key(&self) -> String {
        self.get_field_value_by_id("api_key")
            .or_else(|| self.get_field_value_by_id("bot_token"))
            .or_else(|| self.get_field_value_by_id("access_token"))
            .or_else(|| self.get_field_value_by_id("token"))
            .or_else(|| {
                self.current_auth_method()
                    .and_then(|m| m.fields.iter().find(|f| f.secret))
                    .and_then(|f| self.get_field_value_by_id(&f.id))
            })
            .unwrap_or("")
            .to_string()
    }

    /// Get the base URL value
    pub fn base_url(&self) -> String {
        self.get_field_value_by_id("base_url")
            .unwrap_or("")
            .to_string()
    }

    /// Get the AWS region value
    pub fn aws_region(&self) -> String {
        self.get_field_value_by_id("region")
            .unwrap_or("us-east-1")
            .to_string()
    }

    /// Get env var hint for the primary secret field
    pub fn env_var_hint(&self) -> Option<String> {
        self.current_auth_method()
            .and_then(|m| m.fields.iter().find(|f| f.secret))
            .and_then(|f| f.env_var.clone())
    }
}

/// State for the "Add/Edit Agent" modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditAgentState {
    /// Whether this is an edit (true) or create (false)
    pub is_edit: bool,
    /// Current field index
    pub field_index: usize,
    /// Agent ID (read-only when editing)
    pub id: String,
    /// Model override
    pub model: String,
    /// Parent agent ID (empty = top-level agent)
    pub parent_id: String,
    /// Workspace directory
    pub workspace: String,
    /// Max tool calls per turn
    pub max_tool_calls: String,
    /// System prompt override
    pub system_prompt: String,
    /// Whether the agent is enabled
    pub enabled: bool,
}

impl EditAgentState {
    /// Field labels in order
    pub const FIELD_LABELS: &'static [&'static str] = &[
        "Agent ID",
        "Model",
        "Parent Agent (subagent)",
        "Workspace",
        "Max Tool Calls",
        "System Prompt",
    ];

    pub fn new() -> Self {
        Self {
            is_edit: false,
            field_index: 0,
            id: String::new(),
            model: String::new(),
            parent_id: String::new(),
            workspace: String::new(),
            max_tool_calls: "10".to_string(),
            system_prompt: String::new(),
            enabled: true,
        }
    }

    pub fn from_agent(agent: &AgentInfo) -> Self {
        Self {
            is_edit: true,
            field_index: 0,
            id: agent.id.clone(),
            model: agent.model.clone().unwrap_or_default(),
            parent_id: agent.parent_id.clone().unwrap_or_default(),
            workspace: agent.workspace.clone().unwrap_or_default(),
            max_tool_calls: agent.max_tool_calls.map(|n| n.to_string()).unwrap_or_else(|| "10".to_string()),
            system_prompt: agent.system_prompt.clone().unwrap_or_default(),
            enabled: agent.enabled,
        }
    }

    pub fn total_fields(&self) -> usize {
        Self::FIELD_LABELS.len()
    }

    pub fn next_field(&mut self) {
        self.field_index = (self.field_index + 1) % self.total_fields();
        // Skip ID field when editing (it's read-only)
        if self.is_edit && self.field_index == 0 {
            self.field_index = 1;
        }
    }

    pub fn prev_field(&mut self) {
        if self.field_index == 0 {
            self.field_index = self.total_fields() - 1;
        } else {
            self.field_index -= 1;
        }
        // Skip ID field when editing
        if self.is_edit && self.field_index == 0 {
            self.field_index = self.total_fields() - 1;
        }
    }

    /// Get mutable reference to current field value
    pub fn current_value_mut(&mut self) -> Option<&mut String> {
        match self.field_index {
            0 => if !self.is_edit { Some(&mut self.id) } else { None },
            1 => Some(&mut self.model),
            2 => Some(&mut self.parent_id),
            3 => Some(&mut self.workspace),
            4 => Some(&mut self.max_tool_calls),
            5 => Some(&mut self.system_prompt),
            _ => None,
        }
    }

    pub fn current_field_label(&self) -> &'static str {
        Self::FIELD_LABELS.get(self.field_index).unwrap_or(&"")
    }

    pub fn is_last_field(&self) -> bool {
        self.field_index == self.total_fields() - 1
    }

    pub fn validate(&self) -> Option<String> {
        if self.id.trim().is_empty() {
            return Some("Agent ID is required".to_string());
        }
        if self.id.contains(' ') || self.id.contains('/') {
            return Some("Agent ID cannot contain spaces or slashes".to_string());
        }
        if !self.max_tool_calls.is_empty() {
            if self.max_tool_calls.parse::<u32>().is_err() {
                return Some("Max tool calls must be a number".to_string());
            }
        }
        None
    }
}

/// Definition of a form field for dynamic credential forms.
///
/// Different endpoint types require different fields. This struct defines
/// the metadata for each field so the UI can render appropriate inputs.
///
/// # Example
///
/// ```
/// use rockbot_cli::tui::state::FieldDef;
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
/// use rockbot_cli::tui::state::AddCredentialState;
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
    
    /// Create new state pre-filled from a credential schema (preferred)
    pub fn new_from_schema(schema: &CredentialSchemaInfo) -> Self {
        // Use the first auth method's fields to determine the form
        let auth_method = schema.auth_methods.first();

        // Map schema auth method to vault endpoint type
        let endpoint_type = match auth_method.map(|m| m.id.as_str()) {
            Some("aws_credentials") => 3,       // API Key Service (closest match)
            Some("oauth") => 2,                  // OAuth2 Service
            Some("api_key") | Some("api_token") | Some("personal_access_token")
            | Some("integration_token") => 3,    // API Key Service
            Some("bot_token") | Some("long_lived_token") => 5, // Bearer Token
            _ => 3,                              // Default to API Key Service
        };

        let fields = get_fields_for_endpoint_type(endpoint_type);
        let mut field_values = vec![String::new(); fields.len()];

        // Pre-fill defaults from schema fields
        if let Some(method) = auth_method {
            for schema_field in &method.fields {
                if let Some(default) = &schema_field.default {
                    // Try to match schema field to vault form field
                    for (i, form_field) in fields.iter().enumerate() {
                        if schema_field.id == form_field.id
                            || (schema_field.id == "base_url" && form_field.id == "url")
                            || (schema_field.id.contains("token") && form_field.id == "token")
                            || (schema_field.id.contains("key") && form_field.id == "api_key")
                        {
                            field_values[i] = default.clone();
                            break;
                        }
                    }
                }
            }
        }

        Self {
            field_index: 0,
            name: schema.provider_name.clone(),
            endpoint_type,
            field_values,
        }
    }

    /// Create new state pre-filled for a specific provider (legacy fallback)
    pub fn new_for_provider(provider: &ProviderInfo) -> Self {
        // Default to API Key Service endpoint type
        let endpoint_type = 3;
        let fields = get_fields_for_endpoint_type(endpoint_type);
        let field_values = vec![String::new(); fields.len()];

        Self {
            field_index: 0,
            name: provider.name.clone(),
            endpoint_type,
            field_values,
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
            
            chat_messages: Vec::new(),
            chat_loading: false,
            chat_scroll: 0,
            
            vault: VaultStatus::default(),
            vault_loading: true,
            endpoints: Vec::new(),
            selected_endpoint: 0,
            selected_category: 0,
            selected_provider_index: 0,
            provider_list_focus: false,
            credentials_tab: 0,
            
            providers: Vec::new(),
            selected_provider: 0,
            credential_schemas: Vec::new(),
            
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
            Message::AgentSaved(id) => {
                self.status_message = Some((format!("Agent '{}' saved", id), false));
            }
            Message::AgentSaveError(err) => {
                self.status_message = Some((format!("Failed to save agent: {}", err), true));
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
                // Debug: log vault status
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
                    use std::io::Write;
                    let _ = writeln!(f, "VaultStatus received: initialized={}, locked={}, method={:?}", 
                        status.initialized, status.locked, status.unlock_method);
                }
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
            Message::CredentialAdded(name) => {
                self.status_message = Some((format!("✅ Added: {}", name), false));
            }
            Message::CredentialAddError(err) => {
                self.status_message = Some((format!("❌ Failed: {}", err), true));
            }
            
            Message::ModelsLoaded(providers) => {
                self.providers = providers;
            }

            Message::CredentialSchemasLoaded(schemas) => {
                self.credential_schemas = schemas;
            }

            Message::ChatResponse(content) => {
                self.chat_messages.push(ChatMessage::assistant(content));
                self.chat_loading = false;
            }
            Message::ChatError(err) => {
                self.chat_messages.push(ChatMessage::system(format!("Error: {}", err)));
                self.chat_loading = false;
            }
            Message::ChatStreamChunk(_chunk) => {
                // TODO: Handle streaming chunks for incremental display
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
    
    /// Number of registered LLM providers (dynamic, loaded from gateway)
    pub fn model_provider_count(&self) -> usize {
        self.providers.len().max(1) // At least 1 to avoid div-by-zero
    }
    
    /// Number of credential categories (All, Model, Communication, Tool)
    pub const CREDENTIAL_CATEGORY_COUNT: usize = 4;
    
    /// Get provider count for current category
    pub fn provider_count_for_category(&self) -> usize {
        let schemas = &self.credential_schemas;
        match self.selected_category {
            0 => schemas.len(), // All
            1 => schemas.iter().filter(|s| s.category == "model").count(),
            2 => schemas.iter().filter(|s| s.category == "communication").count(),
            3 => schemas.iter().filter(|s| s.category == "tool").count(),
            _ => 0,
        }
    }
    
    /// Toggle focus between category list and provider list (for Credentials Providers tab)
    pub fn toggle_provider_focus(&mut self) {
        if self.credentials_tab == 1 {
            self.provider_list_focus = !self.provider_list_focus;
            // Reset provider selection when entering provider list
            if self.provider_list_focus {
                self.selected_provider_index = 0;
            }
        }
    }
    
    /// Get the currently selected provider info (id, name, description) for the Providers tab.
    /// Driven by credential schemas loaded from the gateway.
    pub fn get_selected_provider_info(&self) -> Option<ProviderInfo> {
        if self.credentials_tab != 1 {
            return None;
        }

        let schemas = &self.credential_schemas;
        let idx = self.selected_provider_index;

        let filtered: Vec<&CredentialSchemaInfo> = match self.selected_category {
            0 => schemas.iter().collect(), // All
            1 => schemas.iter().filter(|s| s.category == "model").collect(),
            2 => schemas.iter().filter(|s| s.category == "communication").collect(),
            3 => schemas.iter().filter(|s| s.category == "tool").collect(),
            _ => return None,
        };

        filtered.get(idx).map(|s| {
            let category = match s.category.as_str() {
                "model" => ProviderCategory::Model,
                "communication" => ProviderCategory::Communication,
                "tool" => ProviderCategory::Tool,
                _ => ProviderCategory::Tool,
            };
            let description = s
                .auth_methods
                .first()
                .and_then(|m| m.hint.clone())
                .unwrap_or_else(|| {
                    s.auth_methods
                        .first()
                        .map(|m| m.label.clone())
                        .unwrap_or_default()
                });
            ProviderInfo {
                id: s.provider_id.clone(),
                name: s.provider_name.clone(),
                description,
                category,
            }
        })
    }

    /// Get the credential schema for the currently selected provider
    pub fn get_selected_credential_schema(&self) -> Option<&CredentialSchemaInfo> {
        if self.credentials_tab != 1 {
            return None;
        }

        let schemas = &self.credential_schemas;
        let idx = self.selected_provider_index;

        let filtered: Vec<&CredentialSchemaInfo> = match self.selected_category {
            0 => schemas.iter().collect(),
            1 => schemas.iter().filter(|s| s.category == "model").collect(),
            2 => schemas.iter().filter(|s| s.category == "communication").collect(),
            3 => schemas.iter().filter(|s| s.category == "tool").collect(),
            _ => return None,
        };

        filtered.get(idx).cloned()
    }
}

/// Provider information for context-aware add credential
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: ProviderCategory,
}

/// Provider category for determining form fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCategory {
    Model,
    Communication,
    Tool,
}

impl AppState {
    /// Move selection up in current list
    pub fn select_prev(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {}
            MenuItem::Credentials => {
                // Navigate based on which tab is active
                if self.credentials_tab == 1 {
                    // Providers tab
                    if self.provider_list_focus {
                        // Navigate providers within category
                        let count = self.provider_count_for_category();
                        if count > 0 {
                            self.selected_provider_index = if self.selected_provider_index == 0 {
                                count - 1
                            } else {
                                self.selected_provider_index - 1
                            };
                        }
                    } else {
                        // Navigate categories
                        self.selected_category = if self.selected_category == 0 {
                            Self::CREDENTIAL_CATEGORY_COUNT - 1
                        } else {
                            self.selected_category - 1
                        };
                        // Reset provider selection when category changes
                        self.selected_provider_index = 0;
                    }
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
                let count = self.model_provider_count();
                self.selected_provider = if self.selected_provider == 0 {
                    count - 1
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
                    // Providers tab
                    if self.provider_list_focus {
                        // Navigate providers within category
                        let count = self.provider_count_for_category();
                        if count > 0 {
                            self.selected_provider_index = (self.selected_provider_index + 1) % count;
                        }
                    } else {
                        // Navigate categories
                        self.selected_category = (self.selected_category + 1) % Self::CREDENTIAL_CATEGORY_COUNT;
                        // Reset provider selection when category changes
                        self.selected_provider_index = 0;
                    }
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
                let count = self.model_provider_count();
                self.selected_provider = (self.selected_provider + 1) % count;
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
