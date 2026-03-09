//! Main Krabbykrus TUI application with async event handling
//!
//! Uses tokio::select! for responsive concurrent event + background task handling.

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

use super::components::{
    render_add_credential_modal, render_confirm_modal, render_dashboard,
    render_agents, render_credentials, render_edit_credential_modal, render_edit_provider_modal,
    render_edit_agent_modal,
    render_models, render_password_modal, render_sessions, render_settings, render_sidebar,
    render_status_bar, render_view_session_modal,
};
use super::effects::EffectState;
use super::state::{
    AddCredentialState, AppState, ChatMessage, ConfirmAction, EditAgentState, EditCredentialState,
    EndpointInfo, InputMode, MenuItem, Message, PasswordAction, UnlockMethod,
};

/// Check if Claude Code OAuth credentials are available
pub fn has_claude_credentials() -> bool {
    krabbykrus_llm::AnthropicProvider::has_credentials()
}

/// Content tabs for views that have sub-tabs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialsTab {
    #[default]
    Endpoints,
    Providers,
    Permissions,
    Audit,
}

impl CredentialsTab {
    pub fn all() -> Vec<Self> {
        vec![Self::Endpoints, Self::Providers, Self::Permissions, Self::Audit]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Endpoints => "Endpoints",
            Self::Providers => "Providers",
            Self::Permissions => "Permissions",
            Self::Audit => "Audit Log",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Endpoints => 0,
            Self::Providers => 1,
            Self::Permissions => 2,
            Self::Audit => 3,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx % 4 {
            0 => Self::Endpoints,
            1 => Self::Providers,
            2 => Self::Permissions,
            _ => Self::Audit,
        }
    }
}

/// Main application struct
pub struct App {
    state: AppState,
    rx: mpsc::UnboundedReceiver<Message>,
    /// Effect state for visual animations
    effect_state: EffectState,
    /// Current tab within Models view (for future use)
    models_tab: usize,
    /// Unlocked vault handle (None if locked or not initialized)
    vault: Option<krabbykrus_credentials::CredentialVault>,
}

impl App {
    pub fn new(config_path: PathBuf, vault_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            state: AppState::new(config_path, vault_path, tx),
            rx,
            effect_state: EffectState::new(),
            models_tab: 0,
            vault: None,
        }
    }

    /// Get current credentials tab as enum
    fn credentials_tab(&self) -> CredentialsTab {
        CredentialsTab::from_index(self.state.credentials_tab)
    }

    /// Navigate to previous content tab (Shift+[)
    fn prev_content_tab(&mut self) {
        match self.state.menu_item {
            MenuItem::Credentials => {
                self.state.credentials_tab = if self.state.credentials_tab == 0 { 3 } else { self.state.credentials_tab - 1 };
            }
            MenuItem::Models => {
                self.models_tab = if self.models_tab == 0 { 2 } else { self.models_tab - 1 };
            }
            _ => {}
        }
    }

    /// Navigate to next content tab (Shift+])
    fn next_content_tab(&mut self) {
        match self.state.menu_item {
            MenuItem::Credentials => {
                self.state.credentials_tab = (self.state.credentials_tab + 1) % 4;
            }
            MenuItem::Models => {
                self.models_tab = (self.models_tab + 1) % 3;
            }
            _ => {}
        }
    }

    /// Initialize app state - load initial data
    pub async fn init(&mut self) -> Result<()> {
        // Spawn background tasks for initial data loading
        self.spawn_gateway_check();
        self.spawn_agents_load();
        self.spawn_vault_check();
        Ok(())
    }

    /// Spawn a task to check gateway status
    fn spawn_gateway_check(&self) {
        let tx = self.state.tx.clone();
        tokio::spawn(async move {
            match check_gateway_status().await {
                Ok(status) => {
                    let _ = tx.send(Message::GatewayStatus(status));
                }
                Err(e) => {
                    let _ = tx.send(Message::GatewayStatusError(e.to_string()));
                }
            }
        });
    }

    /// Spawn a task to load agents
    fn spawn_agents_load(&self) {
        let tx = self.state.tx.clone();
        let config_path = self.state.config_path.clone();
        tokio::spawn(async move {
            match load_agents(&config_path).await {
                Ok(agents) => {
                    let _ = tx.send(Message::AgentsLoaded(agents));
                }
                Err(e) => {
                    let _ = tx.send(Message::AgentsError(e.to_string()));
                }
            }
        });
    }

    /// Spawn a task to check vault status
    fn spawn_vault_check(&self) {
        let tx = self.state.tx.clone();
        let vault_path = self.state.vault_path.clone();
        tokio::spawn(async move {
            match check_vault_status(&vault_path).await {
                Ok(status) => {
                    let _ = tx.send(Message::VaultStatus(status));
                }
                Err(e) => {
                    let _ = tx.send(Message::VaultError(e.to_string()));
                }
            }
        });
    }

    /// Handle incoming messages from async tasks
    fn handle_message(&mut self, msg: Message) {
        // Check if this is a VaultStatus message with keyfile unlock
        // If so, auto-unlock the vault since no password is needed
        if let Message::VaultStatus(ref status) = msg {
            if status.initialized && status.locked {
                if let UnlockMethod::Keyfile { ref path } = status.unlock_method {
                    // Auto-unlock keyfile vault
                    self.auto_unlock_keyfile_vault(path.clone());
                }
            }
        }

        self.state.update(msg);
    }

    /// Auto-unlock a keyfile-protected vault (no user interaction needed)
    fn auto_unlock_keyfile_vault(&mut self, path_hint: Option<String>) {
        let keyfile_path = path_hint.or_else(|| {
            dirs::config_dir().map(|d| d.join("krabbykrus").join("vault.key").to_string_lossy().to_string())
        });

        if let Some(kf_path) = keyfile_path {
            let kf_pathbuf = std::path::PathBuf::from(&kf_path);
            if kf_pathbuf.exists() {
                match krabbykrus_credentials::CredentialVault::open(&self.state.vault_path) {
                    Ok(mut storage) => {
                        match storage.unlock_with_keyfile(&kf_pathbuf) {
                            Ok(()) => {
                                // Load endpoints after unlocking
                                let endpoints: Vec<EndpointInfo> = storage.list_endpoints()
                                    .into_iter()
                                    .map(|e| EndpointInfo {
                                        id: e.id.to_string(),
                                        name: e.name.clone(),
                                        endpoint_type: format!("{:?}", e.endpoint_type),
                                        base_url: e.base_url.clone(),
                                        has_credential: e.credential_id != uuid::Uuid::nil(),
                                        expiration: None,
                                    })
                                    .collect();

                                self.vault = Some(storage);
                                self.state.vault.locked = false;
                                self.state.vault.endpoint_count = endpoints.len();
                                self.state.endpoints = endpoints;
                                self.state.status_message = Some(("✅ Vault auto-unlocked".to_string(), false));
                            }
                            Err(e) => {
                                self.state.status_message = Some((format!("❌ Auto-unlock failed: {}", e), true));
                            }
                        }
                    }
                    Err(e) => {
                        self.state.status_message = Some((format!("❌ Failed to open vault: {}", e), true));
                    }
                }
            }
        }
    }

    /// Handle key events
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Global keybindings (always active)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('q') => {
                    self.state.should_exit = true;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Route to appropriate handler based on input mode
        match &self.state.input_mode {
            InputMode::Normal => self.handle_normal_mode(key),
            InputMode::PasswordInput { masked, action, .. } => {
                let masked = *masked;
                let action = action.clone();
                self.handle_password_input(key, masked, action)
            }
            InputMode::AddCredential(state) => {
                let state = state.clone();
                self.handle_add_credential(key, state)
            }
            InputMode::EditCredential(state) => {
                let state = state.clone();
                self.handle_edit_credential(key, state)
            }
            InputMode::EditProvider(state) => {
                let state = state.clone();
                self.handle_edit_provider(key, state)
            }
            InputMode::AddAgent(state) | InputMode::EditAgent(state) => {
                let state = state.clone();
                self.handle_edit_agent(key, state)
            }
            InputMode::Confirm { action, .. } => {
                let action = action.clone();
                self.handle_confirm(key, action)
            }
            InputMode::ChatInput => self.handle_chat_input(key),
            InputMode::ViewSession { .. } => self.handle_view_session(key),
        }
    }

    fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            // Navigation
            KeyCode::Char('q') if self.state.sidebar_focus => {
                self.state.should_exit = true;
            }
            KeyCode::Tab => {
                self.state.sidebar_focus = !self.state.sidebar_focus;
                self.effect_state.set_active(!self.state.sidebar_focus);
            }

            // Sidebar navigation (only when sidebar focused)
            KeyCode::Up | KeyCode::Char('k') if self.state.sidebar_focus => {
                self.state.menu_prev();
            }
            KeyCode::Down | KeyCode::Char('j') if self.state.sidebar_focus => {
                self.state.menu_next();
            }
            KeyCode::Enter if self.state.sidebar_focus => {
                // Enter selects and switches to content
                self.state.sidebar_focus = false;
                self.effect_state.set_active(true);
            }

            // Content navigation (only when content focused)
            KeyCode::Esc if !self.state.sidebar_focus => {
                // On Credentials Providers tab, Esc first goes back to category list
                if self.state.menu_item == MenuItem::Credentials
                   && self.state.credentials_tab == 1
                   && self.state.provider_list_focus
                {
                    self.state.provider_list_focus = false;
                } else {
                    // Esc returns to sidebar
                    self.state.sidebar_focus = true;
                    self.effect_state.set_active(false);
                }
            }
            KeyCode::Up | KeyCode::Char('k') if !self.state.sidebar_focus => {
                self.state.select_prev();
            }
            KeyCode::Down | KeyCode::Char('j') if !self.state.sidebar_focus => {
                self.state.select_next();
            }
            // Left/Right to switch between panels in Credentials Providers tab
            KeyCode::Left | KeyCode::Char('h') if !self.state.sidebar_focus => {
                if self.state.menu_item == MenuItem::Credentials && self.state.credentials_tab == 1 {
                    self.state.provider_list_focus = false;
                }
            }
            KeyCode::Right | KeyCode::Char('l') if !self.state.sidebar_focus => {
                if self.state.menu_item == MenuItem::Credentials && self.state.credentials_tab == 1 {
                    // Only allow if category has providers
                    if self.state.provider_count_for_category() > 0 {
                        self.state.provider_list_focus = true;
                    }
                }
            }
            // Enter to select/enter provider list
            KeyCode::Enter if !self.state.sidebar_focus => {
                if self.state.menu_item == MenuItem::Credentials
                   && self.state.credentials_tab == 1
                   && !self.state.provider_list_focus
                   && self.state.provider_count_for_category() > 0
                {
                    self.state.provider_list_focus = true;
                    self.state.selected_provider_index = 0;
                }
            }

            // Tab navigation within views (Shift+[ and Shift+])
            KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.prev_content_tab();
            }
            KeyCode::Char(']') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.next_content_tab();
            }
            // Also support { and } which are Shift+[ and Shift+] on US keyboards
            KeyCode::Char('{') => {
                self.prev_content_tab();
            }
            KeyCode::Char('}') => {
                self.next_content_tab();
            }

            // Quick nav by number
            KeyCode::Char('1') => self.state.menu_item = MenuItem::Dashboard,
            KeyCode::Char('2') => self.state.menu_item = MenuItem::Credentials,
            KeyCode::Char('3') => self.state.menu_item = MenuItem::Agents,
            KeyCode::Char('4') => self.state.menu_item = MenuItem::Sessions,
            KeyCode::Char('5') => self.state.menu_item = MenuItem::Models,
            KeyCode::Char('6') => self.state.menu_item = MenuItem::Settings,

            // Page-specific actions
            KeyCode::Char('a') if !self.state.sidebar_focus => {
                self.handle_add_action();
            }
            KeyCode::Char('d') if !self.state.sidebar_focus => {
                self.handle_delete_action();
            }
            KeyCode::Char('r') | KeyCode::F(5) => {
                self.handle_refresh_action();
            }
            KeyCode::Char('i') => {
                self.handle_init_action();
            }
            KeyCode::Char('u') => {
                self.handle_unlock_action();
            }
            KeyCode::Char('l') if !self.state.sidebar_focus => {
                self.handle_lock_action();
            }
            KeyCode::Char('c') if !self.state.sidebar_focus => {
                self.handle_chat_action();
            }
            KeyCode::Char('e') if !self.state.sidebar_focus => {
                self.handle_edit_action();
            }
            KeyCode::Char('k') if !self.state.sidebar_focus => {
                self.handle_kill_action();
            }
            KeyCode::Char('v') if !self.state.sidebar_focus => {
                self.handle_view_action();
            }
            KeyCode::Char('t') if !self.state.sidebar_focus => {
                self.handle_test_action();
            }
            KeyCode::Char('s') if !self.state.sidebar_focus => {
                self.handle_start_action();
            }
            KeyCode::Char('S') if !self.state.sidebar_focus => {
                self.handle_stop_action();
            }

            // Shift+Tab for backwards tab navigation
            KeyCode::BackTab => {
                self.prev_content_tab();
            }

            _ => {}
        }
        Ok(())
    }

    fn handle_add_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Agents => {
                self.state.input_mode = InputMode::AddAgent(EditAgentState::new());
            }
            MenuItem::Credentials if self.state.vault.initialized && !self.state.vault.locked => {
                // Context-aware add based on which tab and what's selected
                if self.state.credentials_tab == 1 {
                    // Providers tab - use selected provider context
                    if self.state.provider_list_focus {
                        // In provider list - use the selected provider
                        if let Some(provider_info) = self.state.get_selected_provider_info() {
                            self.state.input_mode = InputMode::AddCredential(
                                AddCredentialState::new_for_provider(&provider_info)
                            );
                            return;
                        }
                    } else {
                        // In category list - check if category has providers
                        let provider_count = self.state.provider_count_for_category();
                        if provider_count > 0 {
                            // Navigate to provider list so user can select which provider
                            self.state.provider_list_focus = true;
                            self.state.selected_provider_index = 0;
                            self.state.status_message = Some((
                                "Select a provider with ↑↓, then press 'a' to add".to_string(),
                                false
                            ));
                            return;
                        }
                        // For OAuth2/Generic categories with no predefined providers,
                        // fall through to default form
                    }
                }
                // Default: show generic add form (API Key Service is more useful than Home Assistant)
                let mut default_state = AddCredentialState::new();
                default_state.endpoint_type = 3; // API Key Service instead of Home Assistant
                default_state.reset_fields_for_type();
                self.state.input_mode = InputMode::AddCredential(default_state);
            }
            _ => {}
        }
    }

    fn handle_delete_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Credentials => {
                if let Some(endpoint) = self.state.endpoints.get(self.state.selected_endpoint) {
                    self.state.input_mode = InputMode::Confirm {
                        message: format!("Delete '{}'?", endpoint.name),
                        action: ConfirmAction::DeleteEndpoint(endpoint.id.clone()),
                    };
                }
            }
            MenuItem::Agents => {
                if let Some(agent) = self.state.agents.get(self.state.selected_agent) {
                    self.state.input_mode = InputMode::Confirm {
                        message: format!("Disable agent '{}'?", agent.id),
                        action: ConfirmAction::DeleteAgent(agent.id.clone()),
                    };
                }
            }
            _ => {}
        }
    }

    fn handle_refresh_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Settings if !self.state.sidebar_focus => {
                // On Settings tab, 'r' means restart gateway
                self.state.status_message = Some(("Restarting gateway...".to_string(), false));
                self.spawn_gateway_control("restart");
            }
            MenuItem::Agents if !self.state.sidebar_focus => {
                // On Agents tab, reload agents from config
                self.state.status_message = Some(("Reloading agents...".to_string(), false));
                self.spawn_agents_load();
            }
            _ => {
                // General refresh
                self.state.status_message = Some(("Refreshing...".to_string(), false));
                self.spawn_gateway_check();
                self.spawn_agents_load();
                self.spawn_vault_check();
            }
        }
    }

    fn handle_init_action(&mut self) {
        if !self.state.vault.initialized {
            self.state.input_mode = InputMode::PasswordInput {
                prompt: "Create vault password (min 8 chars):".to_string(),
                masked: true,
                action: PasswordAction::InitVault,
            };
            self.state.input_buffer.clear();
        }
    }

    fn handle_unlock_action(&mut self) {
        // Debug: log unlock attempt
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
            use std::io::Write;
            let _ = writeln!(f, "handle_unlock_action: initialized={}, locked={}, method={:?}",
                self.state.vault.initialized, self.state.vault.locked, self.state.vault.unlock_method);
        }

        if !self.state.vault.initialized || !self.state.vault.locked {
            return;
        }

        match &self.state.vault.unlock_method {
            UnlockMethod::Password => {
                self.state.input_mode = InputMode::PasswordInput {
                    prompt: "Enter vault password:".to_string(),
                    masked: true,
                    action: PasswordAction::UnlockVault,
                };
                self.state.input_buffer.clear();
            }
            UnlockMethod::Keyfile { path } => {
                // Debug: log keyfile unlock attempt
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
                    use std::io::Write;
                    let _ = writeln!(f, "Keyfile unlock: path={:?}", path);
                }

                // Auto-unlock with keyfile - no password needed
                let keyfile_path = path.clone().or_else(|| {
                    dirs::config_dir().map(|d| d.join("krabbykrus").join("vault.key").to_string_lossy().to_string())
                });

                if let Some(kf_path) = keyfile_path {
                    let kf_pathbuf = std::path::PathBuf::from(&kf_path);
                    if kf_pathbuf.exists() {
                        // Actually unlock with keyfile
                        match krabbykrus_credentials::CredentialVault::open(&self.state.vault_path) {
                            Ok(mut storage) => {
                                match storage.unlock_with_keyfile(&kf_pathbuf) {
                                    Ok(()) => {
                                        // Load endpoints after unlocking
                                        let endpoints: Vec<EndpointInfo> = storage.list_endpoints()
                                            .into_iter()
                                            .map(|e| EndpointInfo {
                                                id: e.id.to_string(),
                                                name: e.name.clone(),
                                                endpoint_type: format!("{:?}", e.endpoint_type),
                                                base_url: e.base_url.clone(),
                                                has_credential: e.credential_id != uuid::Uuid::nil(),
                                                expiration: None,
                                            })
                                            .collect();

                                        self.vault = Some(storage);
                                        self.state.vault.locked = false;
                                        self.state.vault.endpoint_count = endpoints.len();
                                        self.state.endpoints = endpoints;
                                        self.state.status_message = Some((format!("✅ Unlocked with keyfile"), false));
                                    }
                                    Err(e) => {
                                        self.state.status_message = Some((format!("❌ Keyfile unlock failed: {}", e), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message = Some((format!("❌ Failed to open vault: {}", e), true));
                            }
                        }
                    } else {
                        self.state.status_message = Some((format!("Keyfile not found: {}", kf_path), true));
                    }
                } else {
                    self.state.status_message = Some(("No keyfile path configured".to_string(), true));
                }
            }
            UnlockMethod::Age { public_key } => {
                let prompt = if let Some(pk) = public_key {
                    format!("Enter Age identity (pub: {}...):", &pk[..20.min(pk.len())])
                } else {
                    "Enter Age identity:".to_string()
                };
                self.state.input_mode = InputMode::PasswordInput {
                    prompt,
                    masked: false,
                    action: PasswordAction::UnlockVault,
                };
                self.state.input_buffer.clear();
            }
            UnlockMethod::SshKey { path } => {
                // Try SSH agent unlock
                let ssh_path = path.clone().unwrap_or_else(|| "~/.ssh/id_ed25519".to_string());
                // TODO: Actually unlock via SSH agent
                self.state.status_message = Some((format!("SSH unlock not yet implemented (key: {})", ssh_path), true));
            }
            UnlockMethod::Unknown => {
                // Default to password prompt
                self.state.input_mode = InputMode::PasswordInput {
                    prompt: "Enter vault password:".to_string(),
                    masked: true,
                    action: PasswordAction::UnlockVault,
                };
                self.state.input_buffer.clear();
            }
        }
    }

    fn handle_lock_action(&mut self) {
        if self.state.vault.initialized && !self.state.vault.locked {
            self.state.vault.locked = true;
            self.state.status_message = Some(("Vault locked".to_string(), false));
        }
    }

    fn handle_chat_action(&mut self) {
        // Can chat from Sessions page or anywhere with Claude Code authenticated
        match self.state.menu_item {
            MenuItem::Sessions | MenuItem::Dashboard => {
                if has_claude_credentials() {
                    self.state.input_mode = InputMode::ChatInput;
                    self.state.input_buffer.clear();
                } else {
                    self.state.status_message = Some((
                        "Run 'claude' in terminal to authenticate with Claude Code".to_string(),
                        true
                    ));
                }
            }
            _ => {
                // Navigate to Sessions and start chat
                self.state.menu_item = MenuItem::Sessions;
                if has_claude_credentials() {
                    self.state.input_mode = InputMode::ChatInput;
                    self.state.input_buffer.clear();
                }
            }
        }
    }

    fn handle_edit_action(&mut self) {
        use super::state::EditCredentialState;

        match self.state.menu_item {
            MenuItem::Credentials if self.state.vault.initialized && !self.state.vault.locked => {
                // Edit selected endpoint
                if let Some(endpoint) = self.state.endpoints.get(self.state.selected_endpoint) {
                    // Determine endpoint type from the stored string
                    let endpoint_type = match endpoint.endpoint_type.as_str() {
                        "HomeAssistant" => 0,
                        "GenericRest" => 1,
                        "GenericOAuth2" => 2,
                        _ => 3, // Default to API Key Service
                    };

                    let mut edit_state = EditCredentialState::from_endpoint(
                        &endpoint.id,
                        &endpoint.name,
                        endpoint_type,
                        &endpoint.base_url,
                        if endpoint.has_credential { Some(&endpoint.id) } else { None },
                    );

                    // Try to pre-fill secret if vault is unlocked
                    if let Some(ref vault) = self.vault {
                        if let Ok(uuid) = uuid::Uuid::parse_str(&endpoint.id) {
                            if let Ok(secret_bytes) = vault.decrypt_credential_for_endpoint(uuid) {
                                if let Ok(secret_str) = String::from_utf8(secret_bytes) {
                                    // Set the appropriate secret field based on endpoint type
                                    match endpoint_type {
                                        0 | 1 | 5 => edit_state.set_secret("token", &secret_str),
                                        3 => edit_state.set_secret("api_key", &secret_str),
                                        4 => edit_state.set_secret("password", &secret_str),
                                        2 => edit_state.set_secret("client_secret", &secret_str),
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }

                    self.state.input_mode = InputMode::EditCredential(edit_state);
                } else {
                    self.state.status_message = Some(("No endpoint selected".to_string(), true));
                }
            }
            MenuItem::Agents => {
                if let Some(agent) = self.state.agents.get(self.state.selected_agent) {
                    let edit_state = EditAgentState::from_agent(agent);
                    self.state.input_mode = InputMode::EditAgent(edit_state);
                }
            }
            MenuItem::Models => {
                // Edit model provider config
                use super::state::EditProviderState;
                let edit_state = EditProviderState::new(self.state.selected_provider);
                self.state.input_mode = InputMode::EditProvider(edit_state);
            }
            _ => {}
        }
    }

    fn handle_kill_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Sessions => {
                if let Some(session) = self.state.sessions.get(self.state.selected_session) {
                    self.state.input_mode = InputMode::Confirm {
                        message: format!("Kill session '{}'?", session.key),
                        action: ConfirmAction::KillSession(session.key.clone()),
                    };
                } else {
                    self.state.status_message = Some(("No session selected".to_string(), true));
                }
            }
            _ => {}
        }
    }

    fn handle_view_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Sessions => {
                if let Some(session) = self.state.sessions.get(self.state.selected_session) {
                    self.state.input_mode = InputMode::ViewSession {
                        session_key: session.key.clone()
                    };
                    // Spawn async task to load session details
                    self.spawn_session_details(&session.key);
                } else {
                    self.state.status_message = Some(("No session selected".to_string(), true));
                }
            }
            _ => {}
        }
    }

    fn handle_test_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Models => {
                let provider_names = ["Anthropic", "OpenAI", "Google AI", "AWS Bedrock", "Ollama"];
                if let Some(name) = provider_names.get(self.state.selected_provider) {
                    self.state.status_message = Some((format!("Testing {} connection...", name), false));
                    self.spawn_model_test(self.state.selected_provider);
                }
            }
            _ => {}
        }
    }

    fn handle_start_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Settings => {
                if self.state.gateway.connected {
                    self.state.status_message = Some(("Gateway already running".to_string(), false));
                } else {
                    self.state.status_message = Some(("Starting gateway...".to_string(), false));
                    self.spawn_gateway_control("start");
                }
            }
            _ => {}
        }
    }

    fn handle_stop_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Settings => {
                if !self.state.gateway.connected {
                    self.state.status_message = Some(("Gateway not running".to_string(), false));
                } else {
                    self.state.status_message = Some(("Stopping gateway...".to_string(), false));
                    self.spawn_gateway_control("stop");
                }
            }
            _ => {}
        }
    }

    fn spawn_gateway_control(&self, action: &str) {
        let tx = self.state.tx.clone();
        let action = action.to_string();
        tokio::spawn(async move {
            match run_gateway_control(&action).await {
                Ok(msg) => {
                    let _ = tx.send(Message::SetStatus(msg, false));
                    // Refresh gateway status after action
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Ok(status) = check_gateway_status().await {
                        let _ = tx.send(Message::GatewayStatus(status));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(format!("❌ {}", e), true));
                }
            }
        });
    }

    fn spawn_session_details(&self, _session_key: &str) {
        // TODO: Load session details from gateway API
        // For now, just show the view modal with basic info
    }

    fn spawn_model_test(&self, provider_index: usize) {
        let tx = self.state.tx.clone();

        // Get API key for the provider (Anthropic uses OAuth, not API key)
        let api_key: Option<String> = match provider_index {
            0 => None, // Anthropic uses Claude Code OAuth - test differently
            1 => self.get_provider_api_key("openai"),
            2 => self.get_provider_api_key("google"),
            // Bedrock uses AWS credentials, Ollama is local
            _ => None,
        };

        // For Anthropic, check if Claude Code credentials exist
        let has_anthropic_oauth = provider_index == 0 && has_claude_credentials();

        let provider_name = ["Anthropic", "OpenAI", "Google AI", "AWS Bedrock", "Ollama"][provider_index];

        tokio::spawn(async move {
            if provider_index == 0 {
                // Anthropic - check Claude Code OAuth credentials
                if has_anthropic_oauth {
                    let _ = tx.send(Message::SetStatus(
                        "✅ Claude Code OAuth credentials found".to_string(),
                        false
                    ));
                } else {
                    let _ = tx.send(Message::SetStatus(
                        "❌ Run 'claude' in terminal to authenticate".to_string(),
                        true
                    ));
                }
            } else if provider_index == 4 {
                // Ollama - test local connection
                match test_ollama_connection().await {
                    Ok(models) => {
                        let _ = tx.send(Message::SetStatus(
                            format!("✅ Ollama connected ({} models)", models),
                            false
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(Message::SetStatus(format!("❌ Ollama: {}", e), true));
                    }
                }
            } else if let Some(key) = api_key {
                match test_api_connection(provider_index, &key).await {
                    Ok(()) => {
                        let _ = tx.send(Message::SetStatus(
                            format!("✅ {} API key valid", provider_name),
                            false
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(Message::SetStatus(format!("❌ {}: {}", provider_name, e), true));
                    }
                }
            } else {
                let _ = tx.send(Message::SetStatus(
                    format!("❌ No API key found for {}", provider_name),
                    true
                ));
            }
        });
    }

    /// Get API key for a specific provider from vault
    fn get_provider_api_key(&self, provider_name: &str) -> Option<String> {
        let vault = self.vault.as_ref()?;

        for endpoint in vault.list_endpoints() {
            let matches = endpoint.name.to_lowercase().contains(provider_name)
                || endpoint.base_url.to_lowercase().contains(provider_name);

            if matches && endpoint.credential_id != uuid::Uuid::nil() {
                if let Ok(secret_bytes) = vault.decrypt_credential_for_endpoint(endpoint.id) {
                    if let Ok(api_key) = String::from_utf8(secret_bytes) {
                        return Some(api_key);
                    }
                }
            }
        }
        None
    }

    fn spawn_kill_session(&self, session_key: &str) {
        let tx = self.state.tx.clone();
        let key = session_key.to_string();
        tokio::spawn(async move {
            match kill_session(&key).await {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus(format!("✅ Session killed: {}", key), false));
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(format!("❌ Failed to kill session: {}", e), true));
                }
            }
        });
    }

    fn handle_password_input(&mut self, key: KeyEvent, _masked: bool, action: PasswordAction) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let password = self.state.input_buffer.clone();
                self.state.input_buffer.clear();
                self.state.input_mode = InputMode::Normal;

                if password.is_empty() {
                    self.state.status_message = Some(("Password cannot be empty".to_string(), true));
                    return Ok(());
                }

                match action {
                    PasswordAction::InitVault => {
                        if password.len() < 8 {
                            self.state.status_message = Some(("Password must be at least 8 characters".to_string(), true));
                        } else {
                            // Initialize vault with password
                            match krabbykrus_credentials::CredentialVault::init_with_password(
                                &self.state.vault_path,
                                &password,
                            ) {
                                Ok(storage) => {
                                    self.vault = Some(storage);
                                    self.state.vault.initialized = true;
                                    self.state.vault.locked = false;
                                    self.state.vault.unlock_method = UnlockMethod::Password;
                                    self.state.status_message = Some(("✅ Vault initialized!".to_string(), false));
                                }
                                Err(e) => {
                                    self.state.status_message = Some((format!("❌ Init failed: {}", e), true));
                                }
                            }
                        }
                    }
                    PasswordAction::UnlockVault => {
                        // Open and unlock vault with password
                        match krabbykrus_credentials::CredentialVault::open(&self.state.vault_path) {
                            Ok(mut storage) => {
                                match storage.unlock_with_password(&password) {
                                    Ok(()) => {
                                        // Load endpoints after unlocking
                                        let endpoints: Vec<EndpointInfo> = storage.list_endpoints()
                                            .into_iter()
                                            .map(|e| EndpointInfo {
                                                id: e.id.to_string(),
                                                name: e.name.clone(),
                                                endpoint_type: format!("{:?}", e.endpoint_type),
                                                base_url: e.base_url.clone(),
                                                has_credential: e.credential_id != uuid::Uuid::nil(),
                                                expiration: None,
                                            })
                                            .collect();

                                        self.vault = Some(storage);
                                        self.state.vault.locked = false;
                                        self.state.vault.endpoint_count = endpoints.len();
                                        self.state.endpoints = endpoints;
                                        self.state.status_message = Some(("✅ Vault unlocked".to_string(), false));
                                    }
                                    Err(e) => {
                                        self.state.status_message = Some((format!("❌ Wrong password: {}", e), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message = Some((format!("❌ Failed to open vault: {}", e), true));
                            }
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.state.input_buffer.clear();
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.state.input_buffer.pop();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_add_credential(&mut self, key: KeyEvent, mut state: AddCredentialState) -> Result<()> {
        use super::components::modals::ENDPOINT_TYPES;

        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.next_field();
                self.state.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.prev_field();
                self.state.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Enter => {
                if state.is_last_field() {
                    // Submit - validate all required fields
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                    } else {
                        // Actually add credential to vault
                        if let Some(ref mut vault) = self.vault {
                            match add_credential_to_vault(vault, &state) {
                                Ok(endpoint_name) => {
                                    // Refresh endpoints list
                                    self.state.endpoints = vault.list_endpoints()
                                        .into_iter()
                                        .map(|e| EndpointInfo {
                                            id: e.id.to_string(),
                                            name: e.name.clone(),
                                            endpoint_type: format!("{:?}", e.endpoint_type),
                                            base_url: e.base_url.clone(),
                                            has_credential: e.credential_id != uuid::Uuid::nil(),
                                            expiration: None,
                                        })
                                        .collect();
                                    self.state.vault.endpoint_count = self.state.endpoints.len();
                                    self.state.status_message = Some((format!("✅ Added: {}", endpoint_name), false));
                                    self.state.input_mode = InputMode::Normal;
                                }
                                Err(e) => {
                                    self.state.status_message = Some((format!("❌ Failed: {}", e), true));
                                }
                            }
                        } else {
                            self.state.status_message = Some(("❌ Vault not unlocked".to_string(), true));
                        }
                    }
                } else {
                    state.next_field();
                    self.state.input_mode = InputMode::AddCredential(state);
                }
            }
            KeyCode::Left if state.is_type_field() => {
                let old_type = state.endpoint_type;
                state.endpoint_type = if state.endpoint_type == 0 {
                    ENDPOINT_TYPES.len() - 1
                } else {
                    state.endpoint_type - 1
                };
                if old_type != state.endpoint_type {
                    state.reset_fields_for_type();
                }
                self.state.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Right if state.is_type_field() => {
                let old_type = state.endpoint_type;
                state.endpoint_type = (state.endpoint_type + 1) % ENDPOINT_TYPES.len();
                if old_type != state.endpoint_type {
                    state.reset_fields_for_type();
                }
                self.state.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Char(c) => {
                // Only handle text input for name and dynamic fields
                if let Some(value) = state.current_value_mut() {
                    value.push(c);
                }
                self.state.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Backspace => {
                if let Some(value) = state.current_value_mut() {
                    value.pop();
                }
                self.state.input_mode = InputMode::AddCredential(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_edit_credential(&mut self, key: KeyEvent, mut state: EditCredentialState) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.next_field();
                self.state.input_mode = InputMode::EditCredential(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.prev_field();
                self.state.input_mode = InputMode::EditCredential(state);
            }
            KeyCode::Enter => {
                if state.is_last_field() {
                    // Submit - validate all required fields
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                    } else {
                        // Update the endpoint in vault
                        if let Some(ref mut vault) = self.vault {
                            match update_credential_in_vault(vault, &state) {
                                Ok(endpoint_name) => {
                                    // Refresh endpoints list
                                    self.state.endpoints = vault.list_endpoints()
                                        .into_iter()
                                        .map(|e| EndpointInfo {
                                            id: e.id.to_string(),
                                            name: e.name.clone(),
                                            endpoint_type: format!("{:?}", e.endpoint_type),
                                            base_url: e.base_url.clone(),
                                            has_credential: e.credential_id != uuid::Uuid::nil(),
                                            expiration: None,
                                        })
                                        .collect();
                                    self.state.vault.endpoint_count = self.state.endpoints.len();
                                    self.state.status_message = Some((format!("✅ Updated: {}", endpoint_name), false));
                                    self.state.input_mode = InputMode::Normal;
                                }
                                Err(e) => {
                                    self.state.status_message = Some((format!("❌ Failed: {}", e), true));
                                }
                            }
                        } else {
                            self.state.status_message = Some(("❌ Vault not unlocked".to_string(), true));
                        }
                    }
                } else {
                    state.next_field();
                    self.state.input_mode = InputMode::EditCredential(state);
                }
            }
            KeyCode::Char(c) => {
                if let Some(value) = state.current_value_mut() {
                    value.push(c);
                }
                self.state.input_mode = InputMode::EditCredential(state);
            }
            KeyCode::Backspace => {
                if let Some(value) = state.current_value_mut() {
                    value.pop();
                }
                self.state.input_mode = InputMode::EditCredential(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_edit_provider(&mut self, key: KeyEvent, mut state: super::state::EditProviderState) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.next_field();
                self.state.input_mode = InputMode::EditProvider(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.prev_field();
                self.state.input_mode = InputMode::EditProvider(state);
            }
            KeyCode::Left | KeyCode::Right if state.is_auth_type_field() => {
                state.cycle_auth_type(key.code == KeyCode::Right);
                self.state.input_mode = InputMode::EditProvider(state);
            }
            KeyCode::Enter => {
                // Check if on last field - submit
                if state.field_index == state.total_fields() - 1 {
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                        self.state.input_mode = InputMode::EditProvider(state);
                    } else {
                        // Save provider configuration
                        self.save_provider_config(&state);
                        self.state.input_mode = InputMode::Normal;
                    }
                } else {
                    state.next_field();
                    self.state.input_mode = InputMode::EditProvider(state);
                }
            }
            KeyCode::Char(c) if !state.is_auth_type_field() => {
                if let Some(value) = state.current_value_mut() {
                    value.push(c);
                }
                self.state.input_mode = InputMode::EditProvider(state);
            }
            KeyCode::Backspace if !state.is_auth_type_field() => {
                if let Some(value) = state.current_value_mut() {
                    value.pop();
                }
                self.state.input_mode = InputMode::EditProvider(state);
            }
            _ => {
                self.state.input_mode = InputMode::EditProvider(state);
            }
        }
        Ok(())
    }

    /// Save provider configuration to config file
    fn save_provider_config(&mut self, state: &super::state::EditProviderState) {
        use super::state::ProviderAuthType;

        // Save auth mode preference to config file
        self.save_provider_auth_mode(state);

        // For session key auth, just verify Claude Code credentials exist
        if state.auth_type == ProviderAuthType::SessionKey {
            if has_claude_credentials() {
                self.state.status_message = Some((
                    format!("✅ {} configured with Claude Code OAuth", state.provider_name),
                    false
                ));
            } else {
                self.state.status_message = Some((
                    "❌ Run 'claude' in terminal to authenticate with Claude Code".to_string(),
                    true
                ));
            }
            return;
        }

        // For API key auth, store in vault
        if state.auth_type == ProviderAuthType::ApiKey && !state.api_key.is_empty() {
            if let Some(ref mut vault) = self.vault {
                // Create or update provider endpoint in vault
                // Determine base URL based on provider
                let base_url = if state.provider_index == 3 {
                    // Bedrock uses region-based URL
                    format!("bedrock.{}.amazonaws.com", state.aws_region)
                } else {
                    state.base_url.clone()
                };

                // Check if endpoint already exists
                let existing = vault.list_endpoints()
                    .into_iter()
                    .find(|e| e.name.to_lowercase() == state.provider_name.to_lowercase());

                match existing {
                    Some(endpoint) => {
                        // Update existing endpoint's credential
                        match vault.store_credential(
                            endpoint.id,
                            krabbykrus_credentials::CredentialType::BearerToken,
                            state.api_key.as_bytes(),
                        ) {
                            Ok(_) => {
                                self.state.status_message = Some((
                                    format!("✅ {} API key updated", state.provider_name),
                                    false
                                ));
                            }
                            Err(e) => {
                                self.state.status_message = Some((
                                    format!("❌ Failed to store API key: {}", e),
                                    true
                                ));
                            }
                        }
                    }
                    None => {
                        // Create new endpoint
                        match vault.create_endpoint(
                            state.provider_name.clone(),
                            krabbykrus_credentials::EndpointType::GenericRest,
                            base_url.clone(),
                        ) {
                            Ok(endpoint) => {
                                // Store the credential
                                match vault.store_credential(
                                    endpoint.id,
                                    krabbykrus_credentials::CredentialType::BearerToken,
                                    state.api_key.as_bytes(),
                                ) {
                                    Ok(_) => {
                                        // Refresh endpoints list
                                        self.state.endpoints = vault.list_endpoints()
                                            .into_iter()
                                            .map(|e| EndpointInfo {
                                                id: e.id.to_string(),
                                                name: e.name.clone(),
                                                endpoint_type: format!("{:?}", e.endpoint_type),
                                                base_url: e.base_url.clone(),
                                                has_credential: e.credential_id != uuid::Uuid::nil(),
                                                expiration: None,
                                            })
                                            .collect();
                                        self.state.vault.endpoint_count = self.state.endpoints.len();

                                        self.state.status_message = Some((
                                            format!("✅ {} configured with API key", state.provider_name),
                                            false
                                        ));
                                    }
                                    Err(e) => {
                                        self.state.status_message = Some((
                                            format!("❌ Failed to store API key: {}", e),
                                            true
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message = Some((
                                    format!("❌ Failed to create endpoint: {}", e),
                                    true
                                ));
                            }
                        }
                    }
                }
            } else {
                // Vault not unlocked - just show a message with env var hint
                let env_vars = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "", "", "GOOGLE_API_KEY"];
                if let Some(env_var) = env_vars.get(state.provider_index).filter(|v| !v.is_empty()) {
                    self.state.status_message = Some((
                        format!("💡 Set {} environment variable to persist API key", env_var),
                        false
                    ));
                }
            }
            return;
        }

        // For other auth types
        match state.auth_type {
            ProviderAuthType::None => {
                self.state.status_message = Some((
                    format!("✅ {} - no authentication needed", state.provider_name),
                    false
                ));
            }
            ProviderAuthType::AwsCredentials => {
                self.state.status_message = Some((
                    format!("💡 Set AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, and AWS_REGION={}", state.aws_region),
                    false
                ));
            }
            _ => {}
        }
    }

    fn handle_edit_agent(&mut self, key: KeyEvent, mut state: EditAgentState) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.next_field();
                self.state.input_mode = if state.is_edit {
                    InputMode::EditAgent(state)
                } else {
                    InputMode::AddAgent(state)
                };
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.prev_field();
                self.state.input_mode = if state.is_edit {
                    InputMode::EditAgent(state)
                } else {
                    InputMode::AddAgent(state)
                };
            }
            KeyCode::Enter => {
                if state.is_last_field() {
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                        self.state.input_mode = if state.is_edit {
                            InputMode::EditAgent(state)
                        } else {
                            InputMode::AddAgent(state)
                        };
                    } else {
                        // Check for duplicate ID when creating
                        if !state.is_edit && self.state.agents.iter().any(|a| a.id == state.id) {
                            self.state.status_message = Some((
                                format!("Agent '{}' already exists", state.id), true
                            ));
                            self.state.input_mode = InputMode::AddAgent(state);
                        } else {
                            self.save_agent_to_config(&state);
                            self.state.input_mode = InputMode::Normal;
                        }
                    }
                } else {
                    state.next_field();
                    self.state.input_mode = if state.is_edit {
                        InputMode::EditAgent(state)
                    } else {
                        InputMode::AddAgent(state)
                    };
                }
            }
            KeyCode::Char(c) => {
                if let Some(value) = state.current_value_mut() {
                    value.push(c);
                }
                self.state.input_mode = if state.is_edit {
                    InputMode::EditAgent(state)
                } else {
                    InputMode::AddAgent(state)
                };
            }
            KeyCode::Backspace => {
                if let Some(value) = state.current_value_mut() {
                    value.pop();
                }
                self.state.input_mode = if state.is_edit {
                    InputMode::EditAgent(state)
                } else {
                    InputMode::AddAgent(state)
                };
            }
            _ => {}
        }
        Ok(())
    }

    /// Save agent to config file (create or update)
    fn save_agent_to_config(&mut self, state: &EditAgentState) {
        let config_path = &self.state.config_path;
        let content = match std::fs::read_to_string(config_path) {
            Ok(c) => c,
            Err(e) => {
                self.state.status_message = Some((format!("Failed to read config: {}", e), true));
                return;
            }
        };

        let mut doc: toml_edit::DocumentMut = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                self.state.status_message = Some((format!("Failed to parse config: {}", e), true));
                return;
            }
        };

        // Ensure [agents] and [[agents.list]] exist
        if !doc.contains_key("agents") {
            doc["agents"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        if state.is_edit {
            // Find and update the existing agent
            if let Some(list) = doc["agents"]["list"].as_array_of_tables_mut() {
                for table in list.iter_mut() {
                    if table.get("id").and_then(|v| v.as_str()) == Some(&state.id) {
                        if !state.model.is_empty() {
                            table["model"] = toml_edit::value(&state.model);
                        } else {
                            table.remove("model");
                        }
                        if !state.parent_id.is_empty() {
                            table["parent_id"] = toml_edit::value(&state.parent_id);
                        } else {
                            table.remove("parent_id");
                        }
                        if !state.workspace.is_empty() {
                            table["workspace"] = toml_edit::value(&state.workspace);
                        } else {
                            table.remove("workspace");
                        }
                        if !state.max_tool_calls.is_empty() {
                            if let Ok(n) = state.max_tool_calls.parse::<i64>() {
                                table["max_tool_calls"] = toml_edit::value(n);
                            }
                        } else {
                            table.remove("max_tool_calls");
                        }
                        if !state.system_prompt.is_empty() {
                            table["system_prompt"] = toml_edit::value(&state.system_prompt);
                        } else {
                            table.remove("system_prompt");
                        }
                        table["enabled"] = toml_edit::value(state.enabled);
                        break;
                    }
                }
            }
        } else {
            // Create new agent entry
            let mut new_agent = toml_edit::Table::new();
            new_agent["id"] = toml_edit::value(&state.id);
            if !state.model.is_empty() {
                new_agent["model"] = toml_edit::value(&state.model);
            }
            if !state.parent_id.is_empty() {
                new_agent["parent_id"] = toml_edit::value(&state.parent_id);
            }
            if !state.workspace.is_empty() {
                new_agent["workspace"] = toml_edit::value(&state.workspace);
            }
            if !state.max_tool_calls.is_empty() {
                if let Ok(n) = state.max_tool_calls.parse::<i64>() {
                    new_agent["max_tool_calls"] = toml_edit::value(n);
                }
            }
            if !state.system_prompt.is_empty() {
                new_agent["system_prompt"] = toml_edit::value(&state.system_prompt);
            }

            if let Some(list) = doc["agents"]["list"].as_array_of_tables_mut() {
                list.push(new_agent);
            } else {
                let mut arr = toml_edit::ArrayOfTables::new();
                arr.push(new_agent);
                doc["agents"]["list"] = toml_edit::Item::ArrayOfTables(arr);
            }
        }

        // Write back
        if let Err(e) = std::fs::write(config_path, doc.to_string()) {
            self.state.status_message = Some((format!("Failed to save config: {}", e), true));
            return;
        }

        let action = if state.is_edit { "updated" } else { "created" };
        self.state.status_message = Some((format!("Agent '{}' {}", state.id, action), false));

        // Reload agents
        self.spawn_agents_load();
    }

    fn handle_view_session(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.state.input_mode = InputMode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_confirm(&mut self, key: KeyEvent, action: ConfirmAction) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                match action {
                    ConfirmAction::DeleteEndpoint(id) => {
                        // Parse UUID and delete from vault
                        if let Some(ref mut vault) = self.vault {
                            match uuid::Uuid::parse_str(&id) {
                                Ok(uuid) => {
                                    match vault.delete_endpoint(uuid) {
                                        Ok(()) => {
                                            // Refresh endpoints list
                                            self.state.endpoints = vault.list_endpoints()
                                                .into_iter()
                                                .map(|e| EndpointInfo {
                                                    id: e.id.to_string(),
                                                    name: e.name.clone(),
                                                    endpoint_type: format!("{:?}", e.endpoint_type),
                                                    base_url: e.base_url.clone(),
                                                    has_credential: e.credential_id != uuid::Uuid::nil(),
                                                    expiration: None,
                                                })
                                                .collect();
                                            self.state.vault.endpoint_count = self.state.endpoints.len();
                                            // Reset selection if needed
                                            if self.state.selected_endpoint >= self.state.endpoints.len() {
                                                self.state.selected_endpoint = self.state.endpoints.len().saturating_sub(1);
                                            }
                                            self.state.status_message = Some((format!("✅ Deleted endpoint"), false));
                                        }
                                        Err(e) => {
                                            self.state.status_message = Some((format!("❌ Delete failed: {}", e), true));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.state.status_message = Some((format!("❌ Invalid endpoint ID: {}", e), true));
                                }
                            }
                        } else {
                            self.state.status_message = Some(("❌ Vault not unlocked".to_string(), true));
                        }
                    }
                    ConfirmAction::DeleteAgent(id) => {
                        // Remove from display list (doesn't actually disable in config yet)
                        self.state.agents.retain(|a| a.id != id);
                        if self.state.selected_agent >= self.state.agents.len() {
                            self.state.selected_agent = self.state.agents.len().saturating_sub(1);
                        }
                        self.state.status_message = Some((format!("Disabled agent: {} (edit config to persist)", id), false));
                    }
                    ConfirmAction::KillSession(key) => {
                        // Spawn async task to kill session via gateway API
                        self.spawn_kill_session(&key);
                    }
                    ConfirmAction::DisableAgent(id) => {
                        // Same as DeleteAgent for now - mark as disabled
                        self.state.agents.retain(|a| a.id != id);
                        if self.state.selected_agent >= self.state.agents.len() {
                            self.state.selected_agent = self.state.agents.len().saturating_sub(1);
                        }
                        self.state.status_message = Some((format!("Disabled agent: {}", id), false));
                    }
                }
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_chat_input(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                let message = self.state.input_buffer.trim().to_string();
                if !message.is_empty() {
                    // Add user message to chat history
                    self.state.chat_messages.push(ChatMessage::user(message.clone()));
                    self.state.chat_loading = true;

                    // Check for Claude Code OAuth credentials
                    if has_claude_credentials() {
                        self.spawn_chat_request(message);
                    } else {
                        self.state.chat_messages.push(ChatMessage::system(
                            "Claude Code not authenticated. Run 'claude' in terminal to set up OAuth.".to_string()
                        ));
                        self.state.chat_loading = false;
                    }
                }
                self.state.input_buffer.clear();
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.state.input_buffer.pop();
            }
            _ => {}
        }
        Ok(())
    }

    /// Save provider auth mode preference to config file
    fn save_provider_auth_mode(&mut self, state: &super::state::EditProviderState) {
        use super::state::ProviderAuthType;

        // Determine the auth mode string
        let auth_mode = match state.auth_type {
            ProviderAuthType::ApiKey => "api",
            ProviderAuthType::SessionKey => "oauth",
            ProviderAuthType::None => "none",
            ProviderAuthType::AwsCredentials => "aws",
        };

        // Determine provider section name
        let provider_section = match state.provider_index {
            0 => "anthropic",
            1 => "openai",
            2 => "ollama",
            3 => "bedrock",
            4 => "google",
            _ => return, // Unknown provider, skip
        };

        // Read existing config
        let config_path = &self.state.config_path;
        let content = match std::fs::read_to_string(config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to read config for provider auth mode update: {}", e);
                return;
            }
        };

        // Parse as TOML value for manipulation
        let mut doc: toml_edit::DocumentMut = match content.parse() {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("Failed to parse config as TOML: {}", e);
                return;
            }
        };

        // Ensure [providers] section exists
        if !doc.contains_key("providers") {
            doc["providers"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        // Ensure [providers.<provider>] section exists
        if !doc["providers"].as_table().map(|t| t.contains_key(provider_section)).unwrap_or(false) {
            doc["providers"][provider_section] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        // Set the auth_mode
        doc["providers"][provider_section]["auth_mode"] = toml_edit::value(auth_mode);

        // Also save base_url if provided and not default
        if !state.base_url.is_empty() {
            let default_url = match state.provider_index {
                0 => "https://api.anthropic.com",
                1 => "https://api.openai.com",
                2 => "http://localhost:11434",
                _ => "",
            };
            if state.base_url != default_url {
                doc["providers"][provider_section]["api_url"] = toml_edit::value(&state.base_url);
            }
        }

        // Write back to file
        if let Err(e) = std::fs::write(config_path, doc.to_string()) {
            tracing::warn!("Failed to save provider config: {}", e);
            self.state.status_message = Some((
                format!("⚠️ Auth mode set but failed to save config: {}", e),
                true
            ));
        } else {
            tracing::info!("Saved {} auth mode: {}", provider_section, auth_mode);
        }
    }

    /// Spawn an async task to send a chat message via Claude Code SDK
    fn spawn_chat_request(&self, user_message: String) {
        let tx = self.state.tx.clone();
        let chat_history: Vec<(bool, String)> = self.state.chat_messages
            .iter()
            .filter_map(|m| match m.role {
                super::state::ChatRole::User => Some((true, m.content.clone())),
                super::state::ChatRole::Assistant => Some((false, m.content.clone())),
                super::state::ChatRole::System => None,
            })
            .collect();

        tokio::spawn(async move {
            match send_chat_message(&chat_history, &user_message).await {
                Ok(response) => {
                    let _ = tx.send(Message::ChatResponse(response));
                }
                Err(e) => {
                    let _ = tx.send(Message::ChatError(e.to_string()));
                }
            }
        });
    }

    /// Render the entire UI
    fn render(&mut self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(0)])
            .split(frame.area());

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(2)])
            .split(chunks[1]);

        // Sidebar
        render_sidebar(frame, chunks[0], &self.state, &self.effect_state);

        // Content area - pass effect state for active border animation
        match self.state.menu_item {
            MenuItem::Dashboard => render_dashboard(frame, main_chunks[0], &self.state),
            MenuItem::Credentials => render_credentials(frame, main_chunks[0], &self.state, self.state.credentials_tab, &self.effect_state),
            MenuItem::Agents => render_agents(frame, main_chunks[0], &self.state, &self.effect_state),
            MenuItem::Sessions => render_sessions(frame, main_chunks[0], &self.state, &self.effect_state),
            MenuItem::Models => render_models(frame, main_chunks[0], &self.state, &self.effect_state),
            MenuItem::Settings => render_settings(frame, main_chunks[0], &self.state, &self.effect_state),
        }

        // Status bar
        let help_text = self.get_help_text();
        render_status_bar(frame, main_chunks[1], self.state.status_message.as_ref(), &help_text);

        // Render modals on top
        self.render_modals(frame);
    }

    fn render_modals(&self, frame: &mut Frame) {
        match &self.state.input_mode {
            InputMode::PasswordInput { prompt, masked, .. } => {
                render_password_modal(
                    frame,
                    frame.area(),
                    prompt,
                    *masked,
                    &self.state.input_buffer,
                );
            }
            InputMode::AddCredential(state) => {
                render_add_credential_modal(frame, frame.area(), state);
            }
            InputMode::EditCredential(state) => {
                render_edit_credential_modal(frame, frame.area(), state);
            }
            InputMode::EditProvider(state) => {
                render_edit_provider_modal(frame, frame.area(), state);
            }
            InputMode::AddAgent(state) | InputMode::EditAgent(state) => {
                render_edit_agent_modal(frame, frame.area(), state, &self.state.agents);
            }
            InputMode::Confirm { message, .. } => {
                render_confirm_modal(frame, frame.area(), message);
            }
            InputMode::ViewSession { session_key } => {
                render_view_session_modal(frame, frame.area(), session_key, &self.state.sessions);
            }
            _ => {}
        }
    }

    fn get_help_text(&self) -> String {
        match &self.state.input_mode {
            InputMode::Normal => {
                if self.state.sidebar_focus {
                    "q:Quit │ ↑↓/jk:Navigate │ Enter:Select │ Tab:→Content │ 1-6:Quick".to_string()
                } else {
                    match self.state.menu_item {
                        MenuItem::Dashboard => {
                            "r:Refresh │ Esc/Tab:←Sidebar │ 1-6:Quick nav".to_string()
                        }
                        MenuItem::Credentials => {
                            format!(
                                "a:Add │ e:Edit │ d:Delete │ u:Unlock │ l:Lock │ {{}}:Tabs ({}) │ Esc:←",
                                self.credentials_tab().label()
                            )
                        }
                        MenuItem::Agents => {
                            "a:Add │ e:Edit │ d:Disable │ r:Reload │ Esc/Tab:←Sidebar".to_string()
                        }
                        MenuItem::Sessions => {
                            "c:Chat │ k:Kill │ v:View │ Esc/Tab:←Sidebar".to_string()
                        }
                        MenuItem::Models => {
                            "e:Edit │ t:Test │ Esc/Tab:←Sidebar".to_string()
                        }
                        MenuItem::Settings => {
                            "s:Start │ S:Stop │ r:Restart │ Esc/Tab:←Sidebar".to_string()
                        }
                    }
                }
            }
            InputMode::PasswordInput { .. } => "Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::AddCredential(_) => "↑↓/Tab:Navigate │ ←→:Type │ Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::Confirm { .. } => "y:Yes │ n:No │ Esc:Cancel".to_string(),
            InputMode::ChatInput => "Enter:Send │ Esc:Close".to_string(),
            InputMode::EditCredential(_) => "↑↓/Tab:Navigate │ Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::EditProvider(_) => "↑↓/Tab:Navigate │ ←→:Auth Type │ Enter:Save │ Esc:Cancel".to_string(),
            InputMode::AddAgent(_) | InputMode::EditAgent(_) => "↑↓/Tab:Navigate │ Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::ViewSession { .. } => "Esc/Enter:Close".to_string(),
        }
    }
}

/// Add a credential to the vault based on form state (standalone to avoid borrow issues)
fn add_credential_to_vault(
    vault: &mut krabbykrus_credentials::CredentialVault,
    state: &AddCredentialState,
) -> Result<String> {
    use krabbykrus_credentials::{EndpointType, CredentialType};

    // Map TUI endpoint type to core types
    let (endpoint_type, credential_type, secret_data) = match state.endpoint_type {
        0 => {
            // Home Assistant - token field
            let token = state.get_field_value("token").unwrap_or("");
            (
                EndpointType::HomeAssistant,
                CredentialType::BearerToken,
                token.as_bytes().to_vec(),
            )
        }
        1 | 5 => {
            // Generic REST / Bearer Token
            let token = state.get_field_value("token").unwrap_or("");
            (
                EndpointType::GenericRest,
                CredentialType::BearerToken,
                token.as_bytes().to_vec(),
            )
        }
        2 => {
            // OAuth2 Service
            let client_id = state.get_field_value("client_id").unwrap_or("").to_string();
            let client_secret = state.get_field_value("client_secret").unwrap_or("");
            let token_url = state.get_field_value("token_url").unwrap_or("").to_string();
            let scopes = state.get_field_value("scopes").unwrap_or("").to_string();

            (
                EndpointType::GenericOAuth2,
                CredentialType::OAuth2 {
                    client_id,
                    token_url,
                    scopes: scopes.split_whitespace().map(String::from).collect(),
                },
                client_secret.as_bytes().to_vec(),
            )
        }
        3 => {
            // API Key Service
            let api_key = state.get_field_value("api_key").unwrap_or("");
            let header_name = state.get_field_value("header_name")
                .unwrap_or("X-API-Key")
                .to_string();

            (
                EndpointType::GenericRest,
                CredentialType::ApiKey { header_name },
                api_key.as_bytes().to_vec(),
            )
        }
        4 => {
            // Basic Auth
            let username = state.get_field_value("username").unwrap_or("").to_string();
            let password = state.get_field_value("password").unwrap_or("");

            (
                EndpointType::GenericRest,
                CredentialType::BasicAuth { username },
                password.as_bytes().to_vec(),
            )
        }
        _ => {
            return Err(anyhow::anyhow!("Unknown endpoint type"));
        }
    };

    // Get URL from first field (all types have URL as first dynamic field)
    let base_url = state.get_field_value("url")
        .unwrap_or("")
        .to_string();

    // Create endpoint
    let endpoint = vault.create_endpoint(
        state.name.clone(),
        endpoint_type,
        base_url,
    )?;

    // Store credential
    vault.store_credential(
        endpoint.id,
        credential_type,
        &secret_data,
    )?;

    Ok(state.name.clone())
}

/// Update a credential in the vault based on edit form state
fn update_credential_in_vault(
    vault: &mut krabbykrus_credentials::CredentialVault,
    state: &EditCredentialState,
) -> Result<String> {
    use krabbykrus_credentials::{EndpointType, CredentialType};

    let endpoint_id = uuid::Uuid::parse_str(&state.endpoint_id)?;

    // Get the existing endpoint
    let mut endpoint = vault.get_endpoint(endpoint_id)?.clone();

    // Update endpoint metadata
    endpoint.name = state.name.clone();
    endpoint.base_url = state.get_field_value("url")
        .unwrap_or(&state.base_url)
        .to_string();
    endpoint.updated_at = chrono::Utc::now();

    vault.update_endpoint(endpoint.clone())?;

    // If secret was modified, rotate the credential
    if state.secret_modified && endpoint.credential_id != uuid::Uuid::nil() {
        let secret_data = match state.endpoint_type {
            0 | 1 | 5 => {
                // Home Assistant / Generic REST / Bearer Token
                state.get_field_value("token").unwrap_or("").as_bytes().to_vec()
            }
            2 => {
                // OAuth2 Service
                state.get_field_value("client_secret").unwrap_or("").as_bytes().to_vec()
            }
            3 => {
                // API Key Service
                state.get_field_value("api_key").unwrap_or("").as_bytes().to_vec()
            }
            4 => {
                // Basic Auth
                state.get_field_value("password").unwrap_or("").as_bytes().to_vec()
            }
            _ => vec![],
        };

        if !secret_data.is_empty() {
            vault.rotate_credential(endpoint.credential_id, &secret_data)?;
        }
    }

    Ok(state.name.clone())
}

/// Run the main async TUI event loop
pub async fn run_app(config_path: PathBuf, vault_path: PathBuf) -> Result<()> {
    use crossterm::{
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create and initialize app
    let mut app = App::new(config_path, vault_path);
    app.init().await?;

    // Tick interval for animations and periodic updates
    let mut tick_interval = tokio::time::interval(Duration::from_millis(100));

    // Periodic refresh interval
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(15));

    // Main async event loop
    loop {
        // Render
        terminal.draw(|frame| {
            app.render(frame);
        })?;

        // Async select on multiple event sources
        tokio::select! {
            // Terminal events (non-blocking poll)
            _ = async {
                tokio::task::yield_now().await;
            } => {
                // Poll for terminal events with a short timeout
                if event::poll(Duration::from_millis(10))? {
                    if let CrosstermEvent::Key(key) = event::read()? {
                        app.handle_key(key)?;
                    }
                }
            }

            // Messages from background tasks
            msg = app.rx.recv() => {
                if let Some(msg) = msg {
                    app.handle_message(msg);
                }
            }

            // Tick for animations
            _ = tick_interval.tick() => {
                app.state.tick_count = app.state.tick_count.wrapping_add(1);
            }

            // Periodic refresh
            _ = refresh_interval.tick() => {
                if !app.state.gateway_loading {
                    app.spawn_gateway_check();
                }
            }
        }

        if app.state.should_exit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

// =============================================================================
// Background task implementations
// =============================================================================

use super::state::{AgentInfo, AgentStatus, GatewayStatus, VaultStatus};

async fn check_gateway_status() -> Result<GatewayStatus> {
    use tokio::time::timeout;

    // Try to fetch actual status from the gateway API
    let client = reqwest::Client::new();
    let status_result = timeout(
        Duration::from_millis(500),
        client.get("http://127.0.0.1:18080/api/status").send()
    ).await;

    match status_result {
        Ok(Ok(response)) if response.status().is_success() => {
            // Parse the JSON response
            if let Ok(json) = response.json::<serde_json::Value>().await {
                return Ok(GatewayStatus {
                    connected: true,
                    version: json.get("version").and_then(|v| v.as_str()).map(String::from),
                    uptime_secs: json.get("uptime_secs").and_then(|v| v.as_u64()),
                    active_sessions: json.get("active_sessions").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    pending_agents: json.get("pending_agents").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                });
            }
            // Connected but couldn't parse response
            Ok(GatewayStatus {
                connected: true,
                version: Some("unknown".to_string()),
                uptime_secs: None,
                active_sessions: 0,
                pending_agents: 0,
            })
        }
        Ok(Ok(_)) | Ok(Err(_)) | Err(_) => {
            // Not connected or error
            Ok(GatewayStatus {
                connected: false,
                version: None,
                uptime_secs: None,
                active_sessions: 0,
                pending_agents: 0,
            })
        }
    }
}

async fn load_agents(config_path: &PathBuf) -> Result<Vec<AgentInfo>> {
    let content = tokio::fs::read_to_string(config_path).await?;

    // Parse as TOML value for reliable field extraction
    let doc: toml::Value = content.parse().unwrap_or(toml::Value::Table(toml::map::Map::new()));

    let mut agents = Vec::new();

    if let Some(agents_table) = doc.get("agents") {
        if let Some(list) = agents_table.get("list").and_then(|v| v.as_array()) {
            for entry in list {
                let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if id.is_empty() { continue; }

                let model = entry.get("model").and_then(|v| v.as_str()).map(String::from);
                let parent_id = entry.get("parent_id").and_then(|v| v.as_str()).map(String::from);
                let system_prompt = entry.get("system_prompt").and_then(|v| v.as_str()).map(String::from);
                let workspace = entry.get("workspace").and_then(|v| v.as_str()).map(String::from);
                let max_tool_calls = entry.get("max_tool_calls").and_then(|v| v.as_integer()).map(|n| n as u32);
                let enabled = entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

                let status = if enabled { AgentStatus::Active } else { AgentStatus::Disabled };

                agents.push(AgentInfo {
                    id,
                    model,
                    status,
                    session_count: 0,
                    parent_id,
                    system_prompt,
                    workspace,
                    max_tool_calls,
                    enabled,
                });
            }
        }
    }

    // If no agents found, add a default
    if agents.is_empty() {
        agents.push(AgentInfo {
            id: "default".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            status: AgentStatus::Active,
            session_count: 0,
            parent_id: None,
            system_prompt: None,
            workspace: None,
            max_tool_calls: Some(10),
            enabled: true,
        });
    }

    Ok(agents)
}

async fn check_vault_status(vault_path: &PathBuf) -> Result<VaultStatus> {
    use krabbykrus_credentials::CredentialVault;

    // Debug logging
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: path={:?}", vault_path);
    }

    let exists = CredentialVault::exists(vault_path);

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: exists={}", exists);
    }

    if !exists {
        return Ok(VaultStatus {
            enabled: true,
            initialized: false,
            locked: false,
            endpoint_count: 0,
            unlock_method: UnlockMethod::Unknown,
        });
    }

    // Try to read the vault metadata to determine unlock method
    let unlock_method = match CredentialVault::open(vault_path) {
        Ok(vault) => {
            let method = vault.unlock_method();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: raw unlock_method={:?}", method);
            }
            match method {
                Some(krabbykrus_credentials::UnlockMethod::Password { .. }) => {
                    UnlockMethod::Password
                }
                Some(krabbykrus_credentials::UnlockMethod::Keyfile { path_hint }) => {
                    UnlockMethod::Keyfile { path: path_hint.clone() }
                }
                Some(krabbykrus_credentials::UnlockMethod::Age { public_key, .. }) => {
                    UnlockMethod::Age { public_key: Some(public_key.clone()) }
                }
                Some(krabbykrus_credentials::UnlockMethod::SshKey { public_key_path, .. }) => {
                    UnlockMethod::SshKey { path: Some(public_key_path.clone()) }
                }
                None => UnlockMethod::Unknown,
            }
        }
        Err(e) => {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: open error={:?}", e);
            }
            UnlockMethod::Unknown
        }
    };

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/krabbykrus_debug.log") {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: final unlock_method={:?}", unlock_method);
    }

    Ok(VaultStatus {
        enabled: true,
        initialized: true,
        locked: true, // Assume locked until unlocked
        endpoint_count: 0,
        unlock_method,
    })
}

/// Send a chat message via Claude Code SDK (OAuth)
async fn send_chat_message(
    chat_history: &[(bool, String)], // (is_user, content)
    user_message: &str,
) -> Result<String> {
    use krabbykrus_llm::{AnthropicProvider, LlmProvider, ChatCompletionRequest, Message, MessageRole};

    // Create provider using Claude Code OAuth
    let provider = AnthropicProvider::new()
        .map_err(|e| anyhow::anyhow!("Failed to create Anthropic provider: {}", e))?;

    // Build messages from history
    let mut messages: Vec<Message> = chat_history
        .iter()
        .map(|(is_user, content)| Message {
            role: if *is_user { MessageRole::User } else { MessageRole::Assistant },
            content: content.clone(),
            tool_calls: None,
        })
        .collect();

    // Add the current user message
    messages.push(Message {
        role: MessageRole::User,
        content: user_message.to_string(),
        tool_calls: None,
    });

    let request = ChatCompletionRequest {
        model: "claude-sonnet-4-20250514".to_string(),
        messages,
        temperature: Some(0.7),
        max_tokens: Some(4096),
        tools: None,
        stream: false,
    };

    let response = provider.chat_completion(request).await
        .map_err(|e| anyhow::anyhow!("Chat completion failed: {}", e))?;

    // Extract the assistant's response
    let content = response.choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_else(|| "No response received".to_string());

    Ok(content)
}

/// Run gateway control command (start/stop/restart)
async fn run_gateway_control(action: &str) -> Result<String> {
    use tokio::process::Command;

    let output = Command::new("openclaw")
        .args(["gateway", action])
        .output()
        .await?;

    if output.status.success() {
        let msg = match action {
            "start" => "✅ Gateway started",
            "stop" => "✅ Gateway stopped",
            "restart" => "✅ Gateway restarted",
            _ => "✅ Gateway command completed",
        };
        Ok(msg.to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow::anyhow!("Gateway {} failed: {}", action, stderr))
    }
}

/// Test Ollama local connection
async fn test_ollama_connection() -> Result<usize> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await?;
        let model_count = json
            .get("models")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        Ok(model_count)
    } else {
        Err(anyhow::anyhow!("Ollama returned status {}", response.status()))
    }
}

/// Test API connection for various providers
async fn test_api_connection(provider_index: usize, api_key: &str) -> Result<()> {
    let client = reqwest::Client::new();

    match provider_index {
        0 => {
            // Anthropic - test with a minimal request
            // Detect auth type: session key starts with "sk-ant-oat" (OAuth token)
            let is_session_key = api_key.starts_with("sk-ant-oat");

            let mut request = client
                .post("https://api.anthropic.com/v1/messages")
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json");

            // Use appropriate auth header
            request = if is_session_key {
                request.header("Authorization", format!("Bearer {}", api_key))
            } else {
                request.header("x-api-key", api_key)
            };

            let response = request
                .json(&serde_json::json!({
                    "model": "claude-3-5-haiku-latest",
                    "max_tokens": 1,
                    "messages": [{"role": "user", "content": "Hi"}]
                }))
                .timeout(Duration::from_secs(10))
                .send()
                .await?;

            if response.status().is_success() || response.status().as_u16() == 400 {
                // 400 can mean invalid request format but valid API key
                Ok(())
            } else if response.status().as_u16() == 401 {
                Err(anyhow::anyhow!("Invalid API key or session token"))
            } else {
                Err(anyhow::anyhow!("API returned status {}", response.status()))
            }
        }
        1 => {
            // OpenAI
            let response = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {}", api_key))
                .timeout(Duration::from_secs(10))
                .send()
                .await?;

            if response.status().is_success() {
                Ok(())
            } else if response.status().as_u16() == 401 {
                Err(anyhow::anyhow!("Invalid API key"))
            } else {
                Err(anyhow::anyhow!("API returned status {}", response.status()))
            }
        }
        2 => {
            // Google AI
            let response = client
                .get(format!(
                    "https://generativelanguage.googleapis.com/v1/models?key={}",
                    api_key
                ))
                .timeout(Duration::from_secs(10))
                .send()
                .await?;

            if response.status().is_success() {
                Ok(())
            } else if response.status().as_u16() == 400 || response.status().as_u16() == 403 {
                Err(anyhow::anyhow!("Invalid API key"))
            } else {
                Err(anyhow::anyhow!("API returned status {}", response.status()))
            }
        }
        _ => Err(anyhow::anyhow!("Unknown provider")),
    }
}

/// Kill a session via gateway API
async fn kill_session(session_key: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .delete(format!("http://127.0.0.1:18080/api/sessions/{}", session_key))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if response.status().is_success() || response.status().as_u16() == 404 {
        // 404 means session already gone, which is fine
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to kill session: {}", response.status()))
    }
}
