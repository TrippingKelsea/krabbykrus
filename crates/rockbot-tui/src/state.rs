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
//! use rockbot_tui::state::{AppState, Message};
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
    AgentSaved(String), // agent id
    AgentSaveError(String),

    // Sessions
    SessionsLoaded(Vec<SessionInfo>),
    SessionsError(String),
    ReloadSessions,
    SessionCreated(String), // session id
    SessionCreateError(String),

    // Vault/Credentials
    VaultStatus(VaultStatus),
    VaultUnlocked,
    VaultLocked,
    VaultError(String),
    EndpointsLoaded(Vec<EndpointInfo>),
    CredentialAdded(String), // endpoint name
    CredentialAddError(String),

    // Models
    ModelsLoaded(Vec<ModelProvider>),
    ReloadProviders,

    // Credential schemas (from gateway)
    CredentialSchemasLoaded(Vec<CredentialSchemaInfo>),

    // Chat
    ChatResponse(String, String), // (session_key, AI response text)
    ChatAgentResponse(String, String, Vec<ToolCallInfo>), // (session_key, content, tool_calls)
    ChatError(String, String),    // (session_key, error text)
    ChatStreamChunk(String),      // Streaming chunk: "session_key:text"
    ChatTokenUsage {
        // Structured token usage from gateway
        session_key: String,
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        cumulative_total: u64,
    },
    ChatThinkingStatus {
        // Thinking/processing phase update
        session_key: String,
        phase: String,
        tool_name: Option<String>,
        iteration: Option<usize>,
    },
    SessionMessagesLoaded(String, Vec<ChatMessage>), // (session_key, messages)

    // Cron jobs
    CronJobsLoaded(Vec<CronJobInfo>),
    CronJobToggled(String, bool), // (job_id, new_enabled_state)
    CronJobDeleted(String),       // job_id
    CronJobError(String),         // error message

    // Context files
    ContextFilesLoaded(String, Vec<ContextFileInfo>), // (agent_id, files)
    ContextFileLoaded(String, String, String),        // (agent_id, filename, content)
    ContextFileSaved(String, String),                 // (agent_id, filename)
    ContextFileError(String),                         // error message

    // UI feedback
    SetStatus(String, bool), // (message, is_error)
    ClearStatus,

    // Tick for animations/refresh
    Tick,

    // Exit
    Quit,

    /// Keybinding config reloaded from vault
    KeybindingsReloaded(Box<crate::keybindings::KeybindingConfig>),

    // Butler chat
    ButlerChunk(String),
    ButlerDone(String),
    ButlerError(String),
}

/// Main menu items - unified between TUI and Web UI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuItem {
    #[default]
    Dashboard,
    Credentials,
    Agents,
    Sessions,
    CronJobs,
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
            Self::CronJobs,
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
            Self::CronJobs => "Cron Jobs",
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
            Self::CronJobs => "🕐",
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
            Self::CronJobs => 4,
            Self::Models => 5,
            Self::Settings => 6,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx % 7 {
            0 => Self::Dashboard,
            1 => Self::Credentials,
            2 => Self::Agents,
            3 => Self::Sessions,
            4 => Self::CronJobs,
            5 => Self::Models,
            _ => Self::Settings,
        }
    }
}

// ---------------------------------------------------------------------------
// Slotted Card Bar Navigation
// ---------------------------------------------------------------------------

/// Identifies a compact card-sized widget for rendering inside card slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardWidgetId {
    GatewayStatus,
    GatewayLoad,
    GatewayNetwork,
    ClientStatus,
    ClientMessages,
    ClientResources,
    AgentOverview,
    AgentSessions,
    AgentTools,
    VaultStatus,
    CronOverview,
    ModelsOverview,
    SettingsGeneral,
    Alerts,
}

/// Severity level for alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
}

/// A single alert item.
#[derive(Debug, Clone)]
pub struct AlertItem {
    pub severity: AlertSeverity,
    pub message: String,
    pub source: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// What kind of slot this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    ModeSelector,
    InfoCard,
}

/// A single view within a card slot.
#[derive(Debug, Clone)]
pub struct SlotView {
    pub label: String,
    pub widget: CardWidgetId,
}

/// A single slot in the card bar.
#[derive(Debug, Clone)]
pub struct CardSlot {
    pub label: String,
    pub icon: char,
    pub badge: Option<String>,
    pub views: Vec<SlotView>,
    pub active_view: usize,
    pub kind: SlotKind,
}

/// A fixed "mode" card at slot 0 with dynamic info cards to the right.
pub struct SlottedCardBar {
    pub mode: usize,
    pub slots: Vec<CardSlot>,
    pub active_slot: usize,
}

impl SlottedCardBar {
    pub fn new() -> Self {
        let mut bar = Self {
            mode: 0,
            slots: vec![CardSlot {
                label: MenuItem::Dashboard.title().to_string(),
                icon: '*',
                badge: None,
                views: vec![],
                active_view: 0,
                kind: SlotKind::ModeSelector,
            }],
            active_slot: 0,
        };
        bar.slots.extend(build_dashboard_slots());
        bar
    }

    pub fn select_left(&mut self) {
        if self.active_slot > 0 {
            self.active_slot -= 1;
        }
    }

    pub fn select_right(&mut self) {
        if self.active_slot + 1 < self.slots.len() {
            self.active_slot += 1;
        }
    }

    /// Cycle up on slot 0 = prev mode; on slots 1+ = prev view
    pub fn cycle_up(&mut self, agents: &[AgentInfo], sessions: &[SessionInfo]) {
        if self.active_slot == 0 {
            let modes = MenuItem::all();
            if self.mode > 0 {
                self.mode -= 1;
            } else {
                self.mode = modes.len() - 1;
            }
            self.slots[0].label = modes[self.mode].title().to_string();
            self.rebuild_content_slots(agents, sessions);
        } else if let Some(slot) = self.slots.get_mut(self.active_slot) {
            if !slot.views.is_empty() {
                if slot.active_view > 0 {
                    slot.active_view -= 1;
                } else {
                    slot.active_view = slot.views.len() - 1;
                }
            }
        }
    }

    /// Cycle down on slot 0 = next mode; on slots 1+ = next view
    pub fn cycle_down(&mut self, agents: &[AgentInfo], sessions: &[SessionInfo]) {
        if self.active_slot == 0 {
            let modes = MenuItem::all();
            self.mode = (self.mode + 1) % modes.len();
            self.slots[0].label = modes[self.mode].title().to_string();
            self.rebuild_content_slots(agents, sessions);
        } else if let Some(slot) = self.slots.get_mut(self.active_slot) {
            if !slot.views.is_empty() {
                slot.active_view = (slot.active_view + 1) % slot.views.len();
            }
        }
    }

    /// Current mode as MenuItem
    pub fn current_mode(&self) -> MenuItem {
        MenuItem::from_index(self.mode)
    }

    /// Widget ID for the currently active slot's current view
    pub fn active_widget(&self) -> Option<CardWidgetId> {
        let slot = self.slots.get(self.active_slot)?;
        let view = slot.views.get(slot.active_view)?;
        Some(view.widget)
    }

    /// Rebuild slots 1+ based on current mode
    pub fn rebuild_content_slots(&mut self, agents: &[AgentInfo], sessions: &[SessionInfo]) {
        self.slots.truncate(1);
        let new_slots = match self.current_mode() {
            MenuItem::Dashboard => build_dashboard_slots(),
            MenuItem::Agents => build_agents_slots_from(agents),
            MenuItem::Sessions => build_sessions_slots_from(sessions),
            MenuItem::Credentials => build_credentials_slots(),
            MenuItem::CronJobs => build_cron_slots(),
            MenuItem::Models => build_models_slots(),
            MenuItem::Settings => build_settings_slots(),
        };
        self.slots.extend(new_slots);
        // Pinned alerts card — always rightmost
        self.slots.push(CardSlot {
            label: "Alerts".to_string(),
            icon: '!',
            badge: None,
            views: vec![SlotView {
                label: "Alerts".to_string(),
                widget: CardWidgetId::Alerts,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        });
        if self.active_slot >= self.slots.len() {
            self.active_slot = self.slots.len().saturating_sub(1);
        }
    }
}

fn build_dashboard_slots() -> Vec<CardSlot> {
    vec![
        CardSlot {
            label: "Gateway".to_string(),
            icon: 'G',
            badge: None,
            views: vec![
                SlotView {
                    label: "Status".to_string(),
                    widget: CardWidgetId::GatewayStatus,
                },
                SlotView {
                    label: "Load".to_string(),
                    widget: CardWidgetId::GatewayLoad,
                },
                SlotView {
                    label: "Network".to_string(),
                    widget: CardWidgetId::GatewayNetwork,
                },
            ],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Client".to_string(),
            icon: 'C',
            badge: None,
            views: vec![
                SlotView {
                    label: "Status".to_string(),
                    widget: CardWidgetId::ClientStatus,
                },
                SlotView {
                    label: "Messages".to_string(),
                    widget: CardWidgetId::ClientMessages,
                },
                SlotView {
                    label: "Resources".to_string(),
                    widget: CardWidgetId::ClientResources,
                },
            ],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Agents".to_string(),
            icon: 'A',
            badge: None,
            views: vec![
                SlotView {
                    label: "Overview".to_string(),
                    widget: CardWidgetId::AgentOverview,
                },
                SlotView {
                    label: "Sessions".to_string(),
                    widget: CardWidgetId::AgentSessions,
                },
                SlotView {
                    label: "Tools".to_string(),
                    widget: CardWidgetId::AgentTools,
                },
            ],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Vault".to_string(),
            icon: 'V',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::VaultStatus,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
    ]
}

fn build_agents_slots_from(agents: &[AgentInfo]) -> Vec<CardSlot> {
    agents
        .iter()
        .map(|a| CardSlot {
            label: a.id.clone(),
            icon: if a.enabled { '+' } else { 'o' },
            badge: Some(format!("{}", a.session_count)),
            views: vec![
                SlotView {
                    label: "Overview".to_string(),
                    widget: CardWidgetId::AgentOverview,
                },
                SlotView {
                    label: "Sessions".to_string(),
                    widget: CardWidgetId::AgentSessions,
                },
            ],
            active_view: 0,
            kind: SlotKind::InfoCard,
        })
        .collect()
}

fn build_sessions_slots_from(sessions: &[SessionInfo]) -> Vec<CardSlot> {
    sessions
        .iter()
        .map(|s| CardSlot {
            label: s.key.clone(),
            icon: 'S',
            badge: Some(format!("{}", s.message_count)),
            views: vec![SlotView {
                label: "Messages".to_string(),
                widget: CardWidgetId::ClientMessages,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        })
        .collect()
}

fn build_credentials_slots() -> Vec<CardSlot> {
    vec![
        CardSlot {
            label: "Endpoints".to_string(),
            icon: 'E',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::VaultStatus,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Providers".to_string(),
            icon: 'P',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::GatewayStatus,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
    ]
}

fn build_cron_slots() -> Vec<CardSlot> {
    vec![
        CardSlot {
            label: "All Jobs".to_string(),
            icon: 'A',
            badge: None,
            views: vec![SlotView {
                label: "Overview".to_string(),
                widget: CardWidgetId::CronOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Active".to_string(),
            icon: '+',
            badge: None,
            views: vec![SlotView {
                label: "Active".to_string(),
                widget: CardWidgetId::CronOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Disabled".to_string(),
            icon: 'o',
            badge: None,
            views: vec![SlotView {
                label: "Disabled".to_string(),
                widget: CardWidgetId::CronOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
    ]
}

fn build_models_slots() -> Vec<CardSlot> {
    vec![
        CardSlot {
            label: "Bedrock".to_string(),
            icon: 'B',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::ModelsOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Anthropic".to_string(),
            icon: 'A',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::ModelsOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "OpenAI".to_string(),
            icon: 'O',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::ModelsOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Ollama".to_string(),
            icon: 'L',
            badge: None,
            views: vec![SlotView {
                label: "Status".to_string(),
                widget: CardWidgetId::ModelsOverview,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
    ]
}

fn build_settings_slots() -> Vec<CardSlot> {
    vec![
        CardSlot {
            label: "General".to_string(),
            icon: 'G',
            badge: None,
            views: vec![SlotView {
                label: "General".to_string(),
                widget: CardWidgetId::SettingsGeneral,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "Paths".to_string(),
            icon: 'P',
            badge: None,
            views: vec![SlotView {
                label: "Paths".to_string(),
                widget: CardWidgetId::SettingsGeneral,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
        CardSlot {
            label: "About".to_string(),
            icon: 'i',
            badge: None,
            views: vec![SlotView {
                label: "About".to_string(),
                widget: CardWidgetId::SettingsGeneral,
            }],
            active_view: 0,
            kind: SlotKind::InfoCard,
        },
    ]
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
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
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

/// Cron job information for the TUI
#[derive(Debug, Clone)]
pub struct CronJobInfo {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub agent_id: Option<String>,
    pub schedule: String,
    pub last_run: Option<String>,
    pub last_status: Option<String>,
    pub next_run: Option<String>,
}

/// Session information
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub key: String,
    pub agent_id: String,
    pub channel: Option<String>,
    pub started_at: Option<String>,
    pub message_count: usize,
    pub model: Option<String>,
}

/// Thinking phase tracking for the AI processing indicator
#[derive(Debug, Clone, Default)]
pub struct ThinkingState {
    /// Current processing phase ("llm", "tool", etc.)
    pub phase: String,
    /// Name of tool currently running (if phase == "tool")
    pub tool_name: Option<String>,
    /// Current iteration number
    pub iteration: Option<usize>,
    /// Cumulative prompt tokens consumed so far
    pub prompt_tokens: u64,
    /// Cumulative completion tokens generated so far
    pub completion_tokens: u64,
    /// Cumulative total tokens
    pub cumulative_total: u64,
    /// When processing started (for elapsed time / tok/s calculation)
    pub started_at: Option<std::time::Instant>,
}

impl ThinkingState {
    /// Average completion tokens per second since processing started
    pub fn tokens_per_second(&self) -> f64 {
        let elapsed = self
            .started_at
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        if elapsed > 0.5 {
            self.completion_tokens as f64 / elapsed
        } else {
            0.0
        }
    }
}

/// Per-session chat state
#[derive(Debug, Clone)]
pub struct SessionChatState {
    pub messages: Vec<ChatMessage>,
    pub scroll: usize,
    pub loading: bool,
    pub loaded: bool,      // Whether history has been fetched from gateway
    pub auto_scroll: bool, // When true, scroll follows latest content
    pub max_scroll: std::cell::Cell<usize>, // Last computed max scroll value (updated during render)
    /// Live thinking/processing state for the spinner
    pub thinking: ThinkingState,
}

impl Default for SessionChatState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            scroll: 0,
            loading: false,
            loaded: false,
            auto_scroll: true,
            max_scroll: std::cell::Cell::new(0),
            thinking: ThinkingState::default(),
        }
    }
}

/// Chat message role
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
}

/// Tool call information for display
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub success: bool,
    pub duration_ms: u64,
    pub expanded: bool,
}

/// A message in a chat session
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    pub timestamp: Option<String>,
    pub tool_calls: Vec<ToolCallInfo>,
}

impl ChatMessage {
    pub fn user(content: String) -> Self {
        Self {
            role: ChatRole::User,
            content,
            timestamp: Some(chrono::Local::now().format("%H:%M:%S").to_string()),
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            timestamp: Some(chrono::Local::now().format("%H:%M:%S").to_string()),
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_with_tools(content: String, tool_calls: Vec<ToolCallInfo>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content,
            timestamp: Some(chrono::Local::now().format("%H:%M:%S").to_string()),
            tool_calls,
        }
    }

    pub fn system(content: String) -> Self {
        Self {
            role: ChatRole::System,
            content,
            timestamp: None,
            tool_calls: Vec::new(),
        }
    }
}

/// Vault unlock method
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UnlockMethod {
    #[default]
    Unknown,
    Password,
    Keyfile {
        path: Option<String>,
    },
    Age {
        public_key: Option<String>,
    },
    SshKey {
        path: Option<String>,
    },
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

/// Access level for a credential permission rule.
/// Matches `PermissionLevel` from rockbot-credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AccessLevel {
    /// Execute immediately without human involvement
    Allow,
    /// Human-in-the-loop: requires approval before each access
    #[default]
    AllowHil,
    /// Human-in-the-loop with YubiKey/2FA verification
    AllowHil2fa,
    /// Reject request and log attempt
    Deny,
}

impl AccessLevel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Allow => "Allow",
            Self::AllowHil => "Allow HIL",
            Self::AllowHil2fa => "Allow HIL+2FA",
            Self::Deny => "Deny",
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Allow => "ALLOW",
            Self::AllowHil => "HIL",
            Self::AllowHil2fa => "2FA",
            Self::Deny => "DENY",
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        use ratatui::style::Color;
        match self {
            Self::Allow => Color::Green,
            Self::AllowHil => Color::Yellow,
            Self::AllowHil2fa => Color::Magenta,
            Self::Deny => Color::Red,
        }
    }
}

/// Source that can access a credential
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionSource {
    /// Any source (wildcard)
    Any,
    /// The gateway/system itself (e.g., for provider credentials)
    System,
    /// A specific agent
    Agent(String),
}

impl PermissionSource {
    pub fn label(&self) -> String {
        match self {
            Self::Any => "Any Source".to_string(),
            Self::System => "System / Gateway".to_string(),
            Self::Agent(id) => format!("Agent: {id}"),
        }
    }

    pub fn short_label(&self) -> String {
        match self {
            Self::Any => "*".to_string(),
            Self::System => "system".to_string(),
            Self::Agent(id) => id.clone(),
        }
    }
}

/// A permission rule granting a source access to a credential endpoint.
/// Rules are evaluated in order (lowest priority number first).
/// An implicit Deny-all rule exists at the end (not stored).
#[derive(Debug, Clone)]
pub struct PermissionRule {
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub source: PermissionSource,
    pub access: AccessLevel,
    pub priority: usize, // rule evaluation order (1-based)
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
    // Slotted card bar navigation
    pub slot_bar: SlottedCardBar,

    // Paths
    pub config_path: PathBuf,
    pub vault_path: PathBuf,
    pub launch_dir: PathBuf,

    // Gateway connection (WS URL for WebSocket, HTTP URL for REST API)
    pub gateway_url: String,
    pub gateway_http_url: String,

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

    // Butler chat (permanent companion chat, always visible)
    pub butler_chat: SessionChatState,

    // Chat state — per-session
    pub session_chats: std::collections::HashMap<String, SessionChatState>,
    pub chat_model: Option<String>,    // Model ID for current chat
    pub chat_agent_id: Option<String>, // Agent ID if agent-bound session

    // Vault/Credentials
    pub vault: VaultStatus,
    pub vault_loading: bool,
    pub endpoints: Vec<EndpointInfo>,
    pub selected_endpoint: usize,
    pub selected_category: usize, // For Providers tab - which category
    pub selected_provider_index: usize, // For Providers tab - which provider within category
    pub provider_list_focus: bool, // true = right panel (provider list), false = left panel (categories)
    pub credentials_tab: usize,    // Which tab is active (0=Endpoints, 1=Providers, etc.)
    pub permissions: Vec<PermissionRule>,
    pub selected_permission: usize,

    // Models (dynamically loaded from gateway)
    pub providers: Vec<ModelProvider>,
    pub selected_provider: usize,

    // Cron jobs
    pub cron_jobs: Vec<CronJobInfo>,
    pub cron_loading: bool,
    pub selected_cron_job: usize,
    pub selected_cron_card: usize,

    // History buffers for sparkline widgets
    pub gateway_load_history: std::collections::VecDeque<u64>,
    pub client_msg_history: std::collections::VecDeque<u64>,
    // Settings card selection (General=0, Paths=1, About=2)
    pub selected_settings_card: usize,

    // Credential schemas (from gateway — drives Credentials->Providers forms)
    pub credential_schemas: Vec<CredentialSchemaInfo>,

    // Alerts
    pub alerts: Vec<AlertItem>,

    // UI state
    pub status_message: Option<(String, bool)>, // (message, is_error)
    pub should_exit: bool,
    pub tick_count: usize,

    // Input modes (for modals, text input, etc.)
    pub input_mode: InputMode,
    pub input_buffer: String,
    /// Cursor byte position within input_buffer
    pub input_cursor: usize,

    // TUI display preferences
    pub tui_config: rockbot_core::TuiConfig,

    // Message sender for async updates
    pub tx: mpsc::UnboundedSender<Message>,
}

/// Input modes for capturing text/modal interactions
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    /// Password input for vault
    PasswordInput {
        prompt: String,
        masked: bool,
        action: PasswordAction,
    },
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
    /// Create session modal
    CreateSession(CreateSessionState),
    /// Confirmation dialog
    Confirm {
        message: String,
        action: ConfirmAction,
    },
    /// Chat input
    ChatInput,
    /// View session details
    ViewSession { session_key: String },
    /// View endpoint details (read-only modal, 'e' to edit)
    ViewEndpoint { endpoint_index: usize },
    /// View provider details (read-only modal, 'e' to edit)
    ViewProvider { provider_index: usize },
    /// View full model list for a provider
    ViewModelList {
        provider_index: usize,
        scroll: usize,
    },
    /// View permission rule details (read-only, 'e' to edit, +/- to reorder)
    ViewPermission { permission_index: usize },
    /// Edit permission for a credential endpoint
    EditPermission(EditPermissionState),
    /// Browse context files for an agent
    ViewContextFiles(ViewContextFilesState),
    /// Edit a context file (fullscreen markdown editor)
    EditContextFile(EditContextFileState),
    /// Context menu (page-specific actions, opened with '?')
    ContextMenu(ContextMenuState),
    /// Card detail overlay (Alt+Enter on a card slot)
    CardDetail(CardDetailState),
}

/// State for a card detail overlay modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDetailState {
    /// Which mode (MenuItem) the card belongs to.
    pub mode: MenuItem,
    /// Which slot index (1+) was activated.
    pub slot_index: usize,
    /// Scroll offset within the detail content.
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordAction {
    InitVault,
    UnlockVault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteEndpoint(String),                    // endpoint id
    DeleteAgent(String),                       // agent id
    KillSession(String),                       // session key
    DisableAgent(String), // agent id (different from delete - actually disables in config)
    DeleteCronJob(String), // cron job id
    DiscardContextFile(ViewContextFilesState), // return to file browser state
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
            if field.required && self.field_values.get(i).is_none_or(|v| v.trim().is_empty()) {
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
                return self.field_values.get(i).map(std::string::String::as_str);
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
            "aws_credentials" | "aws_bearer_token" | "agentcore_oauth2" | "agentcore_api_key" => {
                Self::AwsCredentials
            }
            "api_key"
            | "personal_access_token"
            | "bot_token"
            | "long_lived_token"
            | "api_token"
            | "integration_token" => Self::ApiKey,
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
            .map_or(ProviderAuthType::ApiKey, |m| {
                ProviderAuthType::from_auth_method_id(&m.id)
            });

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
        let field_count = self.current_auth_method().map_or(0, |m| m.fields.len());
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
        let count = self.schema.as_ref().map_or(1, |s| s.auth_methods.len());
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
            .is_some_and(|f| f.secret)
    }

    /// Validate the form — check that all required fields are filled
    pub fn validate(&self) -> Option<String> {
        if let Some(method) = self.current_auth_method() {
            for (i, field) in method.fields.iter().enumerate() {
                if field.required {
                    let value = self.field_values.get(i).map_or("", |(_, v)| v.as_str());
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

/// A model option for the model picker dropdown
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOption {
    /// Display-friendly label (e.g., "Claude Sonnet 4")
    pub label: String,
    /// Value to store (e.g., "anthropic/claude-sonnet-4-20250514")
    pub value: String,
    /// Provider name for grouping
    pub provider: String,
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
    /// LLM temperature (0.0-2.0)
    pub temperature: String,
    /// LLM max response tokens
    pub max_tokens: String,
    /// System prompt override
    pub system_prompt: String,
    /// Whether the agent is enabled
    pub enabled: bool,
    /// Available models for picker (populated from providers)
    pub available_models: Vec<ModelOption>,
    /// Currently selected model index in the picker (-1 or None = custom text)
    pub selected_model_index: Option<usize>,
}

impl Default for EditAgentState {
    fn default() -> Self {
        Self::new()
    }
}

impl EditAgentState {
    /// Field labels in order
    pub const FIELD_LABELS: &'static [&'static str] = &[
        "Agent ID",
        "Model",
        "Parent Agent (subagent)",
        "Workspace",
        "Max Tool Calls",
        "Temperature",
        "Max Tokens",
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
            max_tool_calls: String::new(),
            temperature: "0.3".to_string(),
            max_tokens: "16000".to_string(),
            system_prompt: String::new(),
            enabled: true,
            available_models: Vec::new(),
            selected_model_index: None,
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
            max_tool_calls: agent
                .max_tool_calls
                .map_or_else(String::new, |n| n.to_string()),
            temperature: agent
                .temperature
                .map_or_else(|| "0.3".to_string(), |n| format!("{n}")),
            max_tokens: agent
                .max_tokens
                .map_or_else(|| "16000".to_string(), |n| n.to_string()),
            system_prompt: agent.system_prompt.clone().unwrap_or_default(),
            enabled: agent.enabled,
            available_models: Vec::new(),
            selected_model_index: None,
        }
    }

    /// Populate the model picker from available providers
    pub fn populate_models(&mut self, providers: &[ModelProvider]) {
        self.available_models.clear();
        for provider in providers {
            if !provider.available {
                continue;
            }
            for model in &provider.models {
                let value = format!("{}/{}", provider.id, model.id);
                self.available_models.push(ModelOption {
                    label: format!("{} ({})", model.name, provider.name),
                    value: value.clone(),
                    provider: provider.name.clone(),
                });
                // If current model matches, select it
                if self.model == value {
                    self.selected_model_index = Some(self.available_models.len() - 1);
                }
            }
        }
    }

    /// Cycle to next model in the picker
    pub fn next_model(&mut self) {
        if self.available_models.is_empty() {
            return;
        }
        let next = match self.selected_model_index {
            Some(i) if i + 1 < self.available_models.len() => Some(i + 1),
            Some(_) => None, // wrap to "custom" (no selection)
            None => Some(0),
        };
        self.selected_model_index = next;
        if let Some(idx) = next {
            self.model = self.available_models[idx].value.clone();
        }
    }

    /// Cycle to previous model in the picker
    pub fn prev_model(&mut self) {
        if self.available_models.is_empty() {
            return;
        }
        let prev = match self.selected_model_index {
            Some(0) => None, // wrap to "custom"
            Some(i) => Some(i - 1),
            None => Some(self.available_models.len() - 1),
        };
        self.selected_model_index = prev;
        if let Some(idx) = prev {
            self.model = self.available_models[idx].value.clone();
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

    /// Returns true if the model field is using the picker (has available models)
    pub fn is_model_picker_active(&self) -> bool {
        self.field_index == 1 && !self.available_models.is_empty()
    }

    /// Get mutable reference to current field value
    pub fn current_value_mut(&mut self) -> Option<&mut String> {
        match self.field_index {
            0 => {
                if !self.is_edit {
                    Some(&mut self.id)
                } else {
                    None
                }
            }
            // Model field: only allow text input if no models are available (fallback)
            1 if self.available_models.is_empty() => Some(&mut self.model),
            1 => None, // picker mode — use next_model/prev_model instead
            2 => Some(&mut self.parent_id),
            3 => Some(&mut self.workspace),
            4 => Some(&mut self.max_tool_calls),
            5 => Some(&mut self.temperature),
            6 => Some(&mut self.max_tokens),
            7 => Some(&mut self.system_prompt),
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
        if !self.max_tool_calls.is_empty() && self.max_tool_calls.parse::<u32>().is_err() {
            return Some("Max tool calls must be a number".to_string());
        }
        None
    }
}

/// Info about a context file in an agent's directory
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ContextFileInfo {
    pub name: String,
    pub exists: bool,
    pub size_bytes: u64,
    pub well_known: bool,
}

/// State for browsing an agent's context files
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewContextFilesState {
    pub agent_id: String,
    pub files: Vec<ContextFileInfo>,
    pub selected: usize,
    pub loading: bool,
}

/// State for editing a single context file
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditContextFileState {
    pub agent_id: String,
    pub filename: String,
    pub content: String,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub is_dirty: bool,
    pub is_loading: bool,
    pub is_new_file: bool,
}

impl EditContextFileState {
    pub fn new(agent_id: String, filename: String, content: String) -> Self {
        let is_new_file = content.is_empty();
        Self {
            agent_id,
            filename,
            content,
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0,
            is_dirty: false,
            is_loading: false,
            is_new_file,
        }
    }

    /// Insert a character at the current cursor position
    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.cursor_byte_position();
        self.content.insert(byte_pos, c);
        if c == '\n' {
            self.cursor_line += 1;
            self.cursor_col = 0;
        } else {
            self.cursor_col += 1;
        }
        self.is_dirty = true;
    }

    /// Delete the character before the cursor (backspace)
    pub fn delete_char(&mut self) {
        if self.cursor_col == 0 && self.cursor_line == 0 {
            return;
        }
        let byte_pos = self.cursor_byte_position();
        if byte_pos == 0 {
            return;
        }
        // Find the previous character boundary
        let prev_char = self.content[..byte_pos].chars().next_back();
        if let Some(ch) = prev_char {
            let char_start = byte_pos - ch.len_utf8();
            self.content.remove(char_start);
            if ch == '\n' {
                // Move cursor to end of previous line
                self.cursor_line -= 1;
                let prev_line = self.lines().nth(self.cursor_line).unwrap_or("");
                self.cursor_col = prev_line.len();
            } else {
                self.cursor_col -= 1;
            }
            self.is_dirty = true;
        }
    }

    /// Get the byte position of the cursor
    fn cursor_byte_position(&self) -> usize {
        let mut pos = 0;
        for (i, line) in self.content.split('\n').enumerate() {
            if i == self.cursor_line {
                return pos + self.cursor_col.min(line.len());
            }
            pos += line.len() + 1; // +1 for the \n
        }
        self.content.len()
    }

    /// Get an iterator over lines
    fn lines(&self) -> std::str::Split<'_, char> {
        self.content.split('\n')
    }

    /// Total number of lines
    pub fn line_count(&self) -> usize {
        self.content.split('\n').count()
    }

    /// Move cursor up
    pub fn cursor_up(&mut self) {
        if self.cursor_line > 0 {
            self.cursor_line -= 1;
            let line_len = self.lines().nth(self.cursor_line).unwrap_or("").len();
            self.cursor_col = self.cursor_col.min(line_len);
        }
    }

    /// Move cursor down
    pub fn cursor_down(&mut self) {
        if self.cursor_line + 1 < self.line_count() {
            self.cursor_line += 1;
            let line_len = self.lines().nth(self.cursor_line).unwrap_or("").len();
            self.cursor_col = self.cursor_col.min(line_len);
        }
    }

    /// Move cursor left
    pub fn cursor_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            let line_len = self.lines().nth(self.cursor_line).unwrap_or("").len();
            self.cursor_col = line_len;
        }
    }

    /// Move cursor right
    pub fn cursor_right(&mut self) {
        let line_len = self.lines().nth(self.cursor_line).unwrap_or("").len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.line_count() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    /// Ensure the cursor is visible by adjusting scroll_offset
    pub fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        if visible_lines == 0 {
            return;
        }
        if self.cursor_line < self.scroll_offset {
            self.scroll_offset = self.cursor_line;
        } else if self.cursor_line >= self.scroll_offset + visible_lines {
            self.scroll_offset = self.cursor_line - visible_lines + 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Context menu types
// ---------------------------------------------------------------------------

/// A single item in the context menu
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuItem {
    pub label: String,
    pub key: char,
    pub action: ContextMenuAction,
}

/// Actions the context menu can dispatch
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    OpenAddAgent,
    OpenEditAgent,
    DeleteAgent,
    OpenAddCredential,
    OpenCreateSession,
    OpenContextFiles,
    TriggerCronJob,
    RefreshPage,
}

/// State for the context menu overlay
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenuState {
    pub items: Vec<ContextMenuItem>,
    pub selected: usize,
    pub position: (u16, u16),
}

impl AppState {
    /// Build a context menu with page-specific items
    pub fn build_context_menu(&self) -> ContextMenuState {
        let items = match self.menu_item {
            MenuItem::Agents => vec![
                ContextMenuItem {
                    label: "Add Agent".to_string(),
                    key: 'a',
                    action: ContextMenuAction::OpenAddAgent,
                },
                ContextMenuItem {
                    label: "Edit Agent".to_string(),
                    key: 'e',
                    action: ContextMenuAction::OpenEditAgent,
                },
                ContextMenuItem {
                    label: "Delete Agent".to_string(),
                    key: 'd',
                    action: ContextMenuAction::DeleteAgent,
                },
                ContextMenuItem {
                    label: "Context Files".to_string(),
                    key: 'f',
                    action: ContextMenuAction::OpenContextFiles,
                },
                ContextMenuItem {
                    label: "Refresh".to_string(),
                    key: 'r',
                    action: ContextMenuAction::RefreshPage,
                },
            ],
            MenuItem::Credentials => vec![
                ContextMenuItem {
                    label: "Add Credential".to_string(),
                    key: 'a',
                    action: ContextMenuAction::OpenAddCredential,
                },
                ContextMenuItem {
                    label: "Refresh".to_string(),
                    key: 'r',
                    action: ContextMenuAction::RefreshPage,
                },
            ],
            MenuItem::Sessions => vec![
                ContextMenuItem {
                    label: "New Session".to_string(),
                    key: 'n',
                    action: ContextMenuAction::OpenCreateSession,
                },
                ContextMenuItem {
                    label: "Refresh".to_string(),
                    key: 'r',
                    action: ContextMenuAction::RefreshPage,
                },
            ],
            MenuItem::CronJobs => vec![
                ContextMenuItem {
                    label: "Trigger Job".to_string(),
                    key: 't',
                    action: ContextMenuAction::TriggerCronJob,
                },
                ContextMenuItem {
                    label: "Refresh".to_string(),
                    key: 'r',
                    action: ContextMenuAction::RefreshPage,
                },
            ],
            _ => vec![ContextMenuItem {
                label: "Refresh".to_string(),
                key: 'r',
                action: ContextMenuAction::RefreshPage,
            }],
        };

        ContextMenuState {
            items,
            selected: 0,
            position: (2, 6), // below the top bar
        }
    }
}

/// Session creation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    /// Ad-hoc: no agent, user picks model, not in agent memory
    AdHoc,
    /// Agent-bound: picks agent, model from agent config, goes to agent memory
    AgentBound,
}

/// State for the "Create Session" modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSessionState {
    /// Current mode
    pub mode: SessionMode,
    /// Current field index (0=mode, 1=model/agent picker)
    pub field_index: usize,
    /// Selected model for ad-hoc mode
    pub available_models: Vec<ModelOption>,
    pub selected_model_index: usize,
    /// Selected agent for agent-bound mode
    pub available_agents: Vec<(String, String)>, // (id, display_name)
    pub selected_agent_index: usize,
}

impl CreateSessionState {
    pub fn new(providers: &[ModelProvider], agents: &[AgentInfo]) -> Self {
        let mut available_models = Vec::new();
        for provider in providers {
            if !provider.available {
                continue;
            }
            for model in &provider.models {
                available_models.push(ModelOption {
                    label: format!("{} ({})", model.name, provider.name),
                    value: format!("{}/{}", provider.id, model.id),
                    provider: provider.name.clone(),
                });
            }
        }

        let available_agents: Vec<(String, String)> = agents
            .iter()
            .filter(|a| a.enabled)
            .map(|a| {
                let display = if let Some(ref model) = a.model {
                    format!("{} [{}]", a.id, model)
                } else {
                    a.id.clone()
                };
                (a.id.clone(), display)
            })
            .collect();

        Self {
            mode: SessionMode::AdHoc,
            field_index: 1, // Start on the model/agent picker for quick selection
            available_models,
            selected_model_index: 0,
            available_agents,
            selected_agent_index: 0,
        }
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            SessionMode::AdHoc => SessionMode::AgentBound,
            SessionMode::AgentBound => SessionMode::AdHoc,
        };
        self.field_index = 0;
    }

    pub fn next_option(&mut self) {
        match (self.mode, self.field_index) {
            (SessionMode::AdHoc, 1) if !self.available_models.is_empty() => {
                self.selected_model_index =
                    (self.selected_model_index + 1) % self.available_models.len();
            }
            (SessionMode::AgentBound, 1) if !self.available_agents.is_empty() => {
                self.selected_agent_index =
                    (self.selected_agent_index + 1) % self.available_agents.len();
            }
            _ => {}
        }
    }

    pub fn prev_option(&mut self) {
        match (self.mode, self.field_index) {
            (SessionMode::AdHoc, 1) if !self.available_models.is_empty() => {
                if self.selected_model_index == 0 {
                    self.selected_model_index = self.available_models.len() - 1;
                } else {
                    self.selected_model_index -= 1;
                }
            }
            (SessionMode::AgentBound, 1) if !self.available_agents.is_empty() => {
                if self.selected_agent_index == 0 {
                    self.selected_agent_index = self.available_agents.len() - 1;
                } else {
                    self.selected_agent_index -= 1;
                }
            }
            _ => {}
        }
    }

    pub fn selected_model(&self) -> Option<&str> {
        self.available_models
            .get(self.selected_model_index)
            .map(|m| m.value.as_str())
    }

    pub fn selected_agent_id(&self) -> Option<&str> {
        self.available_agents
            .get(self.selected_agent_index)
            .map(|(id, _)| id.as_str())
    }

    pub fn total_fields(&self) -> usize {
        2 // mode selector + model/agent picker
    }
}

/// State for the permission editor modal
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPermissionState {
    /// Available endpoints to assign permissions to: (id, name)
    pub endpoints: Vec<(String, String)>,
    /// Selected endpoint index
    pub selected_endpoint: usize,
    /// Available sources: Any, System, then each agent
    pub sources: Vec<PermissionSource>,
    /// Selected source index
    pub selected_source: usize,
    /// Access level for the selected source
    pub access: AccessLevel,
    /// Field index: 0=endpoint, 1=source, 2=access
    pub field_index: usize,
    /// Whether editing an existing rule (vs creating new)
    pub is_edit: bool,
}

impl EditPermissionState {
    pub fn new(
        endpoints: &[EndpointInfo],
        agents: &[AgentInfo],
        preselect_endpoint: Option<usize>,
    ) -> Self {
        let ep_list: Vec<(String, String)> = endpoints
            .iter()
            .map(|ep| (ep.id.clone(), ep.name.clone()))
            .collect();
        let mut sources = vec![PermissionSource::Any, PermissionSource::System];
        for agent in agents {
            sources.push(PermissionSource::Agent(agent.id.clone()));
        }
        Self {
            endpoints: ep_list,
            selected_endpoint: preselect_endpoint.unwrap_or(0),
            sources,
            selected_source: 0,
            access: AccessLevel::AllowHil,
            field_index: 0,
            is_edit: false,
        }
    }

    pub fn from_rule(
        rule: &PermissionRule,
        endpoints: &[EndpointInfo],
        agents: &[AgentInfo],
    ) -> Self {
        let ep_index = endpoints
            .iter()
            .position(|ep| ep.id == rule.endpoint_id)
            .unwrap_or(0);
        let mut state = Self::new(endpoints, agents, Some(ep_index));
        state.access = rule.access;
        state.is_edit = true;
        // Find matching source
        for (i, src) in state.sources.iter().enumerate() {
            if *src == rule.source {
                state.selected_source = i;
                break;
            }
        }
        state
    }

    pub fn cycle_endpoint(&mut self, forward: bool) {
        if self.endpoints.is_empty() {
            return;
        }
        if forward {
            self.selected_endpoint = (self.selected_endpoint + 1) % self.endpoints.len();
        } else {
            self.selected_endpoint = if self.selected_endpoint == 0 {
                self.endpoints.len() - 1
            } else {
                self.selected_endpoint - 1
            };
        }
    }

    pub fn cycle_source(&mut self, forward: bool) {
        if self.sources.is_empty() {
            return;
        }
        if forward {
            self.selected_source = (self.selected_source + 1) % self.sources.len();
        } else {
            self.selected_source = if self.selected_source == 0 {
                self.sources.len() - 1
            } else {
                self.selected_source - 1
            };
        }
    }

    pub fn cycle_access(&mut self, forward: bool) {
        self.access = match (self.access, forward) {
            (AccessLevel::Allow, true) => AccessLevel::AllowHil,
            (AccessLevel::AllowHil, true) => AccessLevel::AllowHil2fa,
            (AccessLevel::AllowHil2fa, true) => AccessLevel::Deny,
            (AccessLevel::Deny, true) => AccessLevel::Allow,
            (AccessLevel::Allow, false) => AccessLevel::Deny,
            (AccessLevel::AllowHil, false) => AccessLevel::Allow,
            (AccessLevel::AllowHil2fa, false) => AccessLevel::AllowHil,
            (AccessLevel::Deny, false) => AccessLevel::AllowHil2fa,
        };
    }

    pub fn selected_endpoint_id(&self) -> &str {
        self.endpoints
            .get(self.selected_endpoint)
            .map(|(id, _)| id.as_str())
            .unwrap_or("")
    }

    pub fn selected_endpoint_name(&self) -> &str {
        self.endpoints
            .get(self.selected_endpoint)
            .map(|(_, name)| name.as_str())
            .unwrap_or("")
    }

    pub fn to_rule(&self, priority: usize) -> PermissionRule {
        PermissionRule {
            endpoint_id: self.selected_endpoint_id().to_string(),
            endpoint_name: self.selected_endpoint_name().to_string(),
            source: self.sources[self.selected_source].clone(),
            access: self.access,
            priority,
        }
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
/// use rockbot_tui::state::FieldDef;
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
        0 => vec![
            // Home Assistant
            FieldDef {
                id: "url",
                label: "Home Assistant URL",
                placeholder: "http://homeassistant.local:8123",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "token",
                label: "Long-Lived Access Token",
                placeholder: "eyJ0eXAi...",
                required: true,
                masked: true,
            },
        ],
        1 => vec![
            // Generic REST API
            FieldDef {
                id: "url",
                label: "Base URL",
                placeholder: "https://api.example.com",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "token",
                label: "Bearer Token",
                placeholder: "Your token",
                required: false,
                masked: true,
            },
        ],
        2 => vec![
            // OAuth2 Service
            FieldDef {
                id: "url",
                label: "API Base URL",
                placeholder: "https://api.example.com",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "auth_url",
                label: "Authorization URL",
                placeholder: "https://auth.example.com/authorize",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "token_url",
                label: "Token URL",
                placeholder: "https://auth.example.com/token",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "client_id",
                label: "Client ID",
                placeholder: "",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "client_secret",
                label: "Client Secret",
                placeholder: "",
                required: true,
                masked: true,
            },
            FieldDef {
                id: "scopes",
                label: "Scopes",
                placeholder: "read write offline_access",
                required: false,
                masked: false,
            },
            FieldDef {
                id: "redirect_uri",
                label: "Redirect URI",
                placeholder: "http://localhost:18080/oauth/callback",
                required: false,
                masked: false,
            },
        ],
        3 => vec![
            // API Key Service
            FieldDef {
                id: "url",
                label: "Base URL",
                placeholder: "https://api.example.com",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "api_key",
                label: "API Key",
                placeholder: "",
                required: true,
                masked: true,
            },
            FieldDef {
                id: "header_name",
                label: "Header Name",
                placeholder: "X-API-Key",
                required: false,
                masked: false,
            },
        ],
        4 => vec![
            // Basic Auth Service
            FieldDef {
                id: "url",
                label: "Base URL",
                placeholder: "https://api.example.com",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "username",
                label: "Username",
                placeholder: "",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "password",
                label: "Password",
                placeholder: "",
                required: true,
                masked: true,
            },
        ],
        5 => vec![
            // Bearer Token
            FieldDef {
                id: "url",
                label: "Base URL",
                placeholder: "https://api.example.com",
                required: true,
                masked: false,
            },
            FieldDef {
                id: "token",
                label: "Token",
                placeholder: "",
                required: true,
                masked: true,
            },
        ],
        _ => vec![FieldDef {
            id: "url",
            label: "URL",
            placeholder: "",
            required: true,
            masked: false,
        }],
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
/// use rockbot_tui::state::AddCredentialState;
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
            Some("aws_credentials") => 3, // API Key Service (closest match)
            Some("oauth") => 2,           // OAuth2 Service
            Some("api_key")
            | Some("api_token")
            | Some("personal_access_token")
            | Some("integration_token") => 3, // API Key Service
            Some("bot_token") | Some("long_lived_token") => 5, // Bearer Token
            _ => 3,                       // Default to API Key Service
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
            if field.required && self.field_values.get(i).is_none_or(|v| v.trim().is_empty()) {
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
                return self.field_values.get(i).map(std::string::String::as_str);
            }
        }
        None
    }
}

#[allow(deprecated)]
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
        gateway_url: String,
        tx: mpsc::UnboundedSender<Message>,
    ) -> Self {
        Self {
            menu_item: MenuItem::Dashboard,
            menu_index: 0,
            slot_bar: SlottedCardBar::new(),

            config_path,
            vault_path,
            launch_dir: std::env::current_dir().unwrap_or_default(),
            gateway_http_url: rockbot_client::ws_url_to_http(&gateway_url),
            gateway_url,

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

            butler_chat: SessionChatState::default(),
            session_chats: std::collections::HashMap::new(),
            chat_model: None,
            chat_agent_id: None,

            vault: VaultStatus::default(),
            vault_loading: true,
            endpoints: Vec::new(),
            selected_endpoint: 0,
            selected_category: 0,
            selected_provider_index: 0,
            provider_list_focus: false,
            credentials_tab: 0,
            permissions: Vec::new(),
            selected_permission: 0,

            providers: Vec::new(),
            selected_provider: 0,
            cron_jobs: Vec::new(),
            cron_loading: false,
            selected_cron_job: 0,
            selected_cron_card: 0,
            gateway_load_history: std::collections::VecDeque::new(),
            client_msg_history: std::collections::VecDeque::new(),
            selected_settings_card: 0,
            credential_schemas: Vec::new(),

            alerts: Vec::new(),

            status_message: None,
            should_exit: false,
            tick_count: 0,

            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            input_cursor: 0,

            tui_config: rockbot_core::TuiConfig::default(),

            tx,
        }
    }

    /// Process a message and update state
    /// Push an alert. Keeps most recent 100.
    pub fn push_alert(&mut self, severity: AlertSeverity, source: &str, message: String) {
        self.alerts.push(AlertItem {
            severity,
            message,
            source: source.to_string(),
            timestamp: chrono::Utc::now(),
        });
        if self.alerts.len() > 100 {
            self.alerts.remove(0);
        }
    }

    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Navigate(item) => {
                self.menu_item = item;
                self.menu_index = item.index();
                // Keep slot_bar in sync
                self.slot_bar.mode = item.index();
                self.slot_bar.slots[0].label = item.title().to_string();
                let agents = self.agents.clone();
                let sessions = self.sessions.clone();
                self.slot_bar.rebuild_content_slots(&agents, &sessions);
            }
            Message::ToggleSidebar => {
                // No-op: sidebar_focus removed, card bar always navigated via Alt+arrows
            }

            Message::GatewayStatus(status) => {
                self.gateway = status;
                self.gateway_loading = false;
                self.gateway_error = None;
            }
            Message::GatewayStatusError(err) => {
                self.gateway_loading = false;
                self.push_alert(AlertSeverity::Warning, "gateway", err.clone());
                self.gateway_error = Some(err);
            }

            Message::AgentsLoaded(agents) => {
                self.agents = agents;
                self.agents_loading = false;
                self.agents_error = None;
            }
            Message::AgentsError(err) => {
                self.agents_loading = false;
                self.push_alert(AlertSeverity::Error, "agents", err.clone());
                self.agents_error = Some(err);
            }
            Message::ReloadAgents => {
                self.agents_loading = true;
            }
            Message::AgentSaved(id) => {
                self.status_message = Some((format!("Agent '{id}' saved"), false));
            }
            Message::AgentSaveError(err) => {
                self.status_message = Some((format!("Failed to save agent: {err}"), true));
            }

            Message::SessionsLoaded(sessions) => {
                self.sessions = sessions;
                self.sessions_loading = false;
                self.sessions_error = None;
            }
            Message::SessionsError(err) => {
                self.sessions_loading = false;
                self.push_alert(AlertSeverity::Error, "sessions", err.clone());
                self.sessions_error = Some(err);
            }
            Message::ReloadSessions => {
                self.sessions_loading = true;
            }
            Message::ReloadProviders => {
                // Handled in app.rs handle_message — no state change needed
            }
            Message::SessionCreated(id) => {
                self.status_message = Some((format!("Session '{id}' created"), false));
            }
            Message::SessionCreateError(err) => {
                self.status_message = Some((format!("Failed to create session: {err}"), true));
            }

            Message::VaultStatus(status) => {
                // Debug: log vault status
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/rockbot_debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "VaultStatus received: initialized={}, locked={}, method={:?}",
                        status.initialized, status.locked, status.unlock_method
                    );
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
                self.push_alert(AlertSeverity::Error, "vault", err.clone());
                self.status_message = Some((err, true));
            }
            Message::EndpointsLoaded(endpoints) => {
                self.endpoints = endpoints;
            }
            Message::CredentialAdded(name) => {
                self.status_message = Some((format!("✅ Added: {name}"), false));
            }
            Message::CredentialAddError(err) => {
                self.status_message = Some((format!("❌ Failed: {err}"), true));
            }

            Message::ModelsLoaded(providers) => {
                self.providers = providers;
            }

            Message::CredentialSchemasLoaded(schemas) => {
                self.credential_schemas = schemas;
            }

            Message::ChatResponse(session_key, content) => {
                let chat = self.session_chats.entry(session_key).or_default();
                // If streaming already created an assistant message, finalize it
                // instead of creating a duplicate
                let has_streamed = chat
                    .messages
                    .last()
                    .is_some_and(|m| m.role == ChatRole::Assistant && chat.loading);
                if has_streamed {
                    if let Some(last) = chat.messages.last_mut() {
                        if content.len() > last.content.len() {
                            last.content = content;
                        }
                    }
                } else {
                    chat.messages.push(ChatMessage::assistant(content));
                }
                chat.loading = false;
                chat.thinking = ThinkingState::default();
            }
            Message::ChatAgentResponse(session_key, content, tool_calls) => {
                let chat = self.session_chats.entry(session_key).or_default();
                let has_streamed = chat
                    .messages
                    .last()
                    .is_some_and(|m| m.role == ChatRole::Assistant && chat.loading);
                if has_streamed {
                    if let Some(last) = chat.messages.last_mut() {
                        if content.len() > last.content.len() {
                            last.content = content;
                        }
                        last.tool_calls = tool_calls;
                    }
                } else {
                    chat.messages
                        .push(ChatMessage::assistant_with_tools(content, tool_calls));
                }
                chat.loading = false;
                chat.thinking = ThinkingState::default();
            }
            Message::ChatError(session_key, err) => {
                let chat = self.session_chats.entry(session_key).or_default();
                chat.messages
                    .push(ChatMessage::system(format!("Error: {err}")));
                chat.loading = false;
                chat.thinking = ThinkingState::default();
            }
            Message::ChatStreamChunk(chunk) => {
                // Handle streaming chunks for incremental display
                if let Some((session_key, text)) = chunk.split_once(':') {
                    let chat = self
                        .session_chats
                        .entry(session_key.to_string())
                        .or_default();
                    // Append to last assistant message, or create a new one
                    if let Some(last) = chat.messages.last_mut() {
                        if last.role == ChatRole::Assistant && chat.loading {
                            last.content.push_str(&text);
                        } else {
                            chat.messages.push(ChatMessage::assistant(text.to_string()));
                        }
                    } else {
                        chat.messages.push(ChatMessage::assistant(text.to_string()));
                    }
                }
            }
            Message::ChatTokenUsage {
                session_key,
                prompt_tokens,
                completion_tokens,
                total_tokens: _,
                cumulative_total,
            } => {
                let chat = self.session_chats.entry(session_key).or_default();
                chat.thinking.prompt_tokens = prompt_tokens;
                chat.thinking.completion_tokens = completion_tokens;
                chat.thinking.cumulative_total = cumulative_total;
                if chat.thinking.started_at.is_none() {
                    chat.thinking.started_at = Some(std::time::Instant::now());
                }
            }
            Message::ChatThinkingStatus {
                session_key,
                phase,
                tool_name,
                iteration,
            } => {
                let chat = self.session_chats.entry(session_key).or_default();
                chat.thinking.phase = phase;
                chat.thinking.tool_name = tool_name;
                chat.thinking.iteration = iteration;
                if chat.thinking.started_at.is_none() {
                    chat.thinking.started_at = Some(std::time::Instant::now());
                }
            }
            Message::SessionMessagesLoaded(session_key, messages) => {
                let chat = self.session_chats.entry(session_key).or_default();
                chat.messages = messages;
                chat.loaded = true;
            }

            Message::ContextFilesLoaded(agent_id, files) => {
                if let InputMode::ViewContextFiles(ref mut state) = self.input_mode {
                    if state.agent_id == agent_id {
                        state.files = files;
                        state.loading = false;
                    }
                }
            }
            Message::ContextFileLoaded(agent_id, filename, content) => {
                // Transition to edit mode for this file
                self.input_mode = InputMode::EditContextFile(EditContextFileState::new(
                    agent_id, filename, content,
                ));
            }
            Message::ContextFileSaved(agent_id, filename) => {
                self.status_message = Some((format!("Saved {filename}"), false));
                if let InputMode::EditContextFile(ref mut state) = self.input_mode {
                    if state.agent_id == agent_id && state.filename == filename {
                        state.is_dirty = false;
                        state.is_new_file = false;
                    }
                }
            }
            Message::ContextFileError(err) => {
                self.status_message = Some((err, true));
            }

            Message::CronJobsLoaded(jobs) => {
                self.cron_jobs = jobs;
                self.cron_loading = false;
            }
            Message::CronJobToggled(job_id, enabled) => {
                if let Some(job) = self.cron_jobs.iter_mut().find(|j| j.id == job_id) {
                    job.enabled = enabled;
                }
                let label = if enabled { "enabled" } else { "disabled" };
                self.status_message = Some((format!("Cron job {label}"), false));
            }
            Message::CronJobDeleted(job_id) => {
                self.cron_jobs.retain(|j| j.id != job_id);
                self.status_message = Some(("Cron job deleted".to_string(), false));
            }
            Message::CronJobError(err) => {
                self.cron_loading = false;
                self.push_alert(AlertSeverity::Error, "cron", err.clone());
                self.status_message = Some((err, true));
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

            Message::KeybindingsReloaded(_) => {
                // Handled in app.rs handle_message — keybindings live on App, not AppState
            }

            Message::ButlerChunk(text) => {
                // Append to last assistant message, or create a new one
                if let Some(last) = self.butler_chat.messages.last_mut() {
                    if last.role == ChatRole::Assistant {
                        last.content.push_str(&text);
                    } else {
                        self.butler_chat.messages.push(ChatMessage::assistant(text));
                    }
                } else {
                    self.butler_chat.messages.push(ChatMessage::assistant(text));
                }
                if self.butler_chat.auto_scroll {
                    self.butler_chat.scroll = usize::MAX;
                }
            }
            Message::ButlerDone(text) => {
                // If last message is streaming assistant, replace; otherwise push
                if let Some(last) = self.butler_chat.messages.last_mut() {
                    if last.role == ChatRole::Assistant && last.content.is_empty() {
                        last.content = text;
                    } else if last.role != ChatRole::Assistant {
                        self.butler_chat.messages.push(ChatMessage::assistant(text));
                    }
                    // If last is already assistant with content (from streaming), it's fine
                } else {
                    self.butler_chat.messages.push(ChatMessage::assistant(text));
                }
                self.butler_chat.loading = false;
                self.butler_chat.thinking = ThinkingState::default();
            }
            Message::ButlerError(err) => {
                self.butler_chat
                    .messages
                    .push(ChatMessage::system(format!("Butler error: {err}")));
                self.butler_chat.loading = false;
                self.butler_chat.thinking = ThinkingState::default();
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

    /// Get the currently selected session's key (ID)
    pub fn active_session_key(&self) -> Option<&str> {
        self.sessions
            .get(self.selected_session)
            .map(|s| s.key.as_str())
    }

    /// Get the chat state for the currently selected session
    pub fn active_chat(&self) -> Option<&SessionChatState> {
        self.active_session_key()
            .and_then(|key| self.session_chats.get(key))
    }

    /// Get mutable chat state for the currently selected session, creating if needed
    pub fn active_chat_mut(&mut self) -> Option<&mut SessionChatState> {
        let key = self.sessions.get(self.selected_session)?.key.clone();
        Some(self.session_chats.entry(key).or_default())
    }

    /// Convenience: chat messages for active session
    pub fn chat_messages(&self) -> &[ChatMessage] {
        self.active_chat().map_or(&[], |c| &c.messages)
    }

    /// Convenience: is chat loading for active session
    pub fn chat_loading(&self) -> bool {
        self.active_chat().map_or(false, |c| c.loading)
    }

    /// Convenience: chat scroll for active session
    pub fn chat_scroll(&self) -> usize {
        self.active_chat().map_or(0, |c| c.scroll)
    }

    /// Convenience: chat auto-scroll for active session
    pub fn chat_auto_scroll(&self) -> bool {
        self.active_chat().map_or(true, |c| c.auto_scroll)
    }

    /// Toggle expand/collapse on all tool calls in the active chat
    pub fn toggle_tool_expansion(&mut self) {
        if let Some(chat) = self.active_chat_mut() {
            // Find current state: if any are expanded, collapse all; otherwise expand all
            let any_expanded = chat
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .any(|tc| tc.expanded);
            let new_state = !any_expanded;
            for msg in &mut chat.messages {
                for tc in &mut msg.tool_calls {
                    tc.expanded = new_state;
                }
            }
        }
    }

    /// Number of credential categories (All, Model, Communication, Tool)
    pub const CREDENTIAL_CATEGORY_COUNT: usize = 4;

    /// Get provider count for current category
    pub fn provider_count_for_category(&self) -> usize {
        let schemas = &self.credential_schemas;
        match self.selected_category {
            0 => schemas.len(), // All
            1 => schemas.iter().filter(|s| s.category == "model").count(),
            2 => schemas
                .iter()
                .filter(|s| s.category == "communication")
                .count(),
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
            2 => schemas
                .iter()
                .filter(|s| s.category == "communication")
                .collect(),
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
            2 => schemas
                .iter()
                .filter(|s| s.category == "communication")
                .collect(),
            3 => schemas.iter().filter(|s| s.category == "tool").collect(),
            _ => return None,
        };

        filtered.get(idx).copied()
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
    /// Build a full HTTP API URL from a path (e.g. "/api/agents").
    pub fn api_url(&self, path: &str) -> String {
        format!("{}{path}", self.gateway_http_url)
    }

    /// Return the WebSocket URL (already normalized by `normalize_gateway_url`).
    pub fn ws_url(&self) -> String {
        self.gateway_url.clone()
    }

    /// Clear the input buffer and reset cursor
    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
        self.input_cursor = 0;
    }

    /// Move selection up in current list
    pub fn select_prev(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {
                // Dashboard left/right navigation handled via slot_bar; no-op here
            }
            MenuItem::Credentials => {
                // Left/Right navigates the tab cards
                self.credentials_tab = if self.credentials_tab == 0 {
                    3
                } else {
                    self.credentials_tab - 1
                };
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
            MenuItem::CronJobs => {
                self.selected_cron_card = if self.selected_cron_card == 0 {
                    2
                } else {
                    self.selected_cron_card - 1
                };
            }
            MenuItem::Models => {
                let count = self.model_provider_count();
                self.selected_provider = if self.selected_provider == 0 {
                    count - 1
                } else {
                    self.selected_provider - 1
                };
            }
            MenuItem::Settings => {
                self.selected_settings_card = if self.selected_settings_card == 0 {
                    2
                } else {
                    self.selected_settings_card - 1
                };
            }
        }
    }

    /// Move selection down in current list
    pub fn select_next(&mut self) {
        match self.menu_item {
            MenuItem::Dashboard => {
                // Dashboard left/right navigation handled via slot_bar; no-op here
            }
            MenuItem::Credentials => {
                // Left/Right navigates the tab cards
                self.credentials_tab = (self.credentials_tab + 1) % 4;
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
            MenuItem::CronJobs => {
                self.selected_cron_card = (self.selected_cron_card + 1) % 3;
            }
            MenuItem::Models => {
                let count = self.model_provider_count();
                self.selected_provider = (self.selected_provider + 1) % count;
            }
            MenuItem::Settings => {
                self.selected_settings_card = (self.selected_settings_card + 1) % 3;
            }
        }
    }

    /// Move selection up in credential list (Up/Down within selected tab)
    pub fn credential_list_prev(&mut self) {
        match self.credentials_tab {
            0 => {
                if !self.endpoints.is_empty() {
                    self.selected_endpoint = if self.selected_endpoint == 0 {
                        self.endpoints.len() - 1
                    } else {
                        self.selected_endpoint - 1
                    };
                }
            }
            1 => {
                let count = self.provider_count_for_category();
                if count > 0 {
                    self.selected_provider_index = if self.selected_provider_index == 0 {
                        count - 1
                    } else {
                        self.selected_provider_index - 1
                    };
                }
            }
            2 => {
                if !self.permissions.is_empty() {
                    self.selected_permission = if self.selected_permission == 0 {
                        self.permissions.len() - 1
                    } else {
                        self.selected_permission - 1
                    };
                }
            }
            _ => {}
        }
    }

    /// Move selection down in credential list
    pub fn credential_list_next(&mut self) {
        match self.credentials_tab {
            0 => {
                if !self.endpoints.is_empty() {
                    self.selected_endpoint = (self.selected_endpoint + 1) % self.endpoints.len();
                }
            }
            1 => {
                let count = self.provider_count_for_category();
                if count > 0 {
                    self.selected_provider_index = (self.selected_provider_index + 1) % count;
                }
            }
            2 => {
                if !self.permissions.is_empty() {
                    self.selected_permission =
                        (self.selected_permission + 1) % self.permissions.len();
                }
            }
            _ => {}
        }
    }

    /// Navigate to previous menu item
    pub fn menu_prev(&mut self) {
        self.menu_index = if self.menu_index == 0 {
            5
        } else {
            self.menu_index - 1
        };
        self.menu_item = MenuItem::from_index(self.menu_index);
    }

    /// Navigate to next menu item
    pub fn menu_next(&mut self) {
        self.menu_index = (self.menu_index + 1) % 6;
        self.menu_item = MenuItem::from_index(self.menu_index);
    }
}
