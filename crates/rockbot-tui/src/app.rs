//! Main RockBot TUI application with async event handling
//!
//! Uses tokio::select! for responsive concurrent event + background task handling.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use rockbot_core::{AnimationStyle, ColorTheme};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::components::{
    render_add_credential_modal, render_confirm_modal, render_edit_agent_modal,
    render_edit_context_file_modal, render_edit_credential_modal, render_edit_permission_modal,
    render_edit_provider_modal, render_password_modal, render_slot_bar, render_status_bar,
    render_view_context_files_modal, render_view_endpoint_modal, render_view_model_list_modal,
    render_view_permission_modal, render_view_provider_modal, render_view_session_modal,
};
use crate::effects::EffectState;
use crate::state::{
    AddCredentialState, AppState, ChatMessage, ChatTarget, ConfirmAction, ContextFileInfo,
    ContextMenuAction, ContextMenuState, CreateSessionState, EditAgentState, EditCredentialState,
    EditProviderState, EndpointInfo, FontRole, InputMode, MenuItem, Message, PasswordAction,
    SessionMode, ThemeToken, ToolCallInfo, UnlockMethod, ViewContextFilesState,
};

/// Check if Claude Code OAuth credentials are available
pub fn has_claude_credentials() -> bool {
    #[cfg(feature = "anthropic")]
    {
        rockbot_llm::AnthropicProvider::has_credentials()
    }
    #[cfg(not(feature = "anthropic"))]
    {
        false
    }
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
        vec![
            Self::Endpoints,
            Self::Providers,
            Self::Permissions,
            Self::Audit,
        ]
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
    /// Timestamp of last rendered frame (for per-frame elapsed duration)
    last_frame: Instant,
    /// Whether a modal was open on the previous frame (for transition detection)
    was_modal_open: bool,
    /// Previous menu_item index (for page transition detection)
    prev_menu_index: usize,
    /// Unlocked vault handle (None if locked or not initialized)
    vault: Option<rockbot_credentials::CredentialVault>,
    /// Gateway WebSocket client
    gateway_client: Option<rockbot_client::GatewayClient>,
    /// Receiver for gateway events
    gateway_events_rx: Option<tokio::sync::broadcast::Receiver<rockbot_client::GatewayEvent>>,
    /// Timestamp of the most recent WS ping, used for RTT sampling.
    last_ws_ping_sent_at: Option<Instant>,
    /// Local tool registry for remote tool execution (TUI executes tools on behalf of gateway)
    #[cfg(feature = "remote-exec")]
    #[allow(dead_code)]
    local_tool_registry: Option<std::sync::Arc<rockbot_tools::ToolRegistry>>,
    /// Keybinding configuration (data-driven, replaces hardcoded key matching)
    keybindings: super::keybindings::KeybindingConfig,
    /// Butler companion agent (local GGUF model)
    #[cfg(feature = "butler")]
    butler: Option<rockbot_butler::Butler>,
    /// Butler conversation session
    #[cfg(feature = "butler")]
    butler_session: Option<rockbot_butler::ButlerSession>,
    /// Chat command registry for slash command dispatch
    command_registry: rockbot_chat::ChatCommandRegistry,
}

/// Build the unified command registry from all crates.
fn build_command_registry() -> rockbot_chat::ChatCommandRegistry {
    let mut reg = rockbot_chat::ChatCommandRegistry::new();
    // TUI-local commands: /exit, /help, /clear, /mode, /alerts
    crate::chat_commands::register_chat_commands(&mut reg);
    // Per-crate commands
    rockbot_credentials::chat_commands::register_chat_commands(&mut reg);
    // rockbot-core re-exports its own cron commands
    rockbot_core::chat_commands::register_chat_commands(&mut reg);
    rockbot_editor::register_chat_commands(&mut reg);
    rockbot_shell::register_chat_commands(&mut reg);
    #[cfg(feature = "doctor-ai")]
    rockbot_doctor::chat_commands::register_chat_commands(&mut reg);
    #[cfg(feature = "butler")]
    rockbot_butler::chat_commands::register_chat_commands(&mut reg);
    reg
}

impl App {
    pub fn new(config_path: PathBuf, vault_path: PathBuf, gateway_url: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            state: AppState::new(config_path, vault_path, gateway_url, tx),
            rx,
            effect_state: EffectState::new(),
            last_frame: Instant::now(),
            was_modal_open: false,
            prev_menu_index: 0,
            vault: None,
            gateway_client: None,
            gateway_events_rx: None,
            last_ws_ping_sent_at: None,
            #[cfg(feature = "remote-exec")]
            local_tool_registry: None,
            keybindings: super::keybindings::KeybindingConfig::default(),
            #[cfg(feature = "butler")]
            butler: None,
            #[cfg(feature = "butler")]
            butler_session: None,
            command_registry: build_command_registry(),
        }
    }

    /// Navigate to previous content tab (Shift+[)
    fn prev_content_tab(&mut self) {
        // Tab cycling is now handled within overlay modes
        if matches!(self.state.input_mode, InputMode::VaultOverlay) {
            self.state.credentials_tab = if self.state.credentials_tab == 0 {
                3
            } else {
                self.state.credentials_tab - 1
            };
        }
    }

    /// Navigate to next content tab (Shift+])
    fn next_content_tab(&mut self) {
        if matches!(self.state.input_mode, InputMode::VaultOverlay) {
            self.state.credentials_tab = (self.state.credentials_tab + 1) % 4;
        }
    }

    /// Initialize app state - load initial data
    pub async fn init(&mut self) -> Result<()> {
        // Load TUI config from config file (best-effort, defaults if missing/unparseable)
        if let Ok(content) = std::fs::read_to_string(&self.state.config_path) {
            if let Ok(table) = content.parse::<toml::Table>() {
                if let Some(tui_val) = table.get("tui") {
                    if let Ok(tui_cfg) = tui_val.clone().try_into::<rockbot_core::TuiConfig>() {
                        self.state.tui_config = tui_cfg;
                        self.effect_state
                            .set_animations_enabled(self.state.tui_config.animations);
                        self.effect_state.animation_style =
                            self.state.tui_config.animation_style.clone();
                        self.sync_settings_picker_from_theme();
                    }
                }
            }
        }

        // Spawn background tasks for initial data loading
        self.spawn_gateway_check();
        self.spawn_agents_load();
        self.spawn_vault_check();
        self.spawn_providers_load();
        self.spawn_cron_jobs_load();
        self.spawn_credential_schemas_load();
        self.spawn_sessions_load();
        self.spawn_remote_executors_load();
        self.spawn_ws_connect();
        self.spawn_keybinding_watch();
        Ok(())
    }

    fn current_theme_token(&self) -> ThemeToken {
        ThemeToken::all()[self
            .state
            .selected_theme_token
            .min(ThemeToken::all().len() - 1)]
    }

    fn current_font_role(&self) -> FontRole {
        FontRole::all()[self.state.selected_font_role.min(FontRole::all().len() - 1)]
    }

    fn sync_settings_picker_from_theme(&mut self) {
        let theme = self.state.tui_config.resolved_theme();
        let color = self.current_theme_token().value(&theme);
        let (hue, saturation, value) = super::components::settings::rgba_to_hsv(color);
        self.state.settings_color_hue = hue;
        self.state.settings_color_saturation = saturation;
        self.state.settings_color_value = value;
        self.state.settings_color_alpha = color.a;
    }

    fn ensure_custom_theme(&mut self) -> rockbot_core::TuiThemeConfig {
        self.state
            .tui_config
            .theme
            .clone()
            .unwrap_or_else(|| self.state.tui_config.resolved_theme())
    }

    fn save_tui_preferences(&mut self) {
        match save_tui_preferences_to_config(&self.state.config_path, &self.state.tui_config) {
            Ok(()) => {
                self.state.settings_save_feedback =
                    Some(("Saved to rockbot.toml".to_string(), false));
            }
            Err(err) => {
                tracing::warn!("Failed to save TUI preferences: {err}");
                self.state.settings_save_feedback = Some((format!("Save failed: {err}"), true));
            }
        }
    }

    fn apply_selected_theme_color(&mut self) {
        let mut theme = self.ensure_custom_theme();
        let color = super::components::settings::hsv_to_rgba(
            self.state.settings_color_hue,
            self.state.settings_color_saturation,
            self.state.settings_color_value,
            self.state.settings_color_alpha,
        );
        self.current_theme_token().set_value(&mut theme, color);
        self.state.tui_config.theme = Some(theme);
        self.save_tui_preferences();
    }

    fn cycle_theme_token(&mut self, delta: isize) {
        let len = ThemeToken::all().len() as isize;
        let current = self.state.selected_theme_token as isize;
        self.state.selected_theme_token = (current + delta).rem_euclid(len) as usize;
        self.sync_settings_picker_from_theme();
    }

    fn adjust_theme_component(&mut self, delta: f32) {
        match self.state.selected_settings_field {
            3 => {
                self.state.settings_color_hue =
                    (self.state.settings_color_hue + delta).rem_euclid(1.0);
            }
            4 => {
                self.state.settings_color_saturation =
                    (self.state.settings_color_saturation + delta).clamp(0.0, 1.0);
            }
            5 => {
                self.state.settings_color_value =
                    (self.state.settings_color_value + delta).clamp(0.0, 1.0);
            }
            6 => {
                let alpha = (f32::from(self.state.settings_color_alpha) + (delta * 255.0))
                    .clamp(0.0, 255.0);
                self.state.settings_color_alpha = alpha.round() as u8;
            }
            _ => return,
        }
        self.apply_selected_theme_color();
    }

    fn cycle_font_family(&mut self, delta: isize) {
        let options = super::components::settings::FONT_FAMILY_OPTIONS;
        let role = self.current_font_role();
        let current = role.family(&self.state.tui_config.fonts);
        let current_idx = options
            .iter()
            .position(|item| *item == current)
            .unwrap_or(0) as isize;
        let next_idx = (current_idx + delta).rem_euclid(options.len() as isize) as usize;
        role.set_family(
            &mut self.state.tui_config.fonts,
            options[next_idx].to_string(),
        );
        self.save_tui_preferences();
    }

    fn adjust_font_size(&mut self, delta: i16) {
        let role = self.current_font_role();
        let size = i32::from(role.size(&self.state.tui_config.fonts));
        let next = (size + i32::from(delta)).clamp(8, 48) as u16;
        role.set_size(&mut self.state.tui_config.fonts, next);
        self.save_tui_preferences();
    }

    /// Connect to the gateway via WebSocket using GatewayClient.
    fn spawn_ws_connect(&mut self) {
        if self.ws_connected() {
            return;
        }

        let ws_url = self.state.ws_url();
        let client = rockbot_client::GatewayClient::connect(&ws_url);
        let events_rx = client.subscribe();

        // Initiate Noise handshake once connected
        #[cfg(feature = "remote-exec")]
        {
            let sender = client.sender();
            tokio::spawn(async move {
                // Wait a moment for the connection to establish
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                match initiate_noise_handshake(&sender).await {
                    Ok(()) => tracing::info!("Noise handshake step 1 sent"),
                    Err(e) => tracing::warn!("Failed to initiate Noise handshake: {e}"),
                }
            });
        }

        self.gateway_client = Some(client);
        self.gateway_events_rx = Some(events_rx);
    }

    /// Check if a WebSocket connection is active
    fn ws_connected(&self) -> bool {
        self.gateway_client
            .as_ref()
            .is_some_and(|c| c.is_connected())
    }

    /// Spawn a task to check gateway status
    fn spawn_gateway_check(&self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match check_gateway_status(&gw).await {
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
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_agents(&config_path, &gw).await {
                Ok(agents) => {
                    let _ = tx.send(Message::AgentsLoaded(agents));
                }
                Err(e) => {
                    let _ = tx.send(Message::AgentsError(e.to_string()));
                }
            }
        });
    }

    /// Spawn a task to load cron jobs from gateway
    fn spawn_cron_jobs_load(&mut self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        self.state.cron_loading = true;
        tokio::spawn(async move {
            match load_cron_jobs_from_gateway(&gw).await {
                Ok(jobs) => {
                    let _ = tx.send(Message::CronJobsLoaded(jobs));
                }
                Err(e) => {
                    let _ = tx.send(Message::CronJobError(e.to_string()));
                }
            }
        });
    }

    fn spawn_cron_job_toggle(&self, job_id: &str, enabled: bool) {
        let tx = self.state.tx.clone();
        let job_id = job_id.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match toggle_cron_job(&gw, &job_id, enabled).await {
                Ok(()) => {
                    let _ = tx.send(Message::CronJobToggled(job_id, enabled));
                }
                Err(e) => {
                    let _ = tx.send(Message::CronJobError(format!("Toggle failed: {e}")));
                }
            }
        });
    }

    fn spawn_cron_job_delete(&self, job_id: &str) {
        let tx = self.state.tx.clone();
        let job_id = job_id.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match delete_cron_job(&gw, &job_id).await {
                Ok(()) => {
                    let _ = tx.send(Message::CronJobDeleted(job_id));
                }
                Err(e) => {
                    let _ = tx.send(Message::CronJobError(format!("Delete failed: {e}")));
                }
            }
        });
    }

    fn spawn_cron_job_trigger(&self, job_id: &str) {
        let tx = self.state.tx.clone();
        let job_id = job_id.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match trigger_cron_job(&gw, &job_id).await {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus("Cron job triggered".to_string(), false));
                }
                Err(e) => {
                    let _ = tx.send(Message::CronJobError(format!("Trigger failed: {e}")));
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

    /// Spawn a background watcher that polls the vault store for keybinding changes every 5s.
    fn spawn_keybinding_watch(&self) {
        let vault_path = self.state.vault_path.clone();
        let tx = self.state.tx.clone();
        tokio::spawn(async move {
            let store_path = vault_path.join("agents.redb");
            // Open the store once; if it doesn't exist yet, we just do nothing.
            let store = match rockbot_store::Store::open(&store_path) {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut last_hash: Option<u64> = None;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(Some(bytes)) = store.kv_get("config", "keybindings") {
                    let hash = {
                        use std::hash::{Hash, Hasher};
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        bytes.hash(&mut hasher);
                        hasher.finish()
                    };
                    if last_hash != Some(hash) {
                        last_hash = Some(hash);
                        if let Ok(config) =
                            serde_json::from_slice::<super::keybindings::KeybindingConfig>(&bytes)
                        {
                            let _ = tx.send(Message::KeybindingsReloaded(Box::new(config)));
                        }
                    }
                }
            }
        });
    }

    /// Spawn a task to load credential schemas from gateway
    fn spawn_credential_schemas_load(&self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_credential_schemas(&gw).await {
                Ok(schemas) if !schemas.is_empty() => {
                    let _ = tx.send(Message::CredentialSchemasLoaded(schemas));
                }
                _ => {}
            }
        });
    }

    /// Spawn a task to load providers from gateway
    fn spawn_providers_load(&self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_providers_from_gateway(&gw).await {
                Ok(providers) if !providers.is_empty() => {
                    let _ = tx.send(Message::ModelsLoaded(providers));
                }
                _ => {
                    // Gateway not available - providers list stays empty
                }
            }
        });
    }

    /// Spawn a task to load sessions from gateway
    fn spawn_sessions_load(&self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_sessions_from_gateway(&gw).await {
                Ok(sessions) => {
                    let _ = tx.send(Message::SessionsLoaded(sessions));
                }
                Err(e) => {
                    let _ = tx.send(Message::SessionsError(e.to_string()));
                }
            }
        });
    }

    /// Spawn a task to load connected remote executors from the gateway.
    fn spawn_remote_executors_load(&self) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_remote_executors_from_gateway(&gw).await {
                Ok(executors) => {
                    let _ = tx.send(Message::RemoteExecutorsLoaded(executors));
                }
                Err(e) => {
                    let _ = tx.send(Message::RemoteExecutorsError(e.to_string()));
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

        // ReloadAgents triggers a fresh load from gateway/config
        if matches!(msg, Message::ReloadAgents) {
            self.spawn_agents_load();
        }
        // ReloadSessions triggers a fresh load from gateway
        if matches!(msg, Message::ReloadSessions) {
            self.spawn_sessions_load();
        }
        // ReloadProviders triggers a fresh load of provider status
        if matches!(msg, Message::ReloadProviders) {
            self.spawn_providers_load();
        }

        // KeybindingsReloaded updates keybindings on App (not AppState)
        if let Message::KeybindingsReloaded(ref config) = msg {
            self.keybindings = *config.clone();
            tracing::info!("Keybindings reloaded from vault");
        }

        self.state.update(msg);
    }

    /// Auto-unlock a keyfile-protected vault (no user interaction needed)
    fn auto_unlock_keyfile_vault(&mut self, path_hint: Option<String>) {
        let keyfile_path = path_hint.or_else(|| {
            dirs::config_dir().map(|d| {
                d.join("rockbot")
                    .join("vault.key")
                    .to_string_lossy()
                    .to_string()
            })
        });

        if let Some(kf_path) = keyfile_path {
            let kf_pathbuf = std::path::PathBuf::from(&kf_path);
            if kf_pathbuf.exists() {
                match rockbot_credentials::CredentialVault::open(&self.state.vault_path) {
                    Ok(mut storage) => {
                        match storage.unlock_with_keyfile(&kf_pathbuf) {
                            Ok(()) => {
                                // Load endpoints after unlocking
                                let endpoints: Vec<EndpointInfo> = storage
                                    .list_endpoints()
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
                                self.state.status_message =
                                    Some(("✅ Vault auto-unlocked".to_string(), false));
                            }
                            Err(e) => {
                                self.state.status_message =
                                    Some((format!("❌ Auto-unlock failed: {e}"), true));
                            }
                        }
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("❌ Failed to open vault: {e}"), true));
                    }
                }
            }
        }
    }

    /// Handle key events.
    /// Only receives Press events — release/repeat filtered by the EventStream task.
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
                let action = *action;
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
            InputMode::CreateSession(state) => {
                let state = state.clone();
                self.handle_create_session(key, state)
            }
            InputMode::Confirm { action, .. } => {
                let action = action.clone();
                self.handle_confirm(key, action)
            }
            InputMode::ChatInput => self.handle_chat_input(key),
            InputMode::ViewSession { .. } => self.handle_view_session(key),
            InputMode::ViewEndpoint { endpoint_index } => {
                let idx = *endpoint_index;
                self.handle_view_endpoint(key, idx)
            }
            InputMode::ViewProvider { provider_index } => {
                let idx = *provider_index;
                self.handle_view_provider(key, idx)
            }
            InputMode::ViewModelList {
                provider_index,
                scroll,
            } => {
                let idx = *provider_index;
                let s = *scroll;
                self.handle_view_model_list(key, idx, s)
            }
            InputMode::ViewPermission { permission_index } => {
                let idx = *permission_index;
                self.handle_view_permission(key, idx)
            }
            InputMode::EditPermission(state) => {
                let state = state.clone();
                self.handle_edit_permission(key, state)
            }
            InputMode::ViewContextFiles(state) => {
                let state = state.clone();
                self.handle_view_context_files(key, state)
            }
            InputMode::EditContextFile(state) => {
                let state = state.clone();
                self.handle_edit_context_file(key, state)
            }
            InputMode::ContextMenu(menu_state) => {
                let menu_state = menu_state.clone();
                self.handle_context_menu(key, menu_state)
            }
            InputMode::CardDetail(detail) => {
                let detail = detail.clone();
                self.handle_card_detail(key, detail)
            }
            InputMode::VaultOverlay => self.handle_vault_overlay(key),
            InputMode::SettingsOverlay => self.handle_settings_overlay(key),
            InputMode::ModelsOverlay {
                provider_index,
                scroll,
            } => {
                let pi = *provider_index;
                let s = *scroll;
                self.handle_models_overlay(key, pi, s)
            }
            InputMode::CronOverlay { scroll } => {
                let s = *scroll;
                self.handle_cron_overlay(key, s)
            }
        }
    }

    fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<()> {
        // Also support { and } as aliases for Shift+[ / Shift+] (US keyboard layouts)
        if key.code == KeyCode::Char('{') {
            self.prev_content_tab();
            return Ok(());
        }
        if key.code == KeyCode::Char('}') {
            self.next_content_tab();
            return Ok(());
        }

        if let Some(action) = self.keybindings.lookup("normal", &key) {
            self.dispatch_action(action);
        }
        Ok(())
    }

    fn dispatch_action(&mut self, action: crate::keybindings::TuiAction) {
        use crate::keybindings::TuiAction::*;
        match action {
            Quit => {
                self.state.should_exit = true;
            }
            NavUp => match self.state.menu_item {
                MenuItem::Credentials => self.state.credential_list_prev(),
                MenuItem::Sessions => {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if chat.auto_scroll {
                            chat.scroll = chat.max_scroll.get();
                            chat.auto_scroll = false;
                        }
                        chat.scroll = chat.scroll.saturating_sub(1);
                    }
                }
                MenuItem::CronJobs => {
                    if !self.state.cron_jobs.is_empty() {
                        self.state.selected_cron_job = if self.state.selected_cron_job == 0 {
                            self.state.cron_jobs.len() - 1
                        } else {
                            self.state.selected_cron_job - 1
                        };
                    }
                }
                _ => {}
            },
            NavDown => match self.state.menu_item {
                MenuItem::Credentials => self.state.credential_list_next(),
                MenuItem::Sessions => {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if chat.auto_scroll {
                            return;
                        }
                        chat.scroll = chat.scroll.saturating_add(1);
                        if chat.scroll >= chat.max_scroll.get() {
                            chat.auto_scroll = true;
                        }
                    }
                }
                MenuItem::CronJobs => {
                    if !self.state.cron_jobs.is_empty() {
                        self.state.selected_cron_job =
                            (self.state.selected_cron_job + 1) % self.state.cron_jobs.len();
                    }
                }
                _ => {}
            },
            NavLeft => {
                self.state.select_prev();
                self.sync_slot_bar_from_selection();
                if self.state.menu_item == MenuItem::Sessions {
                    self.on_session_selection_changed();
                }
            }
            NavRight => {
                self.state.select_next();
                self.sync_slot_bar_from_selection();
                if self.state.menu_item == MenuItem::Sessions {
                    self.on_session_selection_changed();
                }
            }
            Enter => match self.state.menu_item {
                MenuItem::Credentials => match self.state.credentials_tab {
                    0 if !self.state.endpoints.is_empty() => {
                        self.state.input_mode = InputMode::ViewEndpoint {
                            endpoint_index: self.state.selected_endpoint,
                        };
                    }
                    1 => {
                        self.state.input_mode = InputMode::ViewProvider {
                            provider_index: self.state.selected_provider_index,
                        };
                    }
                    2 if !self.state.permissions.is_empty() => {
                        self.state.input_mode = InputMode::ViewPermission {
                            permission_index: self.state.selected_permission,
                        };
                    }
                    _ => {}
                },
                MenuItem::Models if !self.state.providers.is_empty() => {
                    self.state.input_mode = InputMode::ViewModelList {
                        provider_index: self.state.selected_provider,
                        scroll: 0,
                    };
                }
                _ => {}
            },
            Escape => {
                // No-op in normal mode (was: return to sidebar)
            }
            // Card bar navigation via Alt+arrows
            CardLeft => {
                self.state.slot_bar.select_left();
                self.sync_selection_from_slot_bar();
            }
            CardRight => {
                self.state.slot_bar.select_right();
                self.sync_selection_from_slot_bar();
            }
            CardUp => {
                let agents = self.state.agents.clone();
                let sessions = self.state.sessions.clone();
                self.state.slot_bar.cycle_up(&agents, &sessions);
                let new_mode = self.state.slot_bar.current_mode();
                self.state.menu_item = new_mode;
                self.state.menu_index = new_mode.index();
            }
            CardDown => {
                let agents = self.state.agents.clone();
                let sessions = self.state.sessions.clone();
                self.state.slot_bar.cycle_down(&agents, &sessions);
                let new_mode = self.state.slot_bar.current_mode();
                self.state.menu_item = new_mode;
                self.state.menu_index = new_mode.index();
            }
            CardActivate => {
                // Open card detail overlay for the active slot (slot 1+)
                if self.state.slot_bar.active_slot > 0 {
                    self.state.input_mode = InputMode::CardDetail(crate::state::CardDetailState {
                        mode: self.state.menu_item,
                        slot_index: self.state.slot_bar.active_slot,
                        scroll: 0,
                    });
                }
            }
            JumpToSection(n) => match n {
                1..=4 => {
                    let item = match n {
                        1 => MenuItem::Dashboard,
                        2 => MenuItem::Agents,
                        3 => MenuItem::Sessions,
                        _ => MenuItem::CronJobs,
                    };
                    self.state.menu_item = item;
                    self.state.menu_index = item.index();
                    self.state.slot_bar.mode = item.index();
                    self.state.slot_bar.slots[0].label = item.title().to_string();
                    let agents = self.state.agents.clone();
                    let sessions = self.state.sessions.clone();
                    self.state
                        .slot_bar
                        .rebuild_content_slots(&agents, &sessions);
                    // Update chat target
                    self.sync_chat_target();
                }
                5 => {
                    self.state.input_mode = InputMode::CronOverlay { scroll: 0 };
                }
                6 => {
                    self.state.input_mode = InputMode::VaultOverlay;
                }
                7 => {
                    self.state.input_mode = InputMode::ModelsOverlay {
                        provider_index: 0,
                        scroll: 0,
                    };
                }
                _ => {}
            },
            PrevTab => {
                self.prev_content_tab();
            }
            NextTab => {
                self.next_content_tab();
            }
            ScrollUp => {
                if self.state.menu_item == MenuItem::Sessions {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if chat.auto_scroll {
                            chat.scroll = chat.max_scroll.get();
                            chat.auto_scroll = false;
                        }
                        chat.scroll = chat.scroll.saturating_sub(10);
                    }
                }
            }
            ScrollDown => {
                if self.state.menu_item == MenuItem::Sessions {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if !chat.auto_scroll {
                            chat.scroll = chat.scroll.saturating_add(10);
                            if chat.scroll >= chat.max_scroll.get() {
                                chat.auto_scroll = true;
                            }
                        }
                    }
                }
            }
            ScrollEnd => {
                if self.state.menu_item == MenuItem::Sessions {
                    if let Some(chat) = self.state.active_chat_mut() {
                        chat.auto_scroll = true;
                    }
                }
            }
            Add => {
                self.handle_add_action();
            }
            Delete => {
                self.handle_delete_action();
            }
            Refresh => {
                self.handle_refresh_action();
            }
            InitVault => {
                self.handle_init_action();
            }
            UnlockVault => {
                self.handle_unlock_action();
            }
            LockVault => {
                self.handle_lock_action();
            }
            Chat => {
                self.handle_chat_action();
            }
            NewSession => {
                if self.state.menu_item == MenuItem::Sessions {
                    self.handle_new_session_action();
                }
            }
            Edit => {
                self.handle_edit_action();
            }
            ContextFiles => {
                if self.state.menu_item == MenuItem::Agents {
                    if let Some(agent) = self.state.agents.get(self.state.selected_agent) {
                        let agent_id = agent.id.clone();
                        self.state.input_mode =
                            InputMode::ViewContextFiles(ViewContextFilesState {
                                agent_id: agent_id.clone(),
                                files: Vec::new(),
                                selected: 0,
                                loading: true,
                            });
                        self.fetch_context_files(&agent_id);
                    }
                }
            }
            Permissions => {
                self.handle_permission_action();
            }
            Kill => {
                self.handle_kill_action();
            }
            View => {
                self.handle_view_action();
            }
            TestAction => {
                self.handle_test_action();
            }
            StartGateway => {
                self.handle_start_action();
            }
            StopGateway => {
                self.handle_stop_action();
            }
            OpenContextMenu => {
                let menu = self.state.build_context_menu();
                self.state.input_mode = InputMode::ContextMenu(menu);
            }
            OpenVault => {
                self.state.input_mode = InputMode::VaultOverlay;
            }
            OpenSettings => {
                self.sync_settings_picker_from_theme();
                self.state.input_mode = InputMode::SettingsOverlay;
            }
            OpenModels => {
                self.state.input_mode = InputMode::ModelsOverlay {
                    provider_index: 0,
                    scroll: 0,
                };
            }
            OpenCron => {
                self.state.input_mode = InputMode::CronOverlay { scroll: 0 };
            }
        }
    }

    fn handle_context_menu(&mut self, key: KeyEvent, mut menu: ContextMenuState) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?') => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
                self.state.input_mode = InputMode::ContextMenu(menu);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if menu.selected + 1 < menu.items.len() {
                    menu.selected += 1;
                }
                self.state.input_mode = InputMode::ContextMenu(menu);
            }
            KeyCode::Enter => {
                let action = menu.items[menu.selected].action;
                self.state.input_mode = InputMode::Normal;
                self.dispatch_context_action(action);
            }
            KeyCode::Char(c) => {
                if let Some(item) = menu.items.iter().find(|i| i.key == c) {
                    let action = item.action;
                    self.state.input_mode = InputMode::Normal;
                    self.dispatch_context_action(action);
                } else {
                    self.state.input_mode = InputMode::ContextMenu(menu);
                }
            }
            _ => {
                self.state.input_mode = InputMode::ContextMenu(menu);
            }
        }
        Ok(())
    }

    fn handle_card_detail(
        &mut self,
        key: KeyEvent,
        mut detail: crate::state::CardDetailState,
    ) -> Result<()> {
        let slot_label = self
            .state
            .slot_bar
            .slots
            .get(detail.slot_index)
            .map(|slot| slot.label.clone())
            .unwrap_or_default();
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            // Alt+Enter also closes
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('t') if slot_label == "Exec" => {
                self.state.allow_local_tool_execution = !self.state.allow_local_tool_execution;
                if self.state.allow_local_tool_execution {
                    self.state.status_message =
                        Some(("Tool execution target: active client".to_string(), false));
                } else {
                    self.state.status_message =
                        Some(("Tool execution target: manual selection".to_string(), false));
                }
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Up | KeyCode::Char('k') if slot_label == "Exec" => {
                if !self.state.remote_executors.is_empty() {
                    if self.state.selected_executor_index == 0 {
                        self.state.selected_executor_index = self.state.remote_executors.len() - 1;
                    } else {
                        self.state.selected_executor_index -= 1;
                    }
                }
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Down | KeyCode::Char('j') if slot_label == "Exec" => {
                if !self.state.remote_executors.is_empty() {
                    self.state.selected_executor_index = (self.state.selected_executor_index + 1)
                        % self.state.remote_executors.len();
                }
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Enter if slot_label == "Exec" => {
                self.state.selected_executor_target = self
                    .state
                    .remote_executors
                    .get(self.state.selected_executor_index)
                    .map(|executor| executor.target_id.clone());
                self.state.allow_local_tool_execution = false;
                self.state.status_message = Some((
                    format!(
                        "Tool execution target: {}",
                        self.state
                            .remote_executors
                            .get(self.state.selected_executor_index)
                            .map(RemoteExecutorInfo::display_name)
                            .unwrap_or_else(|| "gateway".to_string())
                    ),
                    false,
                ));
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Char('g') if slot_label == "Exec" => {
                self.state.allow_local_tool_execution = false;
                self.state.selected_executor_target = None;
                self.state.status_message =
                    Some(("Tool execution target: gateway".to_string(), false));
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                detail.scroll = detail.scroll.saturating_sub(1);
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                detail.scroll = detail.scroll.saturating_add(1);
                self.state.input_mode = InputMode::CardDetail(detail);
            }
            _ => {
                self.state.input_mode = InputMode::CardDetail(detail);
            }
        }
        Ok(())
    }

    fn handle_vault_overlay(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Tab => {
                self.state.credentials_tab = (self.state.credentials_tab + 1) % 4;
            }
            KeyCode::Char(c @ '1'..='4') => {
                self.state.credentials_tab = (c as usize - '1' as usize).min(3);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.credential_list_prev();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.credential_list_next();
            }
            KeyCode::Enter => match self.state.credentials_tab {
                0 if !self.state.endpoints.is_empty() => {
                    self.state.input_mode = InputMode::ViewEndpoint {
                        endpoint_index: self.state.selected_endpoint,
                    };
                }
                1 => {
                    self.state.input_mode = InputMode::ViewProvider {
                        provider_index: self.state.selected_provider_index,
                    };
                }
                2 if !self.state.permissions.is_empty() => {
                    self.state.input_mode = InputMode::ViewPermission {
                        permission_index: self.state.selected_permission,
                    };
                }
                _ => {}
            },
            KeyCode::Char('a') => {
                self.handle_add_action();
            }
            KeyCode::Char('d') => {
                self.handle_delete_action();
            }
            KeyCode::Char('i') => {
                self.handle_init_action();
            }
            KeyCode::Char('u') => {
                self.handle_unlock_action();
            }
            KeyCode::Char('l') => {
                self.handle_lock_action();
            }
            KeyCode::Char('p') => {
                self.handle_permission_action();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_settings_overlay(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('s') => {
                self.handle_start_action();
            }
            KeyCode::Char('S') => {
                self.handle_stop_action();
            }
            KeyCode::Char('r') => {
                self.handle_refresh_action();
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.state.selected_settings_card =
                    self.state.selected_settings_card.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Tab => {
                if self.state.selected_settings_card < 4 {
                    self.state.selected_settings_card += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') if self.state.selected_settings_card == 3 => {
                self.state.selected_settings_field =
                    self.state.selected_settings_field.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.state.selected_settings_card == 3 => {
                if self.state.selected_settings_field < 6 {
                    self.state.selected_settings_field += 1;
                }
            }
            KeyCode::Up | KeyCode::Char('k') if self.state.selected_settings_card == 4 => {
                self.state.selected_font_field = self.state.selected_font_field.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.state.selected_settings_card == 4 => {
                if self.state.selected_font_field < 2 {
                    self.state.selected_font_field += 1;
                }
            }
            KeyCode::Char(']') if self.state.selected_settings_card == 3 => {
                match self.state.selected_settings_field {
                    0 => {
                        self.state.tui_config.color_theme =
                            self.state.tui_config.color_theme.next();
                        self.state.tui_config.theme = Some(self.state.tui_config.resolved_theme());
                        self.sync_settings_picker_from_theme();
                        self.save_tui_preferences();
                    }
                    1 => {
                        let next = self.state.tui_config.animation_style.next();
                        self.state.tui_config.animation_style = next.clone();
                        self.effect_state.animation_style = next;
                        self.save_tui_preferences();
                    }
                    2 => self.cycle_theme_token(1),
                    3..=6 => self.adjust_theme_component(0.02),
                    _ => {}
                }
            }
            KeyCode::Char('[') if self.state.selected_settings_card == 3 => {
                match self.state.selected_settings_field {
                    0 => {
                        self.state.tui_config.color_theme =
                            self.state.tui_config.color_theme.prev();
                        self.state.tui_config.theme = Some(self.state.tui_config.resolved_theme());
                        self.sync_settings_picker_from_theme();
                        self.save_tui_preferences();
                    }
                    1 => {
                        let prev = self.state.tui_config.animation_style.prev();
                        self.state.tui_config.animation_style = prev.clone();
                        self.effect_state.animation_style = prev;
                        self.save_tui_preferences();
                    }
                    2 => self.cycle_theme_token(-1),
                    3..=6 => self.adjust_theme_component(-0.02),
                    _ => {}
                }
            }
            KeyCode::Char(']') if self.state.selected_settings_card == 4 => {
                match self.state.selected_font_field {
                    0 => {
                        self.state.selected_font_role =
                            (self.state.selected_font_role + 1) % FontRole::all().len();
                    }
                    1 => self.cycle_font_family(1),
                    2 => self.adjust_font_size(1),
                    _ => {}
                }
            }
            KeyCode::Char('[') if self.state.selected_settings_card == 4 => {
                match self.state.selected_font_field {
                    0 => {
                        self.state.selected_font_role = self
                            .state
                            .selected_font_role
                            .checked_sub(1)
                            .unwrap_or(FontRole::all().len() - 1);
                    }
                    1 => self.cycle_font_family(-1),
                    2 => self.adjust_font_size(-1),
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, full_area: Rect) -> Result<()> {
        if matches!(self.state.input_mode, InputMode::SettingsOverlay) {
            self.handle_settings_overlay_mouse(mouse, full_area);
        }
        Ok(())
    }

    fn handle_settings_overlay_mouse(&mut self, mouse: MouseEvent, full_area: Rect) {
        if !matches!(
            mouse.kind,
            MouseEventKind::Down(_) | MouseEventKind::Drag(_)
        ) {
            return;
        }

        let overlay = super::components::centered_rect(80, 85, full_area);
        if mouse.column < overlay.x
            || mouse.column >= overlay.x + overlay.width
            || mouse.row < overlay.y
            || mouse.row >= overlay.y + overlay.height
        {
            return;
        }

        let inner = Rect {
            x: overlay.x + 1,
            y: overlay.y + 1,
            width: overlay.width.saturating_sub(2),
            height: overlay.height.saturating_sub(2),
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Fill(1)])
            .split(inner);

        let tab_cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Ratio(
                    1,
                    super::components::settings::SETTINGS_SECTION_LABELS.len() as u32
                );
                super::components::settings::SETTINGS_SECTION_LABELS
                    .len()
            ])
            .split(chunks[0]);
        for (idx, rect) in tab_cells.iter().enumerate() {
            if contains_point(*rect, mouse.column, mouse.row) {
                self.state.selected_settings_card = idx;
                if idx == 3 {
                    self.sync_settings_picker_from_theme();
                }
                return;
            }
        }

        match self.state.selected_settings_card {
            3 => self.handle_theme_mouse(mouse, chunks[1]),
            4 => self.handle_typography_mouse(mouse, chunks[1]),
            _ => {}
        }
    }

    fn handle_theme_mouse(&mut self, mouse: MouseEvent, area: Rect) {
        let layout = super::components::settings::theme_editor_layout(area);
        let control_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Length(3)])
            .split(layout.controls);

        for (idx, rect) in super::components::settings::preset_cells(control_rows[0])
            .into_iter()
            .enumerate()
        {
            if contains_point(rect, mouse.column, mouse.row) {
                self.state.selected_settings_field = 0;
                self.state.tui_config.color_theme = ColorTheme::all()[idx].clone();
                self.state.tui_config.theme = Some(self.state.tui_config.resolved_theme());
                self.sync_settings_picker_from_theme();
                self.save_tui_preferences();
                return;
            }
        }

        for (idx, rect) in super::components::settings::animation_cells(control_rows[1])
            .into_iter()
            .enumerate()
        {
            if contains_point(rect, mouse.column, mouse.row) {
                self.state.selected_settings_field = 1;
                let style = AnimationStyle::all()[idx].clone();
                self.state.tui_config.animation_style = style.clone();
                self.effect_state.animation_style = style;
                self.save_tui_preferences();
                return;
            }
        }

        let token_inner = shrink_rect(layout.tokens, 1);
        if contains_point(token_inner, mouse.column, mouse.row) {
            let row = mouse.row.saturating_sub(token_inner.y) as usize;
            if row < ThemeToken::all().len() {
                self.state.selected_settings_field = 2;
                self.state.selected_theme_token = row;
                self.sync_settings_picker_from_theme();
            }
            return;
        }

        let wheel_inner = shrink_rect(layout.wheel, 1);
        if contains_point(wheel_inner, mouse.column, mouse.row) {
            self.state.selected_settings_field = 3;
            let (hue, saturation) = point_to_wheel_hs(mouse.column, mouse.row, wheel_inner);
            self.state.settings_color_hue = hue;
            self.state.settings_color_saturation = saturation;
            self.apply_selected_theme_color();
            return;
        }

        if contains_point(layout.value_slider, mouse.column, mouse.row) {
            self.state.selected_settings_field = 5;
            self.state.settings_color_value = point_to_slider(mouse.column, layout.value_slider);
            self.apply_selected_theme_color();
            return;
        }

        if contains_point(layout.alpha_slider, mouse.column, mouse.row) {
            self.state.selected_settings_field = 6;
            self.state.settings_color_alpha =
                (point_to_slider(mouse.column, layout.alpha_slider) * 255.0).round() as u8;
            self.apply_selected_theme_color();
        }
    }

    fn handle_typography_mouse(&mut self, mouse: MouseEvent, area: Rect) {
        let layout = super::components::settings::typography_layout(area);
        let roles_inner = shrink_rect(layout.roles, 1);
        if contains_point(roles_inner, mouse.column, mouse.row) {
            let row = mouse.row.saturating_sub(roles_inner.y) as usize;
            if row < FontRole::all().len() {
                self.state.selected_font_field = 0;
                self.state.selected_font_role = row;
            }
            return;
        }

        for (idx, rect) in super::components::settings::family_cells(layout.families)
            .into_iter()
            .enumerate()
        {
            if contains_point(rect, mouse.column, mouse.row) {
                self.state.selected_font_field = 1;
                self.current_font_role().set_family(
                    &mut self.state.tui_config.fonts,
                    super::components::settings::FONT_FAMILY_OPTIONS[idx].to_string(),
                );
                self.save_tui_preferences();
                return;
            }
        }

        let size_inner = shrink_rect(layout.size, 1);
        let cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(4),
                Constraint::Fill(1),
                Constraint::Length(4),
            ])
            .split(size_inner);
        if contains_point(cells[0], mouse.column, mouse.row) {
            self.state.selected_font_field = 2;
            self.adjust_font_size(-1);
        } else if contains_point(cells[2], mouse.column, mouse.row) {
            self.state.selected_font_field = 2;
            self.adjust_font_size(1);
        }
    }

    fn handle_models_overlay(
        &mut self,
        key: KeyEvent,
        mut provider_index: usize,
        mut scroll: usize,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                return Ok(());
            }
            KeyCode::Left | KeyCode::Char('h') => {
                provider_index = provider_index.saturating_sub(1);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if !self.state.providers.is_empty() {
                    provider_index =
                        (provider_index + 1).min(self.state.providers.len().saturating_sub(1));
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                scroll = scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                scroll = scroll.saturating_add(1);
            }
            KeyCode::Enter if !self.state.providers.is_empty() => {
                self.state.input_mode = InputMode::ViewModelList {
                    provider_index,
                    scroll: 0,
                };
                return Ok(());
            }
            KeyCode::Char('e') if !self.state.providers.is_empty() => {
                self.state.selected_provider = provider_index;
                self.handle_edit_action();
                return Ok(());
            }
            _ => {}
        }
        self.state.input_mode = InputMode::ModelsOverlay {
            provider_index,
            scroll,
        };
        Ok(())
    }

    fn handle_cron_overlay(&mut self, key: KeyEvent, mut scroll: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                return Ok(());
            }
            KeyCode::Tab => {
                self.state.selected_cron_card = (self.state.selected_cron_card + 1) % 3;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !self.state.cron_jobs.is_empty() {
                    self.state.selected_cron_job = if self.state.selected_cron_job == 0 {
                        self.state.cron_jobs.len() - 1
                    } else {
                        self.state.selected_cron_job - 1
                    };
                }
                scroll = scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !self.state.cron_jobs.is_empty() {
                    self.state.selected_cron_job =
                        (self.state.selected_cron_job + 1) % self.state.cron_jobs.len();
                }
                scroll = scroll.saturating_add(1);
            }
            KeyCode::Char('e') => {
                self.handle_edit_action();
            }
            KeyCode::Char('d') => {
                self.handle_delete_action();
            }
            KeyCode::Char('t') => {
                self.handle_test_action();
            }
            KeyCode::Char('r') => {
                self.handle_refresh_action();
            }
            _ => {}
        }
        self.state.input_mode = InputMode::CronOverlay { scroll };
        Ok(())
    }

    /// Sync per-mode `selected_*` index from slot_bar.active_slot (after CardLeft/CardRight)
    fn sync_selection_from_slot_bar(&mut self) {
        let idx = self.state.slot_bar.active_slot.saturating_sub(1);
        match self.state.menu_item {
            MenuItem::Agents => {
                if !self.state.agents.is_empty() {
                    self.state.selected_agent = idx.min(self.state.agents.len() - 1);
                }
            }
            MenuItem::Sessions => {
                if !self.state.sessions.is_empty() {
                    self.state.selected_session = idx.min(self.state.sessions.len() - 1);
                    self.on_session_selection_changed();
                }
            }
            MenuItem::CronJobs | MenuItem::Dashboard => {}
            // Legacy modes (now overlays) — no-op
            MenuItem::Credentials | MenuItem::Models | MenuItem::Settings => {}
        }
        self.sync_chat_target();
    }

    /// Sync slot_bar.active_slot from per-mode `selected_*` index (after NavLeft/NavRight)
    fn sync_slot_bar_from_selection(&mut self) {
        let idx = match self.state.menu_item {
            MenuItem::Agents => self.state.selected_agent,
            MenuItem::Sessions => self.state.selected_session,
            MenuItem::CronJobs | MenuItem::Dashboard => return,
            MenuItem::Credentials | MenuItem::Models | MenuItem::Settings => return,
        };
        // slot 0 is the mode selector; data slots start at 1
        let slot = idx + 1;
        let max_slot = self.state.slot_bar.slots.len().saturating_sub(1);
        self.state.slot_bar.active_slot = slot.min(max_slot);
        self.sync_chat_target();
    }

    /// Update chat_target based on current mode and selection
    fn sync_chat_target(&mut self) {
        self.state.chat_target = match self.state.menu_item {
            MenuItem::Dashboard => ChatTarget::Butler,
            MenuItem::Agents => self
                .state
                .agents
                .get(self.state.selected_agent)
                .map(|a| ChatTarget::Agent(a.id.clone()))
                .unwrap_or(ChatTarget::Butler),
            MenuItem::Sessions => self
                .state
                .sessions
                .get(self.state.selected_session)
                .map(|s| ChatTarget::Session(s.key.clone()))
                .unwrap_or(ChatTarget::Butler),
            _ => self.state.chat_target.clone(),
        };
    }

    fn dispatch_context_action(&mut self, action: ContextMenuAction) {
        match action {
            ContextMenuAction::OpenAddAgent => self.handle_add_action(),
            ContextMenuAction::OpenEditAgent => self.handle_edit_action(),
            ContextMenuAction::DeleteAgent => self.handle_delete_action(),
            ContextMenuAction::OpenAddCredential => self.handle_add_action(),
            ContextMenuAction::OpenCreateSession => self.handle_new_session_action(),
            ContextMenuAction::OpenContextFiles => {
                if let Some(agent) = self.state.agents.get(self.state.selected_agent) {
                    let agent_id = agent.id.clone();
                    self.state.input_mode = InputMode::ViewContextFiles(ViewContextFilesState {
                        agent_id: agent_id.clone(),
                        files: Vec::new(),
                        selected: 0,
                        loading: true,
                    });
                    self.fetch_context_files(&agent_id);
                }
            }
            ContextMenuAction::TriggerCronJob => self.handle_test_action(),
            ContextMenuAction::RefreshPage => self.handle_refresh_action(),
        }
    }

    fn handle_add_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Agents => {
                let mut agent_state = EditAgentState::new();
                agent_state.populate_models(&self.state.providers);
                self.state.input_mode = InputMode::AddAgent(agent_state);
            }
            MenuItem::Sessions => {
                self.handle_new_session_action();
            }
            MenuItem::Credentials if self.state.vault.initialized && !self.state.vault.locked => {
                if self.state.credentials_tab == 1 {
                    // Providers tab — open schema-driven configure form for selected provider
                    if let Some(schema) = self
                        .state
                        .credential_schemas
                        .get(self.state.selected_provider_index)
                        .cloned()
                    {
                        let idx = self.state.selected_provider_index;
                        self.state.input_mode =
                            InputMode::EditProvider(EditProviderState::from_schema(&schema, idx));
                        return;
                    }
                }
                // Endpoints tab or fallback: show generic add form
                let mut default_state = AddCredentialState::new();
                default_state.endpoint_type = 3; // API Key Service
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
            MenuItem::Sessions => {
                if let Some(session) = self.state.sessions.get(self.state.selected_session) {
                    self.state.input_mode = InputMode::Confirm {
                        message: format!("Archive session '{}'?", session.key),
                        action: ConfirmAction::KillSession(session.key.clone()),
                    };
                }
            }
            MenuItem::CronJobs => {
                if let Some(job) = self.state.cron_jobs.get(self.state.selected_cron_job) {
                    self.state.input_mode = InputMode::Confirm {
                        message: format!("Delete cron job '{}'?", job.name),
                        action: ConfirmAction::DeleteCronJob(job.id.clone()),
                    };
                }
            }
            _ => {}
        }
    }

    fn handle_refresh_action(&mut self) {
        match self.state.menu_item {
            MenuItem::Settings => {
                // On Settings tab, 'r' means restart gateway
                self.state.status_message = Some(("Restarting gateway...".to_string(), false));
                self.spawn_gateway_control("restart");
            }
            MenuItem::Agents => {
                // On Agents tab, reload agents from config
                self.state.status_message = Some(("Reloading agents...".to_string(), false));
                self.spawn_agents_load();
            }
            MenuItem::Sessions => {
                self.state.status_message = Some(("Reloading sessions...".to_string(), false));
                self.spawn_sessions_load();
            }
            MenuItem::CronJobs => {
                self.state.status_message = Some(("Reloading cron jobs...".to_string(), false));
                self.spawn_cron_jobs_load();
            }
            _ => {
                // General refresh
                self.state.status_message = Some(("Refreshing...".to_string(), false));
                self.spawn_gateway_check();
                self.spawn_agents_load();
                self.spawn_vault_check();
                self.spawn_sessions_load();
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
            self.state.clear_input();
        }
    }

    fn handle_unlock_action(&mut self) {
        // Debug: log unlock attempt
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/rockbot_debug.log")
        {
            use std::io::Write;
            let _ = writeln!(
                f,
                "handle_unlock_action: initialized={}, locked={}, method={:?}",
                self.state.vault.initialized,
                self.state.vault.locked,
                self.state.vault.unlock_method
            );
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
                self.state.clear_input();
            }
            UnlockMethod::Keyfile { path } => {
                // Debug: log keyfile unlock attempt
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/rockbot_debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(f, "Keyfile unlock: path={path:?}");
                }

                // Auto-unlock with keyfile - no password needed
                let keyfile_path = path.clone().or_else(|| {
                    dirs::config_dir().map(|d| {
                        d.join("rockbot")
                            .join("vault.key")
                            .to_string_lossy()
                            .to_string()
                    })
                });

                if let Some(kf_path) = keyfile_path {
                    let kf_pathbuf = std::path::PathBuf::from(&kf_path);
                    if kf_pathbuf.exists() {
                        // Actually unlock with keyfile
                        match rockbot_credentials::CredentialVault::open(&self.state.vault_path) {
                            Ok(mut storage) => {
                                match storage.unlock_with_keyfile(&kf_pathbuf) {
                                    Ok(()) => {
                                        // Load endpoints after unlocking
                                        let endpoints: Vec<EndpointInfo> = storage
                                            .list_endpoints()
                                            .into_iter()
                                            .map(|e| EndpointInfo {
                                                id: e.id.to_string(),
                                                name: e.name.clone(),
                                                endpoint_type: format!("{:?}", e.endpoint_type),
                                                base_url: e.base_url.clone(),
                                                has_credential: e.credential_id
                                                    != uuid::Uuid::nil(),
                                                expiration: None,
                                            })
                                            .collect();

                                        self.vault = Some(storage);
                                        self.state.vault.locked = false;
                                        self.state.vault.endpoint_count = endpoints.len();
                                        self.state.endpoints = endpoints;
                                        self.state.status_message =
                                            Some(("✅ Unlocked with keyfile".to_string(), false));
                                    }
                                    Err(e) => {
                                        self.state.status_message =
                                            Some((format!("❌ Keyfile unlock failed: {e}"), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message =
                                    Some((format!("❌ Failed to open vault: {e}"), true));
                            }
                        }
                    } else {
                        self.state.status_message =
                            Some((format!("Keyfile not found: {kf_path}"), true));
                    }
                } else {
                    self.state.status_message =
                        Some(("No keyfile path configured".to_string(), true));
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
                self.state.clear_input();
            }
            UnlockMethod::SshKey { path } => {
                // Try SSH agent unlock
                let ssh_path = path
                    .clone()
                    .unwrap_or_else(|| "~/.ssh/id_ed25519".to_string());
                // TODO: Actually unlock via SSH agent
                self.state.status_message = Some((
                    format!("SSH unlock not yet implemented (key: {ssh_path})"),
                    true,
                ));
            }
            UnlockMethod::Unknown => {
                // Default to password prompt
                self.state.input_mode = InputMode::PasswordInput {
                    prompt: "Enter vault password:".to_string(),
                    masked: true,
                    action: PasswordAction::UnlockVault,
                };
                self.state.clear_input();
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
        let has_provider = self.state.providers.iter().any(|p| p.available);

        if !has_provider {
            self.state.status_message = Some((
                "No LLM providers available — configure one in Models or Credentials → Providers"
                    .to_string(),
                true,
            ));
            return;
        }

        // Navigate to sessions if not there
        if self.state.menu_item != MenuItem::Sessions {
            self.state.menu_item = MenuItem::Sessions;
        }

        // If there's a selected session, set chat_model from it and open chat
        if let Some(session) = self.state.sessions.get(self.state.selected_session) {
            if let Some(ref model) = session.model {
                self.state.chat_model = Some(model.clone());
            } else if !session.agent_id.is_empty() {
                // Fall back to agent's configured model
                self.state.chat_model = self
                    .state
                    .agents
                    .iter()
                    .find(|a| a.id == session.agent_id)
                    .and_then(|a| a.model.clone());
            }
            // Ensure session has a chat state entry
            let key = session.key.clone();
            self.state.session_chats.entry(key).or_default();
            self.state.input_mode = InputMode::ChatInput;
            self.state.clear_input();
        } else {
            // No sessions — create one
            self.handle_new_session_action();
        }
    }

    fn handle_new_session_action(&mut self) {
        let has_provider = self.state.providers.iter().any(|p| p.available);
        if !has_provider {
            self.state.status_message = Some((
                "No LLM providers available — configure one in Models or Credentials → Providers"
                    .to_string(),
                true,
            ));
            return;
        }
        let create_state = CreateSessionState::new(&self.state.providers, &self.state.agents);
        self.state.input_mode = InputMode::CreateSession(create_state);
    }

    /// Open the edit credential modal for the given endpoint index (used from view modals)
    fn edit_endpoint_at(&mut self, endpoint_index: usize) {
        use crate::state::EditCredentialState;

        if let Some(endpoint) = self.state.endpoints.get(endpoint_index) {
            let endpoint_type = match endpoint.endpoint_type.as_str() {
                "HomeAssistant" => 0,
                "GenericRest" => 1,
                "GenericOAuth2" => 2,
                _ => 3,
            };

            let mut edit_state = EditCredentialState::from_endpoint(
                &endpoint.id,
                &endpoint.name,
                endpoint_type,
                &endpoint.base_url,
                if endpoint.has_credential {
                    Some(&endpoint.id)
                } else {
                    None
                },
            );

            if let Some(ref vault) = self.vault {
                if let Ok(uuid) = uuid::Uuid::parse_str(&endpoint.id) {
                    if let Ok(secret_bytes) = vault.decrypt_credential_for_endpoint(uuid) {
                        if let Ok(secret_str) = String::from_utf8(secret_bytes) {
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
        }
    }

    /// Open the edit permission modal for the given permission index (used from view modals)
    fn edit_permission_at(&mut self, permission_index: usize) {
        use crate::state::EditPermissionState;
        if let Some(rule) = self.state.permissions.get(permission_index) {
            let edit_state =
                EditPermissionState::from_rule(rule, &self.state.endpoints, &self.state.agents);
            self.state.input_mode = InputMode::EditPermission(edit_state);
        }
    }

    fn handle_edit_action(&mut self) {
        match self.state.menu_item {
            // 'e' is NOT available from the credentials list view — edit through info modals
            MenuItem::Agents => {
                if let Some(agent) = self.state.agents.get(self.state.selected_agent) {
                    let mut edit_state = EditAgentState::from_agent(agent);
                    edit_state.populate_models(&self.state.providers);
                    self.state.input_mode = InputMode::EditAgent(edit_state);
                }
            }
            MenuItem::CronJobs => {
                if let Some(job) = self.state.cron_jobs.get(self.state.selected_cron_job) {
                    let job_id = job.id.clone();
                    let new_enabled = !job.enabled;
                    self.spawn_cron_job_toggle(&job_id, new_enabled);
                }
            }
            MenuItem::Models => {
                // Edit model provider config — use schema if available
                let idx = self.state.selected_provider;
                let provider = self.state.providers.get(idx);
                // Find matching credential schema by provider ID
                let schema = provider.and_then(|p| {
                    self.state
                        .credential_schemas
                        .iter()
                        .find(|s| s.provider_id == p.id)
                });
                let edit_state = if let Some(schema) = schema {
                    EditProviderState::from_schema(schema, idx)
                } else {
                    EditProviderState::new(idx)
                };
                self.state.input_mode = InputMode::EditProvider(edit_state);
            }
            _ => {}
        }
    }

    fn handle_kill_action(&mut self) {
        if self.state.menu_item == MenuItem::Sessions {
            if let Some(session) = self.state.sessions.get(self.state.selected_session) {
                self.state.input_mode = InputMode::Confirm {
                    message: format!("Kill session '{}'?", session.key),
                    action: ConfirmAction::KillSession(session.key.clone()),
                };
            } else {
                self.state.status_message = Some(("No session selected".to_string(), true));
            }
        }
    }

    fn handle_view_action(&mut self) {
        if self.state.menu_item == MenuItem::Sessions {
            if let Some(session) = self.state.sessions.get(self.state.selected_session) {
                self.state.input_mode = InputMode::ViewSession {
                    session_key: session.key.clone(),
                };
                // Spawn async task to load session details
                self.spawn_session_details(&session.key);
            } else {
                self.state.status_message = Some(("No session selected".to_string(), true));
            }
        }
    }

    fn handle_permission_action(&mut self) {
        use crate::state::EditPermissionState;

        if self.state.menu_item != MenuItem::Credentials || self.state.endpoints.is_empty() {
            return;
        }

        if self.state.credentials_tab == 2 && !self.state.permissions.is_empty() {
            // On permissions tab with a selected permission — edit it
            if let Some(rule) = self.state.permissions.get(self.state.selected_permission) {
                let edit_state =
                    EditPermissionState::from_rule(rule, &self.state.endpoints, &self.state.agents);
                self.state.input_mode = InputMode::EditPermission(edit_state);
            }
        } else {
            // New permission — preselect endpoint if on endpoints tab
            let preselect = if self.state.credentials_tab == 0 {
                Some(self.state.selected_endpoint)
            } else {
                None
            };
            let edit_state =
                EditPermissionState::new(&self.state.endpoints, &self.state.agents, preselect);
            self.state.input_mode = InputMode::EditPermission(edit_state);
        }
    }

    fn handle_edit_permission(
        &mut self,
        key: KeyEvent,
        mut state: super::state::EditPermissionState,
    ) -> Result<()> {
        let field_count: usize = 3; // endpoint, source, access
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Tab | KeyCode::Down => {
                state.field_index = (state.field_index + 1) % field_count;
                self.state.input_mode = InputMode::EditPermission(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.field_index = if state.field_index == 0 {
                    field_count - 1
                } else {
                    state.field_index - 1
                };
                self.state.input_mode = InputMode::EditPermission(state);
            }
            KeyCode::Left => {
                match state.field_index {
                    0 => state.cycle_endpoint(false),
                    1 => state.cycle_source(false),
                    _ => state.cycle_access(false),
                }
                self.state.input_mode = InputMode::EditPermission(state);
            }
            KeyCode::Right => {
                match state.field_index {
                    0 => state.cycle_endpoint(true),
                    1 => state.cycle_source(true),
                    _ => state.cycle_access(true),
                }
                self.state.input_mode = InputMode::EditPermission(state);
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_permission(state);
            }
            KeyCode::Enter => {
                self.save_permission(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn save_permission(&mut self, state: super::state::EditPermissionState) {
        if state.is_edit {
            // Update existing rule: find by endpoint+source combo and update access
            if let Some(rule) = self.state.permissions.iter_mut().find(|r| {
                r.endpoint_id == state.selected_endpoint_id()
                    && r.source == state.sources[state.selected_source]
            }) {
                rule.access = state.access;
            } else {
                // Source changed — remove old, add new
                let next_priority = self
                    .state
                    .permissions
                    .iter()
                    .map(|r| r.priority)
                    .max()
                    .unwrap_or(0)
                    + 1;
                self.state.permissions.push(state.to_rule(next_priority));
            }
        } else {
            // New rule — remove existing rule for same endpoint+source combo
            self.state.permissions.retain(|r| {
                !(r.endpoint_id == state.selected_endpoint_id()
                    && r.source == state.sources[state.selected_source])
            });
            let next_priority = self
                .state
                .permissions
                .iter()
                .map(|r| r.priority)
                .max()
                .unwrap_or(0)
                + 1;
            self.state.permissions.push(state.to_rule(next_priority));
        }
        // Re-sort by priority
        self.state.permissions.sort_by_key(|r| r.priority);
        self.state.status_message = Some((
            format!("Permission set for '{}'", state.selected_endpoint_name()),
            false,
        ));
        self.state.input_mode = InputMode::Normal;
    }

    fn move_permission(&mut self, up: bool) {
        let idx = self.state.selected_permission;
        let len = self.state.permissions.len();
        if len < 2 {
            return;
        }
        if up && idx > 0 {
            self.state.permissions.swap(idx, idx - 1);
            // Update priorities
            self.state.permissions[idx - 1].priority = idx; // 1-based
            self.state.permissions[idx].priority = idx + 1;
            self.state.selected_permission = idx - 1;
        } else if !up && idx + 1 < len {
            self.state.permissions.swap(idx, idx + 1);
            self.state.permissions[idx].priority = idx + 1;
            self.state.permissions[idx + 1].priority = idx + 2;
            self.state.selected_permission = idx + 1;
        }
    }

    fn handle_test_action(&mut self) {
        if self.state.menu_item == MenuItem::Models {
            let idx = self
                .state
                .selected_provider
                .min(self.state.providers.len().saturating_sub(1));
            if let Some(provider) = self.state.providers.get(idx) {
                self.state.status_message =
                    Some((format!("Testing {} connection...", provider.name), false));
                self.spawn_model_test_via_gateway(&provider.id, &provider.name);
            }
        } else if self.state.menu_item == MenuItem::CronJobs {
            if let Some(job) = self.state.cron_jobs.get(self.state.selected_cron_job) {
                let job_id = job.id.clone();
                let job_name = job.name.clone();
                self.state.status_message = Some((format!("Triggering '{job_name}'..."), false));
                self.spawn_cron_job_trigger(&job_id);
            }
        }
    }

    fn handle_start_action(&mut self) {
        if self.state.menu_item == MenuItem::Settings {
            if self.state.gateway.connected {
                self.state.status_message = Some(("Gateway already running".to_string(), false));
            } else {
                self.state.status_message = Some(("Starting gateway...".to_string(), false));
                self.spawn_gateway_control("start");
            }
        }
    }

    fn handle_stop_action(&mut self) {
        if self.state.menu_item == MenuItem::Settings {
            if !self.state.gateway.connected {
                self.state.status_message = Some(("Gateway not running".to_string(), false));
            } else {
                self.state.status_message = Some(("Stopping gateway...".to_string(), false));
                self.spawn_gateway_control("stop");
            }
        }
    }

    fn spawn_gateway_control(&self, action: &str) {
        let tx = self.state.tx.clone();
        let action = action.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match run_gateway_control(&action).await {
                Ok(msg) => {
                    let _ = tx.send(Message::SetStatus(msg, false));
                    // Refresh gateway status after action
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    if let Ok(status) = check_gateway_status(&gw).await {
                        let _ = tx.send(Message::GatewayStatus(status));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(format!("❌ {e}"), true));
                }
            }
        });
    }

    #[allow(clippy::unused_self)]
    fn spawn_session_details(&self, _session_key: &str) {
        // TODO: Load session details from gateway API
        // For now, just show the view modal with basic info
    }

    /// Test a provider via the gateway API (POST /api/providers/{id}/test)
    fn spawn_model_test_via_gateway(&self, provider_id: &str, provider_name: &str) {
        let tx = self.state.tx.clone();
        let id = provider_id.to_string();
        let name = provider_name.to_string();
        let gw = self.state.gateway_http_url.clone();

        tokio::spawn(async move {
            match test_provider_via_gateway(&gw, &id).await {
                Ok((models_found, _)) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("✅ {name}: connection OK ({models_found} models)"),
                        false,
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(format!("❌ {name}: {e}"), true));
                }
            }
        });
    }

    /// Load message history for a session from the gateway
    fn spawn_load_session_messages(&self, session_key: &str) {
        let tx = self.state.tx.clone();
        let key = session_key.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match load_session_messages(&gw, &key).await {
                Ok(messages) => {
                    let _ = tx.send(Message::SessionMessagesLoaded(key, messages));
                }
                Err(_) => {
                    // Silently fail — session might just have no messages yet
                    let _ = tx.send(Message::SessionMessagesLoaded(key, vec![]));
                }
            }
        });
    }

    /// Called when the selected session changes — loads messages if not yet loaded
    fn on_session_selection_changed(&mut self) {
        if let Some(session) = self.state.sessions.get(self.state.selected_session) {
            let key = session.key.clone();
            // Set chat_model from the selected session, falling back to agent's model
            self.state.chat_model = session.model.clone().or_else(|| {
                if session.agent_id.is_empty() {
                    None
                } else {
                    self.state
                        .agents
                        .iter()
                        .find(|a| a.id == session.agent_id)
                        .and_then(|a| a.model.clone())
                }
            });
            // Set agent_id for agent-bound sessions
            self.state.chat_agent_id =
                if !session.agent_id.is_empty() && !session.agent_id.starts_with("ad-hoc") {
                    Some(session.agent_id.clone())
                } else {
                    None
                };
            // Load messages if not already loaded
            let already_loaded = self
                .state
                .session_chats
                .get(&key)
                .map_or(false, |c| c.loaded);
            if !already_loaded {
                self.spawn_load_session_messages(&key);
            }
        }
    }

    fn spawn_kill_session(&self, session_key: &str) {
        let tx = self.state.tx.clone();
        let key = session_key.to_string();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match kill_session(&gw, &key).await {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("✅ Session archived: {key}"),
                        false,
                    ));
                    let _ = tx.send(Message::ReloadSessions);
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("❌ Failed to archive session: {e}"),
                        true,
                    ));
                }
            }
        });
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_password_input(
        &mut self,
        key: KeyEvent,
        _masked: bool,
        action: PasswordAction,
    ) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let password = self.state.input_buffer.clone();
                self.state.clear_input();
                self.state.input_mode = InputMode::Normal;

                if password.is_empty() {
                    self.state.status_message =
                        Some(("Password cannot be empty".to_string(), true));
                    return Ok(());
                }

                match action {
                    PasswordAction::InitVault => {
                        if password.len() < 8 {
                            self.state.status_message =
                                Some(("Password must be at least 8 characters".to_string(), true));
                        } else {
                            // Initialize vault with password
                            match rockbot_credentials::CredentialVault::init_with_password(
                                &self.state.vault_path,
                                &password,
                            ) {
                                Ok(storage) => {
                                    self.vault = Some(storage);
                                    self.state.vault.initialized = true;
                                    self.state.vault.locked = false;
                                    self.state.vault.unlock_method = UnlockMethod::Password;
                                    self.state.status_message =
                                        Some(("✅ Vault initialized!".to_string(), false));
                                }
                                Err(e) => {
                                    self.state.status_message =
                                        Some((format!("❌ Init failed: {e}"), true));
                                }
                            }
                        }
                    }
                    PasswordAction::UnlockVault => {
                        // Open and unlock vault with password
                        match rockbot_credentials::CredentialVault::open(&self.state.vault_path) {
                            Ok(mut storage) => {
                                match storage.unlock_with_password(&password) {
                                    Ok(()) => {
                                        // Load endpoints after unlocking
                                        let endpoints: Vec<EndpointInfo> = storage
                                            .list_endpoints()
                                            .into_iter()
                                            .map(|e| EndpointInfo {
                                                id: e.id.to_string(),
                                                name: e.name.clone(),
                                                endpoint_type: format!("{:?}", e.endpoint_type),
                                                base_url: e.base_url.clone(),
                                                has_credential: e.credential_id
                                                    != uuid::Uuid::nil(),
                                                expiration: None,
                                            })
                                            .collect();

                                        self.vault = Some(storage);
                                        self.state.vault.locked = false;
                                        self.state.vault.endpoint_count = endpoints.len();
                                        self.state.endpoints = endpoints;
                                        self.state.status_message =
                                            Some(("✅ Vault unlocked".to_string(), false));
                                    }
                                    Err(e) => {
                                        self.state.status_message =
                                            Some((format!("❌ Wrong password: {e}"), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message =
                                    Some((format!("❌ Failed to open vault: {e}"), true));
                            }
                        }
                    }
                }
            }
            KeyCode::Esc => {
                self.state.clear_input();
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

    fn handle_add_credential(
        &mut self,
        key: KeyEvent,
        mut state: AddCredentialState,
    ) -> Result<()> {
        use crate::components::modals::ENDPOINT_TYPES;

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
            // Ctrl+S saves from any field
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.submit_add_credential(state);
            }
            KeyCode::Enter => {
                if state.is_last_field() {
                    self.submit_add_credential(state);
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

    fn submit_add_credential(&mut self, state: AddCredentialState) {
        if let Some(error) = state.validate() {
            self.state.status_message = Some((error, true));
            self.state.input_mode = InputMode::AddCredential(state);
        } else if let Some(ref mut vault) = self.vault {
            match add_credential_to_vault(vault, &state) {
                Ok(endpoint_name) => {
                    self.state.endpoints = vault
                        .list_endpoints()
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
                    // Create default permission: Any Source with HIL access
                    if let Some(ep) = self
                        .state
                        .endpoints
                        .iter()
                        .find(|e| e.name == endpoint_name)
                    {
                        use crate::state::{AccessLevel, PermissionRule, PermissionSource};
                        let next_priority = self.state.permissions.len() + 1;
                        self.state.permissions.push(PermissionRule {
                            endpoint_id: ep.id.clone(),
                            endpoint_name: ep.name.clone(),
                            source: PermissionSource::Any,
                            access: AccessLevel::AllowHil,
                            priority: next_priority,
                        });
                    }
                    self.state.status_message = Some((format!("Added: {endpoint_name}"), false));
                    self.state.input_mode = InputMode::Normal;
                }
                Err(e) => {
                    self.state.status_message = Some((format!("Failed: {e}"), true));
                    self.state.input_mode = InputMode::AddCredential(state);
                }
            }
        } else {
            self.state.status_message = Some(("Vault not unlocked".to_string(), true));
            self.state.input_mode = InputMode::AddCredential(state);
        }
    }

    fn handle_edit_credential(
        &mut self,
        key: KeyEvent,
        mut state: EditCredentialState,
    ) -> Result<()> {
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
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.submit_edit_credential(state);
            }
            KeyCode::Enter => {
                if state.is_last_field() {
                    self.submit_edit_credential(state);
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

    fn submit_edit_credential(&mut self, state: EditCredentialState) {
        if let Some(error) = state.validate() {
            self.state.status_message = Some((error, true));
            self.state.input_mode = InputMode::EditCredential(state);
        } else if let Some(ref mut vault) = self.vault {
            match update_credential_in_vault(vault, &state) {
                Ok(endpoint_name) => {
                    self.state.endpoints = vault
                        .list_endpoints()
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
                    self.state.status_message = Some((format!("Updated: {endpoint_name}"), false));
                    self.state.input_mode = InputMode::Normal;
                }
                Err(e) => {
                    self.state.status_message = Some((format!("Failed: {e}"), true));
                    self.state.input_mode = InputMode::EditCredential(state);
                }
            }
        } else {
            self.state.status_message = Some(("Vault not unlocked".to_string(), true));
            self.state.input_mode = InputMode::EditCredential(state);
        }
    }

    fn handle_edit_provider(
        &mut self,
        key: KeyEvent,
        mut state: super::state::EditProviderState,
    ) -> Result<()> {
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
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(error) = state.validate() {
                    self.state.status_message = Some((error, true));
                    self.state.input_mode = InputMode::EditProvider(state);
                } else {
                    self.save_provider_config(&state);
                    self.state.input_mode = InputMode::Normal;
                }
            }
            KeyCode::Enter => {
                if state.field_index == state.total_fields() - 1 {
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                        self.state.input_mode = InputMode::EditProvider(state);
                    } else {
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

    /// Save provider configuration — routes through gateway API
    fn save_provider_config(&mut self, state: &super::state::EditProviderState) {
        use crate::state::ProviderAuthType;

        // Save auth mode preference to config file
        self.save_provider_auth_mode(state);

        // For session key auth, just verify Claude Code credentials exist
        if state.auth_type == ProviderAuthType::SessionKey {
            if has_claude_credentials() {
                self.state.status_message = Some((
                    format!(
                        "✅ {} configured with Claude Code OAuth",
                        state.provider_name
                    ),
                    false,
                ));
            } else {
                self.state.status_message = Some((
                    "❌ Run 'claude' in terminal to authenticate with Claude Code".to_string(),
                    true,
                ));
            }
            return;
        }

        if state.auth_type == ProviderAuthType::None {
            self.state.status_message = Some((
                format!("✅ {} - no authentication needed", state.provider_name),
                false,
            ));
            return;
        }

        // Collect the secret value from form fields
        let secret_value = state.api_key(); // Checks api_key, bot_token, access_token, token, first secret field

        // For AWS credentials, also check specific field IDs
        let secret_value =
            if secret_value.is_empty() && state.auth_type == ProviderAuthType::AwsCredentials {
                // For AWS, store all secret fields as a JSON object
                let mut aws_creds = serde_json::Map::new();
                for field_id in &[
                    "access_key_id",
                    "secret_access_key",
                    "session_token",
                    "bearer_token",
                ] {
                    if let Some(val) = state.get_field_value_by_id(field_id) {
                        if !val.is_empty() {
                            aws_creds.insert(
                                field_id.to_string(),
                                serde_json::Value::String(val.to_string()),
                            );
                        }
                    }
                }
                if aws_creds.is_empty() {
                    self.state.status_message = Some((
                        format!(
                            "💡 Set AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, and AWS_REGION={}",
                            state.aws_region()
                        ),
                        false,
                    ));
                    return;
                }
                serde_json::to_string(&aws_creds).unwrap_or_default()
            } else {
                secret_value
            };

        if secret_value.is_empty() {
            if let Some(env_var) = state.env_var_hint() {
                self.state.status_message = Some((
                    format!("💡 Set {env_var} environment variable to persist credentials"),
                    false,
                ));
            }
            return;
        }

        // Determine base URL
        let base_url = if state.provider_id == "bedrock" {
            state
                .get_field_value_by_id("endpoint_url")
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
                .unwrap_or_else(|| {
                    format!(
                        "https://bedrock-runtime.{}.amazonaws.com",
                        state.aws_region()
                    )
                })
        } else {
            let url = state.base_url();
            if url.is_empty() {
                format!("{}://configured", state.provider_id)
            } else {
                url
            }
        };

        // Determine endpoint type
        let endpoint_type = match state.auth_type {
            ProviderAuthType::AwsCredentials => "api_key_service",
            ProviderAuthType::ApiKey => "api_key_service",
            _ => "api_key_service",
        };

        // Route through gateway API
        self.spawn_save_provider_credentials(
            state.provider_name.clone(),
            endpoint_type.to_string(),
            base_url,
            secret_value,
        );
    }

    /// Save provider credentials via gateway API (async)
    fn spawn_save_provider_credentials(
        &self,
        provider_name: String,
        endpoint_type: String,
        base_url: String,
        secret: String,
    ) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match save_provider_via_gateway(&gw, &provider_name, &endpoint_type, &base_url, &secret)
                .await
            {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("✅ {provider_name} credentials saved"),
                        false,
                    ));
                    // Reload providers to reflect updated availability
                    let _ = tx.send(Message::ReloadProviders);
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("❌ Failed to save {provider_name} credentials: {e}"),
                        true,
                    ));
                }
            }
        });
    }

    fn handle_edit_agent(&mut self, key: KeyEvent, mut state: EditAgentState) -> Result<()> {
        let set_mode = |s: EditAgentState| -> InputMode {
            if s.is_edit {
                InputMode::EditAgent(s)
            } else {
                InputMode::AddAgent(s)
            }
        };

        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.next_field();
                self.state.input_mode = set_mode(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.prev_field();
                self.state.input_mode = set_mode(state);
            }
            // Model picker: left/right to cycle models
            KeyCode::Left if state.is_model_picker_active() => {
                state.prev_model();
                self.state.input_mode = set_mode(state);
            }
            KeyCode::Right if state.is_model_picker_active() => {
                state.next_model();
                self.state.input_mode = set_mode(state);
            }
            // Ctrl+S saves the form from any field
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(error) = state.validate() {
                    self.state.status_message = Some((error, true));
                    self.state.input_mode = set_mode(state);
                } else if !state.is_edit && self.state.agents.iter().any(|a| a.id == state.id) {
                    self.state.status_message =
                        Some((format!("Agent '{}' already exists", state.id), true));
                    self.state.input_mode = InputMode::AddAgent(state);
                } else {
                    self.save_agent_to_config(&state);
                    self.state.input_mode = InputMode::Normal;
                }
            }
            KeyCode::Enter => {
                // System prompt field (7): Enter inserts a newline
                if state.field_index == 7 {
                    let newline_count = state.system_prompt.chars().filter(|&c| c == '\n').count();
                    if newline_count < 9 {
                        state.system_prompt.push('\n');
                    }
                    self.state.input_mode = set_mode(state);
                } else if state.is_last_field() {
                    if let Some(error) = state.validate() {
                        self.state.status_message = Some((error, true));
                        self.state.input_mode = set_mode(state);
                    } else if !state.is_edit && self.state.agents.iter().any(|a| a.id == state.id) {
                        self.state.status_message =
                            Some((format!("Agent '{}' already exists", state.id), true));
                        self.state.input_mode = InputMode::AddAgent(state);
                    } else {
                        self.save_agent_to_config(&state);
                        self.state.input_mode = InputMode::Normal;
                    }
                } else {
                    state.next_field();
                    self.state.input_mode = set_mode(state);
                }
            }
            KeyCode::Char(c) => {
                if let Some(value) = state.current_value_mut() {
                    value.push(c);
                }
                self.state.input_mode = set_mode(state);
            }
            KeyCode::Backspace => {
                if let Some(value) = state.current_value_mut() {
                    value.pop();
                }
                self.state.input_mode = set_mode(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_create_session(
        &mut self,
        key: KeyEvent,
        mut state: CreateSessionState,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.status_message = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.field_index = (state.field_index + 1) % state.total_fields();
                self.state.input_mode = InputMode::CreateSession(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                if state.field_index == 0 {
                    state.field_index = state.total_fields() - 1;
                } else {
                    state.field_index -= 1;
                }
                self.state.input_mode = InputMode::CreateSession(state);
            }
            KeyCode::Left | KeyCode::Right => {
                if state.field_index == 0 {
                    // Toggle mode
                    state.toggle_mode();
                } else {
                    // Cycle options
                    if key.code == KeyCode::Right {
                        state.next_option();
                    } else {
                        state.prev_option();
                    }
                }
                self.state.input_mode = InputMode::CreateSession(state);
            }
            KeyCode::Enter => {
                // Submit — create session and open chat
                match state.mode {
                    SessionMode::AdHoc => {
                        if let Some(model) = state.selected_model() {
                            let model = model.to_string();
                            self.spawn_create_session(None, Some(model.clone()));
                            self.state.chat_model = Some(model);
                            self.state.chat_agent_id = None;
                            self.state.input_mode = InputMode::ChatInput;
                            self.state.clear_input();
                        } else {
                            self.state.status_message =
                                Some(("No model available".to_string(), true));
                            self.state.input_mode = InputMode::CreateSession(state);
                        }
                    }
                    SessionMode::AgentBound => {
                        if let Some(agent_id) = state.selected_agent_id() {
                            let agent_id = agent_id.to_string();
                            let agent_model = self
                                .state
                                .agents
                                .iter()
                                .find(|a| a.id == agent_id)
                                .and_then(|a| a.model.clone());
                            self.spawn_create_session(Some(agent_id.clone()), agent_model.clone());
                            self.state.chat_model = agent_model;
                            self.state.chat_agent_id = Some(agent_id);
                            self.state.input_mode = InputMode::ChatInput;
                            self.state.clear_input();
                        } else {
                            self.state.status_message =
                                Some(("No agent available".to_string(), true));
                            self.state.input_mode = InputMode::CreateSession(state);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn spawn_create_session(&self, agent_id: Option<String>, model: Option<String>) {
        let tx = self.state.tx.clone();
        let gw = self.state.gateway_http_url.clone();
        tokio::spawn(async move {
            match create_session_via_gateway(&gw, agent_id.as_deref(), model.as_deref()).await {
                Ok(session_id) => {
                    let _ = tx.send(Message::SessionCreated(session_id));
                    let _ = tx.send(Message::ReloadSessions);
                }
                Err(e) => {
                    let _ = tx.send(Message::SessionCreateError(e.to_string()));
                }
            }
        });
    }

    /// Save agent via gateway API, falling back to direct config file edit
    fn save_agent_to_config(&mut self, state: &EditAgentState) {
        // Build the JSON body for the gateway API
        let mut body = serde_json::Map::new();
        if !state.is_edit {
            body.insert(
                "id".to_string(),
                serde_json::Value::String(state.id.clone()),
            );
        }
        if !state.model.is_empty() {
            body.insert(
                "model".to_string(),
                serde_json::Value::String(state.model.clone()),
            );
        }
        if !state.parent_id.is_empty() {
            body.insert(
                "parent_id".to_string(),
                serde_json::Value::String(state.parent_id.clone()),
            );
        }
        if !state.workspace.is_empty() {
            body.insert(
                "workspace".to_string(),
                serde_json::Value::String(state.workspace.clone()),
            );
        }
        if !state.max_tool_calls.is_empty() {
            if let Ok(n) = state.max_tool_calls.parse::<u32>() {
                body.insert(
                    "max_tool_calls".to_string(),
                    serde_json::Value::Number(n.into()),
                );
            }
        }
        if !state.temperature.is_empty() {
            if let Ok(t) = state.temperature.parse::<f64>() {
                if let Some(n) = serde_json::Number::from_f64(t) {
                    body.insert("temperature".to_string(), serde_json::Value::Number(n));
                }
            }
        }
        if !state.max_tokens.is_empty() {
            if let Ok(n) = state.max_tokens.parse::<u32>() {
                body.insert(
                    "max_tokens".to_string(),
                    serde_json::Value::Number(n.into()),
                );
            }
        }
        if !state.system_prompt.is_empty() {
            body.insert(
                "system_prompt".to_string(),
                serde_json::Value::String(state.system_prompt.clone()),
            );
        }
        body.insert(
            "enabled".to_string(),
            serde_json::Value::Bool(state.enabled),
        );

        let json_body = serde_json::Value::Object(body);
        let is_edit = state.is_edit;
        let agent_id = state.id.clone();
        let tx = self.state.tx.clone();
        let config_path = self.state.config_path.clone();
        let gateway_url = self.state.gateway_http_url.clone();

        // Try gateway API first, fall back to direct config file edit
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();

            let gateway_result = if is_edit {
                client
                    .put(format!("{gateway_url}/api/agents/{agent_id}"))
                    .json(&json_body)
                    .send()
                    .await
            } else {
                client
                    .post(format!("{gateway_url}/api/agents"))
                    .json(&json_body)
                    .send()
                    .await
            };

            match gateway_result {
                Ok(resp) if resp.status().is_success() || resp.status().as_u16() == 202 => {
                    let action = if is_edit { "updated" } else { "created" };
                    let _ = tx.send(Message::AgentSaved(agent_id));
                    let _ = tx.send(Message::SetStatus(format!("Agent {action}"), false));
                    let _ = tx.send(Message::ReloadAgents);
                }
                Ok(resp) => {
                    let err_text = resp.text().await.unwrap_or_default();
                    let _ = tx.send(Message::AgentSaveError(format!(
                        "Gateway error: {err_text}"
                    )));
                }
                Err(_) => {
                    // Gateway unreachable — fall back to direct config file edit
                    match save_agent_to_config_file(&config_path, &json_body, is_edit, &agent_id) {
                        Ok(()) => {
                            let action = if is_edit { "updated" } else { "created" };
                            let _ = tx.send(Message::AgentSaved(agent_id));
                            let _ = tx.send(Message::SetStatus(
                                format!("Agent {action} (offline)"),
                                false,
                            ));
                            let _ = tx.send(Message::ReloadAgents);
                        }
                        Err(e) => {
                            let _ = tx.send(Message::AgentSaveError(e.to_string()));
                        }
                    }
                }
            }
        });

        // Let the spawn handle the result — we return to normal mode immediately
    }
}

/// Direct config file save (fallback when gateway is unavailable)
fn save_agent_to_config_file(
    config_path: &PathBuf,
    json_body: &serde_json::Value,
    is_edit: bool,
    agent_id: &str,
) -> Result<()> {
    let content = std::fs::read_to_string(config_path)?;
    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse config: {e}"))?;

    if !doc.contains_key("agents") {
        doc["agents"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    if is_edit {
        if let Some(list) = doc["agents"]["list"].as_array_of_tables_mut() {
            for table in list.iter_mut() {
                if table.get("id").and_then(|v| v.as_str()) == Some(agent_id) {
                    apply_agent_fields_to_table(table, json_body);
                    break;
                }
            }
        }
    } else {
        let mut new_agent = toml_edit::Table::new();
        new_agent["id"] = toml_edit::value(agent_id);
        apply_agent_fields_to_table(&mut new_agent, json_body);

        if let Some(list) = doc["agents"]["list"].as_array_of_tables_mut() {
            list.push(new_agent);
        } else {
            let mut arr = toml_edit::ArrayOfTables::new();
            arr.push(new_agent);
            doc["agents"]["list"] = toml_edit::Item::ArrayOfTables(arr);
        }
    }

    std::fs::write(config_path, doc.to_string())?;
    Ok(())
}

/// Apply JSON fields to a toml_edit table
fn apply_agent_fields_to_table(table: &mut toml_edit::Table, json: &serde_json::Value) {
    if let Some(model) = json.get("model").and_then(|v| v.as_str()) {
        if model.is_empty() {
            table.remove("model");
        } else {
            table["model"] = toml_edit::value(model);
        }
    }
    if let Some(parent_id) = json.get("parent_id").and_then(|v| v.as_str()) {
        if parent_id.is_empty() {
            table.remove("parent_id");
        } else {
            table["parent_id"] = toml_edit::value(parent_id);
        }
    }
    if let Some(workspace) = json.get("workspace").and_then(|v| v.as_str()) {
        if workspace.is_empty() {
            table.remove("workspace");
        } else {
            table["workspace"] = toml_edit::value(workspace);
        }
    }
    if let Some(max_tool_calls) = json
        .get("max_tool_calls")
        .and_then(serde_json::Value::as_i64)
    {
        table["max_tool_calls"] = toml_edit::value(max_tool_calls);
    }
    if let Some(system_prompt) = json.get("system_prompt").and_then(|v| v.as_str()) {
        if system_prompt.is_empty() {
            table.remove("system_prompt");
        } else {
            table["system_prompt"] = toml_edit::value(system_prompt);
        }
    }
    if let Some(enabled) = json.get("enabled").and_then(serde_json::Value::as_bool) {
        table["enabled"] = toml_edit::value(enabled);
    }
}

fn contains_point(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

fn shrink_rect(rect: Rect, margin: u16) -> Rect {
    Rect {
        x: rect.x.saturating_add(margin),
        y: rect.y.saturating_add(margin),
        width: rect.width.saturating_sub(margin.saturating_mul(2)),
        height: rect.height.saturating_sub(margin.saturating_mul(2)),
    }
}

fn point_to_slider(x: u16, rect: Rect) -> f32 {
    if rect.width <= 2 {
        return 0.0;
    }
    let inner = shrink_rect(rect, 1);
    let denom = inner.width.saturating_sub(1).max(1);
    f32::from(x.saturating_sub(inner.x).min(denom)) / f32::from(denom)
}

fn point_to_wheel_hs(x: u16, y: u16, rect: Rect) -> (f32, f32) {
    let cx = rect.x as f32 + rect.width as f32 / 2.0;
    let cy = rect.y as f32 + rect.height as f32 / 2.0;
    let dx = x as f32 + 0.5 - cx;
    let dy = y as f32 + 0.5 - cy;
    let radius = rect.width.min(rect.height) as f32 / 2.0 - 1.0;
    if radius <= 0.0 {
        return (0.0, 0.0);
    }
    let saturation = ((dx * dx + dy * dy).sqrt() / radius).clamp(0.0, 1.0);
    let hue = ((dy.atan2(dx).to_degrees() + 360.0) % 360.0) / 360.0;
    (hue, saturation)
}

fn write_rgba_token(target: &mut toml_edit::Item, value: rockbot_core::RgbaColor) {
    let mut table = toml_edit::InlineTable::new();
    table.insert("r", toml_edit::Value::from(i64::from(value.r)));
    table.insert("g", toml_edit::Value::from(i64::from(value.g)));
    table.insert("b", toml_edit::Value::from(i64::from(value.b)));
    table.insert("a", toml_edit::Value::from(i64::from(value.a)));
    *target = toml_edit::value(table);
}

fn save_tui_preferences_to_config(
    config_path: &PathBuf,
    tui: &rockbot_core::TuiConfig,
) -> Result<()> {
    let content = if config_path.exists() {
        std::fs::read_to_string(config_path)?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = if content.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        content.parse()?
    };

    if !doc.contains_table("tui") {
        doc["tui"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    doc["tui"]["floating_bar"] = toml_edit::value(tui.floating_bar);
    doc["tui"]["animations"] = toml_edit::value(tui.animations);
    doc["tui"]["color_theme"] = toml_edit::value(tui.color_theme.label());
    doc["tui"]["animation_style"] = toml_edit::value(tui.animation_style.label());

    doc["tui"]["theme"] = toml_edit::Item::Table(toml_edit::Table::new());
    let theme = tui.resolved_theme();
    for token in ThemeToken::all() {
        write_rgba_token(
            &mut doc["tui"]["theme"][token.label().to_ascii_lowercase().replace(' ', "_")],
            token.value(&theme),
        );
    }

    doc["tui"]["theme"]["ai_text_color"] = doc["tui"]["theme"]["ai_text"].clone();
    doc["tui"]["theme"]["thinking_text_color"] = doc["tui"]["theme"]["thinking_text"].clone();
    doc["tui"]["theme"]["tool_text_color"] = doc["tui"]["theme"]["tool_text"].clone();
    doc["tui"]["theme"].as_table_mut().map(|table| {
        table.remove("ai_text");
        table.remove("thinking_text");
        table.remove("tool_text");
    });

    doc["tui"]["fonts"] = toml_edit::Item::Table(toml_edit::Table::new());
    let fonts = &tui.fonts;
    doc["tui"]["fonts"]["interface_font_family"] =
        toml_edit::value(fonts.interface_font_family.as_str());
    doc["tui"]["fonts"]["interface_font_size"] =
        toml_edit::value(i64::from(fonts.interface_font_size));
    doc["tui"]["fonts"]["user_font_family"] = toml_edit::value(fonts.user_font_family.as_str());
    doc["tui"]["fonts"]["user_font_size"] = toml_edit::value(i64::from(fonts.user_font_size));
    doc["tui"]["fonts"]["ai_font_family"] = toml_edit::value(fonts.ai_font_family.as_str());
    doc["tui"]["fonts"]["ai_font_size"] = toml_edit::value(i64::from(fonts.ai_font_size));
    doc["tui"]["fonts"]["thinking_font_family"] =
        toml_edit::value(fonts.thinking_font_family.as_str());
    doc["tui"]["fonts"]["thinking_font_size"] =
        toml_edit::value(i64::from(fonts.thinking_font_size));
    doc["tui"]["fonts"]["tool_font_family"] = toml_edit::value(fonts.tool_font_family.as_str());
    doc["tui"]["fonts"]["tool_font_size"] = toml_edit::value(i64::from(fonts.tool_font_size));

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, doc.to_string())?;
    Ok(())
}

impl App {
    fn handle_view_session(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                self.state.input_mode = InputMode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_view_endpoint(&mut self, key: KeyEvent, endpoint_index: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('e') => {
                self.edit_endpoint_at(endpoint_index);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_view_permission(&mut self, key: KeyEvent, permission_index: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('e') => {
                self.edit_permission_at(permission_index);
            }
            KeyCode::Char('+') | KeyCode::Char('K') => {
                // Move rule up in priority
                self.state.selected_permission = permission_index;
                self.move_permission(true);
                let new_idx = self.state.selected_permission;
                self.state.input_mode = InputMode::ViewPermission {
                    permission_index: new_idx,
                };
            }
            KeyCode::Char('-') | KeyCode::Char('J') => {
                // Move rule down in priority
                self.state.selected_permission = permission_index;
                self.move_permission(false);
                let new_idx = self.state.selected_permission;
                self.state.input_mode = InputMode::ViewPermission {
                    permission_index: new_idx,
                };
            }
            KeyCode::Char('d') => {
                if permission_index < self.state.permissions.len() {
                    let rule_name = self.state.permissions[permission_index]
                        .endpoint_name
                        .clone();
                    self.state.permissions.remove(permission_index);
                    // Renumber priorities
                    for (i, rule) in self.state.permissions.iter_mut().enumerate() {
                        rule.priority = i + 1;
                    }
                    self.state.status_message =
                        Some((format!("Removed rule for '{rule_name}'"), false));
                    self.state.input_mode = InputMode::Normal;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_view_provider(&mut self, key: KeyEvent, provider_index: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Char('e') => {
                // Switch to edit/configure modal for this provider
                self.state.input_mode = InputMode::Normal;
                if let Some(schema) = self.state.credential_schemas.get(provider_index) {
                    let edit_state = EditProviderState::from_schema(schema, provider_index);
                    self.state.input_mode = InputMode::EditProvider(edit_state);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_view_model_list(
        &mut self,
        key: KeyEvent,
        provider_index: usize,
        scroll: usize,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let model_count = self
                    .state
                    .providers
                    .get(provider_index)
                    .map_or(0, |p| p.models.len());
                if scroll + 1 < model_count {
                    self.state.input_mode = InputMode::ViewModelList {
                        provider_index,
                        scroll: scroll + 1,
                    };
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if scroll > 0 {
                    self.state.input_mode = InputMode::ViewModelList {
                        provider_index,
                        scroll: scroll - 1,
                    };
                }
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
                                            self.state.endpoints = vault
                                                .list_endpoints()
                                                .into_iter()
                                                .map(|e| EndpointInfo {
                                                    id: e.id.to_string(),
                                                    name: e.name.clone(),
                                                    endpoint_type: format!("{:?}", e.endpoint_type),
                                                    base_url: e.base_url.clone(),
                                                    has_credential: e.credential_id
                                                        != uuid::Uuid::nil(),
                                                    expiration: None,
                                                })
                                                .collect();
                                            self.state.vault.endpoint_count =
                                                self.state.endpoints.len();
                                            // Reset selection if needed
                                            if self.state.selected_endpoint
                                                >= self.state.endpoints.len()
                                            {
                                                self.state.selected_endpoint =
                                                    self.state.endpoints.len().saturating_sub(1);
                                            }
                                            self.state.status_message =
                                                Some(("✅ Deleted endpoint".to_string(), false));
                                        }
                                        Err(e) => {
                                            self.state.status_message =
                                                Some((format!("❌ Delete failed: {e}"), true));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.state.status_message =
                                        Some((format!("❌ Invalid endpoint ID: {e}"), true));
                                }
                            }
                        } else {
                            self.state.status_message =
                                Some(("❌ Vault not unlocked".to_string(), true));
                        }
                    }
                    ConfirmAction::DeleteAgent(id) => {
                        // Remove from display list (doesn't actually disable in config yet)
                        self.state.agents.retain(|a| a.id != id);
                        if self.state.selected_agent >= self.state.agents.len() {
                            self.state.selected_agent = self.state.agents.len().saturating_sub(1);
                        }
                        self.state.status_message = Some((
                            format!("Disabled agent: {id} (edit config to persist)"),
                            false,
                        ));
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
                        self.state.status_message = Some((format!("Disabled agent: {id}"), false));
                    }
                    ConfirmAction::DeleteCronJob(job_id) => {
                        self.spawn_cron_job_delete(&job_id);
                    }
                    ConfirmAction::DiscardContextFile(browser_state) => {
                        self.state.input_mode = InputMode::ViewContextFiles(browser_state);
                        return Ok(());
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

    fn handle_view_context_files(
        &mut self,
        key: KeyEvent,
        state: ViewContextFilesState,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let mut s = state;
                if s.selected > 0 {
                    s.selected -= 1;
                }
                self.state.input_mode = InputMode::ViewContextFiles(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let mut s = state;
                if s.selected + 1 < s.files.len() {
                    s.selected += 1;
                }
                self.state.input_mode = InputMode::ViewContextFiles(s);
            }
            KeyCode::Enter => {
                if let Some(file) = state.files.get(state.selected) {
                    let filename = file.name.clone();
                    let agent_id = state.agent_id.clone();
                    if file.exists {
                        // Load file content from API
                        self.fetch_context_file(&agent_id, &filename);
                    } else {
                        // Open editor with empty content for new file
                        self.state.input_mode =
                            InputMode::EditContextFile(super::state::EditContextFileState::new(
                                agent_id,
                                filename,
                                String::new(),
                            ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_edit_context_file(
        &mut self,
        key: KeyEvent,
        mut state: super::state::EditContextFileState,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                if state.is_dirty {
                    // Build browser state to return to on confirm
                    let browser = ViewContextFilesState {
                        agent_id: state.agent_id.clone(),
                        files: Vec::new(),
                        selected: 0,
                        loading: true,
                    };
                    self.state.input_mode = InputMode::Confirm {
                        message: "Discard unsaved changes?".to_string(),
                        action: ConfirmAction::DiscardContextFile(browser.clone()),
                    };
                    // Also refresh the file list so it's ready when we go back
                    self.fetch_context_files(&browser.agent_id);
                } else {
                    // Go back to file browser
                    let agent_id = state.agent_id.clone();
                    self.state.input_mode = InputMode::ViewContextFiles(ViewContextFilesState {
                        agent_id: agent_id.clone(),
                        files: Vec::new(),
                        selected: 0,
                        loading: true,
                    });
                    self.fetch_context_files(&agent_id);
                }
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.save_context_file(&state.agent_id, &state.filename, &state.content);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Enter => {
                state.insert_char('\n');
                state.ensure_cursor_visible(20);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Backspace => {
                state.delete_char();
                state.ensure_cursor_visible(20);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Up => {
                state.cursor_up();
                state.ensure_cursor_visible(20);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Down => {
                state.cursor_down();
                state.ensure_cursor_visible(20);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Left => {
                state.cursor_left();
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Right => {
                state.cursor_right();
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Home => {
                state.cursor_col = 0;
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::End => {
                let line_len = state
                    .content
                    .split('\n')
                    .nth(state.cursor_line)
                    .unwrap_or("")
                    .len();
                state.cursor_col = line_len;
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            KeyCode::Char(c) => {
                state.insert_char(c);
                state.ensure_cursor_visible(20);
                self.state.input_mode = InputMode::EditContextFile(state);
            }
            _ => {
                self.state.input_mode = InputMode::EditContextFile(state);
            }
        }
        Ok(())
    }

    fn fetch_context_files(&self, agent_id: &str) {
        let tx = self.state.tx.clone();
        let gateway_url = self.state.gateway_http_url.clone();
        let url = format!("{gateway_url}/api/agents/{}/files", agent_id);
        let agent_id = agent_id.to_string();
        tokio::spawn(async move {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(files) = resp.json::<Vec<ContextFileInfo>>().await {
                        let _ = tx.send(Message::ContextFilesLoaded(agent_id, files));
                    }
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to list files: {}",
                        resp.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to list files: {e}"
                    )));
                }
            }
        });
    }

    fn fetch_context_file(&self, agent_id: &str, filename: &str) {
        let tx = self.state.tx.clone();
        let gateway_url = self.state.gateway_http_url.clone();
        let url = format!("{gateway_url}/api/agents/{}/files/{}", agent_id, filename);
        let agent_id = agent_id.to_string();
        let filename = filename.to_string();
        tokio::spawn(async move {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        let content = json
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let _ = tx.send(Message::ContextFileLoaded(agent_id, filename, content));
                    }
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to load {filename}: {}",
                        resp.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to load {filename}: {e}"
                    )));
                }
            }
        });
    }

    fn save_context_file(&self, agent_id: &str, filename: &str, content: &str) {
        let tx = self.state.tx.clone();
        let gateway_url = self.state.gateway_http_url.clone();
        let url = format!("{gateway_url}/api/agents/{}/files/{}", agent_id, filename);
        let agent_id = agent_id.to_string();
        let filename = filename.to_string();
        let body = serde_json::json!({ "content": content });
        tokio::spawn(async move {
            let client = http_client();
            match client.put(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let _ = tx.send(Message::ContextFileSaved(agent_id, filename));
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to save {filename}: {}",
                        resp.status()
                    )));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!(
                        "Failed to save {filename}: {e}"
                    )));
                }
            }
        });
    }

    fn handle_chat_input(&mut self, key: KeyEvent) -> Result<()> {
        use crate::event::{normalize_for_text_input, InputAction};
        use crate::keybindings::TuiAction;

        // Ctrl-modified commands checked first (before normalization eats them)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('r') => {
                    self.retry_last_message();
                    return Ok(());
                }
                KeyCode::Char('t') => {
                    self.state.toggle_tool_expansion();
                    return Ok(());
                }
                KeyCode::Up => {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if chat.auto_scroll {
                            chat.scroll = chat.max_scroll.get();
                            chat.auto_scroll = false;
                        }
                        chat.scroll = chat.scroll.saturating_sub(3);
                    }
                    return Ok(());
                }
                KeyCode::Down => {
                    if let Some(chat) = self.state.active_chat_mut() {
                        if !chat.auto_scroll {
                            chat.scroll = chat.scroll.saturating_add(3);
                            if chat.scroll >= chat.max_scroll.get() {
                                chat.auto_scroll = true;
                            }
                        }
                    }
                    return Ok(());
                }
                _ => {}
            }
        }

        // Card bar / overlay keybindings (Alt+arrows, Alt+Enter, Alt+letter)
        if let Some(action) = self.keybindings.lookup("chat", &key) {
            match action {
                TuiAction::CardLeft
                | TuiAction::CardRight
                | TuiAction::CardUp
                | TuiAction::CardDown
                | TuiAction::CardActivate
                | TuiAction::OpenVault
                | TuiAction::OpenSettings
                | TuiAction::OpenModels
                | TuiAction::OpenCron => {
                    // Delegate to normal-mode handler for card/overlay actions
                    return self.handle_normal_mode(key);
                }
                _ => {} // Submit/Escape/Scroll handled below via normalization
            }
        }

        // Normalize the key event into a text-input action
        match normalize_for_text_input(key) {
            InputAction::Newline => {
                self.insert_chat_newline();
            }
            InputAction::Submit => {
                self.send_chat_buffer();
            }
            InputAction::Cancel => {
                self.state.input_mode = InputMode::Normal;
            }
            InputAction::Text(c) => {
                self.state.input_buffer.insert(self.state.input_cursor, c);
                self.state.input_cursor += c.len_utf8();
            }
            InputAction::Backspace => {
                if self.state.input_cursor > 0 {
                    let prev = self.state.input_buffer[..self.state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.state.input_buffer.remove(prev);
                    self.state.input_cursor = prev;
                }
            }
            InputAction::Delete => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            InputAction::NavLeft => {
                if self.state.input_cursor > 0 {
                    self.state.input_cursor = self.state.input_buffer[..self.state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            InputAction::NavRight => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_cursor = self.state.input_buffer[self.state.input_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.state.input_cursor + i)
                        .unwrap_or(self.state.input_buffer.len());
                }
            }
            InputAction::Home => {
                self.state.input_cursor = 0;
            }
            InputAction::End => {
                self.state.input_cursor = self.state.input_buffer.len();
            }
            InputAction::PageUp => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if chat.auto_scroll {
                        chat.scroll = chat.max_scroll.get();
                        chat.auto_scroll = false;
                    }
                    chat.scroll = chat.scroll.saturating_sub(10);
                }
            }
            InputAction::PageDown => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if !chat.auto_scroll {
                        chat.scroll = chat.scroll.saturating_add(10);
                        if chat.scroll >= chat.max_scroll.get() {
                            chat.auto_scroll = true;
                        }
                    }
                }
            }
            InputAction::NavUp | InputAction::NavDown => {
                // Up/Down in chat scrolls history (same as PageUp/PageDown but smaller)
                if let Some(chat) = self.state.active_chat_mut() {
                    match normalize_for_text_input(key) {
                        InputAction::NavUp => {
                            if chat.auto_scroll {
                                chat.scroll = chat.max_scroll.get();
                                chat.auto_scroll = false;
                            }
                            chat.scroll = chat.scroll.saturating_sub(1);
                        }
                        InputAction::NavDown => {
                            if !chat.auto_scroll {
                                chat.scroll = chat.scroll.saturating_add(1);
                                if chat.scroll >= chat.max_scroll.get() {
                                    chat.auto_scroll = true;
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            InputAction::Raw(_) => {
                // Unrecognized key — ignore in text input context
            }
        }
        Ok(())
    }

    /// Handle bracketed paste — insert text at cursor in chat input mode.
    fn handle_paste(&mut self, text: &str) {
        if !matches!(self.state.input_mode, InputMode::ChatInput) {
            return;
        }
        for c in text.chars() {
            if c == '\n' {
                self.insert_chat_newline();
            } else if !c.is_control() {
                self.state.input_buffer.insert(self.state.input_cursor, c);
                self.state.input_cursor += c.len_utf8();
            }
        }
    }

    fn insert_chat_newline(&mut self) {
        let newline_count = self
            .state
            .input_buffer
            .chars()
            .filter(|&c| c == '\n')
            .count();
        if newline_count < 9 {
            self.state
                .input_buffer
                .insert(self.state.input_cursor, '\n');
            self.state.input_cursor += 1;
        }
    }

    fn send_chat_buffer(&mut self) {
        let message = self.state.input_buffer.trim().to_string();
        if message.is_empty() {
            self.state.clear_input();
            return;
        }

        // Dispatch slash commands through the registry
        if message.starts_with('/') {
            let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
            let ctx = rockbot_chat::CommandContext {
                tx: cmd_tx,
                gateway_url: self.state.gateway_url.clone(),
                active_agent_id: self
                    .state
                    .agents
                    .get(self.state.selected_agent)
                    .map(|a| a.id.clone()),
                active_session_key: self.state.active_session_key(),
            };
            match self.command_registry.dispatch(&message, &ctx) {
                rockbot_chat::CommandResult::Handled(text) => {
                    if let Some(chat) = self.state.active_chat_mut() {
                        chat.messages.push(ChatMessage::system(text));
                        chat.auto_scroll = true;
                    }
                    self.state.clear_input();
                    return;
                }
                rockbot_chat::CommandResult::Action(action) => {
                    self.handle_command_action(action);
                    self.state.clear_input();
                    return;
                }
                rockbot_chat::CommandResult::NotHandled => {
                    // Fall through to normal chat send
                }
            }
        }

        // Check for $@agent-id routing syntax
        if let Some((agent_id, routed_msg)) = rockbot_chat::parse_agent_route(&message) {
            if let Some(chat) = self.state.active_chat_mut() {
                chat.messages
                    .push(ChatMessage::user(format!("@{agent_id}: {routed_msg}")));
                chat.loading = true;
                chat.auto_scroll = true;
            }
            self.spawn_chat_request(routed_msg.to_string());
            self.state.clear_input();
            return;
        }

        if let Some(chat) = self.state.active_chat_mut() {
            chat.messages.push(ChatMessage::user(message.clone()));
            chat.loading = true;
            chat.auto_scroll = true;
        }
        self.spawn_chat_request(message);
        self.state.clear_input();
    }

    /// Handle a CommandAction from the slash command registry.
    fn handle_command_action(&mut self, action: rockbot_chat::CommandAction) {
        use rockbot_chat::CommandAction;
        match action {
            CommandAction::Quit => {
                self.state.should_exit = true;
            }
            CommandAction::ClearChat => {
                if let Some(chat) = self.state.active_chat_mut() {
                    chat.messages.clear();
                }
            }
            CommandAction::ShowOverlay(name) => {
                if name == "help" {
                    let help_text = self
                        .command_registry
                        .list_commands()
                        .iter()
                        .map(|c| format!("  {} — {}", c.usage, c.description))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Some(chat) = self.state.active_chat_mut() {
                        chat.messages.push(ChatMessage::system(format!(
                            "Available commands:\n{help_text}"
                        )));
                        chat.auto_scroll = true;
                    }
                } else if name == "alerts" {
                    // Show alerts in chat
                    let alerts_text = if self.state.alerts.is_empty() {
                        "No alerts.".to_string()
                    } else {
                        self.state
                            .alerts
                            .iter()
                            .map(|a| format!("  [{}] {}", a.severity_str(), a.message))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    if let Some(chat) = self.state.active_chat_mut() {
                        chat.messages.push(ChatMessage::system(alerts_text));
                        chat.auto_scroll = true;
                    }
                }
            }
            CommandAction::SwitchMode(mode) => {
                let target = match mode.as_str() {
                    "dashboard" => Some(super::state::MenuItem::Dashboard),
                    "agents" => Some(super::state::MenuItem::Agents),
                    "sessions" => Some(super::state::MenuItem::Sessions),
                    "credentials" => Some(super::state::MenuItem::Credentials),
                    "cron" => Some(super::state::MenuItem::CronJobs),
                    "models" => Some(super::state::MenuItem::Models),
                    "settings" => Some(super::state::MenuItem::Settings),
                    _ => None,
                };
                if let Some(item) = target {
                    self.state.menu_item = item;
                    let agents = self.state.agents.clone();
                    let sessions = self.state.sessions.clone();
                    self.state
                        .slot_bar
                        .rebuild_content_slots(&agents, &sessions);
                } else {
                    if let Some(chat) = self.state.active_chat_mut() {
                        chat.messages.push(ChatMessage::system(format!(
                            "Unknown mode: {mode}. Try: dashboard, agents, sessions, credentials, cron, models, settings"
                        )));
                        chat.auto_scroll = true;
                    }
                }
            }
            CommandAction::SetStatus(msg, is_error) => {
                self.state.status_message = Some((msg, is_error));
            }
            CommandAction::SendToAgent(agent_id, message) => {
                // Route to specific agent — for now just show in chat
                if let Some(chat) = self.state.active_chat_mut() {
                    chat.messages
                        .push(ChatMessage::user(format!("@{agent_id}: {message}")));
                    chat.loading = true;
                    chat.auto_scroll = true;
                }
                self.spawn_chat_request(message);
            }
            CommandAction::SpawnAsync => {
                // Command spawned async work via tx — nothing to do here
            }
        }
    }

    /// Retry the last user message (removes error message and re-sends)
    fn retry_last_message(&mut self) {
        let last_user_msg = if let Some(chat) = self.state.active_chat() {
            if chat.loading {
                return; // Already processing
            }
            chat.messages
                .iter()
                .rev()
                .find(|m| m.role == super::state::ChatRole::User)
                .map(|m| m.content.clone())
        } else {
            None
        };

        if let Some(message) = last_user_msg {
            if let Some(chat) = self.state.active_chat_mut() {
                // Remove trailing error/system messages
                while chat
                    .messages
                    .last()
                    .is_some_and(|m| m.role == super::state::ChatRole::System)
                {
                    chat.messages.pop();
                }
                // Remove the previous assistant response too
                if chat
                    .messages
                    .last()
                    .is_some_and(|m| m.role == super::state::ChatRole::Assistant)
                {
                    chat.messages.pop();
                }
                chat.loading = true;
                chat.auto_scroll = true;
            }
            self.spawn_chat_request(message);
        }
    }

    /// Save provider auth mode preference to config file
    fn save_provider_auth_mode(&mut self, state: &super::state::EditProviderState) {
        use crate::state::ProviderAuthType;

        // Determine the auth mode string
        let auth_mode = match state.auth_type {
            ProviderAuthType::ApiKey => "api",
            ProviderAuthType::SessionKey => "oauth",
            ProviderAuthType::None => "none",
            ProviderAuthType::AwsCredentials => "aws",
        };

        // Provider ID is the TOML section name
        let provider_section = &state.provider_id;

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
        if !doc["providers"]
            .as_table()
            .is_some_and(|t| t.contains_key(provider_section))
        {
            doc["providers"][provider_section] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        // Set the auth_mode
        doc["providers"][provider_section]["auth_mode"] = toml_edit::value(auth_mode);

        // Also save base_url if provided and not the default from schema
        let base_url = state.base_url();
        if !base_url.is_empty() {
            let default_url = state
                .current_auth_method()
                .and_then(|m| m.fields.iter().find(|f| f.id == "base_url"))
                .and_then(|f| f.default.as_deref())
                .unwrap_or("");
            if base_url != default_url {
                doc["providers"][provider_section]["api_url"] = toml_edit::value(&base_url);
            }
        }

        // Write back to file
        if let Err(e) = std::fs::write(config_path, doc.to_string()) {
            tracing::warn!("Failed to save provider config: {}", e);
            self.state.status_message = Some((
                format!("⚠️ Auth mode set but failed to save config: {e}"),
                true,
            ));
        } else {
            tracing::info!("Saved {} auth mode: {}", provider_section, auth_mode);
        }
    }

    /// Spawn an async task to send a chat message via the gateway
    fn spawn_chat_request(&self, user_message: String) {
        let tx = self.state.tx.clone();
        let session_key = self.state.active_session_key().unwrap_or_default();
        // Resolve agent_id: chat_target > chat_agent_id > selected session's agent
        let agent_id = match &self.state.chat_target {
            ChatTarget::Agent(id) => Some(id.clone()),
            _ => None,
        }
        .or_else(|| self.state.chat_agent_id.clone())
        .or_else(|| {
            self.state
                .sessions
                .get(self.state.selected_session)
                .map(|s| &s.agent_id)
                .filter(|id| !id.is_empty() && !id.starts_with("ad-hoc"))
                .cloned()
        });
        let launch_dir = self.state.launch_dir.to_string_lossy().to_string();
        let executor_target = if self.state.allow_local_tool_execution {
            None
        } else {
            Some(
                self.state
                    .selected_executor_target
                    .clone()
                    .unwrap_or_else(|| "gateway".to_string()),
            )
        };

        // WebSocket is the only communication path — no HTTP fallback
        if !self.ws_connected() {
            let _ = tx.send(Message::ChatError(
                session_key,
                "Not connected to gateway. Check that the gateway is running.".to_string(),
            ));
            return;
        }

        let agent = match agent_id {
            Some(ref a) => a,
            None => {
                let _ = tx.send(Message::ChatError(
                    session_key,
                    "No agent selected for this session.".to_string(),
                ));
                return;
            }
        };

        let ws_msg = serde_json::json!({
            "type": "agent_message",
            "agent_id": agent,
            "session_key": session_key,
            "message": user_message,
            "workspace": launch_dir,
            "executor_target": executor_target,
            "allow_active_client_tools": self.state.allow_local_tool_execution,
        });
        if let Some(ref client) = self.gateway_client {
            let sender = client.sender();
            let session_key_clone = session_key.clone();
            let tx_clone = tx.clone();
            let msg_str = ws_msg.to_string();
            tokio::spawn(async move {
                if sender.send(msg_str).await.is_err() {
                    let _ = tx_clone.send(Message::ChatError(
                        session_key_clone,
                        "Failed to send message over WebSocket.".to_string(),
                    ));
                }
            });
        }
    }

    /// Render the entire UI
    fn render(&mut self, frame: &mut Frame) {
        // Detect modal and page transitions for tachyonfx effects
        let is_modal = !matches!(
            self.state.input_mode,
            InputMode::Normal | InputMode::ChatInput
        );
        if is_modal && !self.was_modal_open {
            self.effect_state.trigger_modal_open();
        } else if !is_modal && self.was_modal_open {
            self.effect_state.trigger_modal_close();
        }
        self.was_modal_open = is_modal;

        if self.state.menu_index != self.prev_menu_index {
            self.effect_state.trigger_page_transition();
            self.prev_menu_index = self.state.menu_index;
        }

        let full_area = frame.area();
        let bar_height = super::components::card_chain::slot_bar_height();

        // Layout: slot bar (top) + status strip (1) + main area (fill) + status bar (1)
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(bar_height),
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(1),
            ])
            .split(full_area);

        let chain_area = rows[0];
        let strip_area = rows[1];
        let main_area = rows[2];
        let status_area = rows[3];

        // Render slotted card bar
        render_slot_bar(frame, chain_area, &self.state, &self.effect_state);

        // Render status strip (contextual info between cards and content)
        self.render_card_status_strip(frame, strip_area);

        // Chat-first: always render chat in the main area
        self.render_unified_chat(frame, main_area);
        let detail_area = main_area;

        // Apply page transition effect
        let elapsed = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        self.effect_state
            .render_page_transition(frame.buffer_mut(), detail_area, elapsed);

        // Status bar
        render_status_bar(frame, status_area, self.state.status_message.as_ref());

        // Render modals on top (with background dimming and effects)
        self.render_modals(frame, elapsed);
    }

    fn render_modals(&mut self, frame: &mut Frame, elapsed: Duration) {
        // Dim background behind modals
        let is_modal = !matches!(
            self.state.input_mode,
            InputMode::Normal | InputMode::ChatInput
        );
        if is_modal {
            let area = frame.area();
            super::effects::EffectState::dim_background(
                frame.buffer_mut(),
                area,
                super::effects::palette::overlay_alpha(&self.state.tui_config),
            );
        }

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
            InputMode::CreateSession(state) => {
                super::components::render_create_session_modal(frame, frame.area(), state);
            }
            InputMode::Confirm { message, .. } => {
                render_confirm_modal(frame, frame.area(), message);
            }
            InputMode::ViewSession { session_key } => {
                render_view_session_modal(frame, frame.area(), session_key, &self.state.sessions);
            }
            InputMode::ViewEndpoint { endpoint_index } => {
                render_view_endpoint_modal(
                    frame,
                    frame.area(),
                    *endpoint_index,
                    &self.state.endpoints,
                );
            }
            InputMode::ViewProvider { provider_index } => {
                render_view_provider_modal(
                    frame,
                    frame.area(),
                    *provider_index,
                    &self.state.credential_schemas,
                    &self.state.endpoints,
                );
            }
            InputMode::ViewModelList {
                provider_index,
                scroll,
            } => {
                render_view_model_list_modal(
                    frame,
                    frame.area(),
                    *provider_index,
                    *scroll,
                    &self.state.providers,
                );
            }
            InputMode::ViewPermission { permission_index } => {
                render_view_permission_modal(
                    frame,
                    frame.area(),
                    *permission_index,
                    &self.state.permissions,
                );
            }
            InputMode::EditPermission(state) => {
                render_edit_permission_modal(frame, frame.area(), state);
            }
            InputMode::ViewContextFiles(state) => {
                render_view_context_files_modal(frame, frame.area(), state);
            }
            InputMode::EditContextFile(state) => {
                render_edit_context_file_modal(frame, frame.area(), state);
            }
            InputMode::ContextMenu(menu_state) => {
                let area = frame.area();
                super::components::context_menu::render_context_menu(frame, area, menu_state);
            }
            InputMode::CardDetail(detail) => {
                self.render_card_detail_modal(frame, detail);
            }
            InputMode::VaultOverlay => {
                super::components::overlays::render_vault_overlay(
                    frame,
                    frame.area(),
                    &self.state,
                    &self.effect_state,
                );
            }
            InputMode::SettingsOverlay => {
                super::components::overlays::render_settings_overlay(
                    frame,
                    frame.area(),
                    &self.state,
                    &self.effect_state,
                );
            }
            InputMode::ModelsOverlay {
                provider_index,
                scroll,
            } => {
                super::components::overlays::render_models_overlay(
                    frame,
                    frame.area(),
                    &self.state,
                    *provider_index,
                    *scroll,
                    &self.effect_state,
                );
            }
            InputMode::CronOverlay { scroll } => {
                super::components::overlays::render_cron_overlay(
                    frame,
                    frame.area(),
                    &self.state,
                    *scroll,
                    &self.effect_state,
                );
            }
            _ => {}
        }

        // Apply modal open/close effects on top of rendered content
        let full_area = frame.area();
        if is_modal {
            self.effect_state
                .render_modal_open(frame.buffer_mut(), full_area, elapsed);
        } else {
            self.effect_state
                .render_modal_close(frame.buffer_mut(), full_area, elapsed);
        }
    }

    /// Render the unified chat area based on current chat_target.
    ///
    /// Chat is always visible — butler, session, or agent ad-hoc.
    /// Input box renders unconditionally at the bottom.
    fn render_unified_chat(&self, frame: &mut Frame, area: Rect) {
        use super::components::render_chat_input;

        let is_chat_mode = matches!(self.state.input_mode, InputMode::ChatInput);

        // Calculate input height from wrapping
        let inner_width = area.width.saturating_sub(3).max(1) as usize;
        let visual_lines: usize = self
            .state
            .input_buffer
            .split('\n')
            .map(|line| {
                let char_count = (line.len() + 1).max(1);
                (char_count + inner_width - 1) / inner_width
            })
            .sum();
        let input_line_count = visual_lines.clamp(1, 10);
        let input_height = (input_line_count as u16) + 2;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Fill(1), Constraint::Length(input_height)])
            .split(area);

        // Messages area — dispatch per target
        match &self.state.chat_target {
            ChatTarget::Butler => {
                render_butler_main(frame, chunks[0], &self.state, &self.effect_state);
            }
            ChatTarget::Session(key) => {
                if let Some(chat) = self.state.session_chats.get(key) {
                    self.render_session_messages(frame, chunks[0], chat);
                } else {
                    render_butler_main(frame, chunks[0], &self.state, &self.effect_state);
                }
            }
            ChatTarget::Agent(id) => {
                let agent_key = format!("agent:{id}");
                if let Some(chat) = self.state.session_chats.get(&agent_key) {
                    self.render_session_messages(frame, chunks[0], chat);
                } else {
                    // Show agent welcome screen
                    let lines = vec![
                        Line::from(""),
                        Line::from(""),
                        Line::from(Span::styled(
                            format!("@{id}"),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(""),
                        Line::from(Span::styled(
                            "Type a message below to start chatting.",
                            Style::default().fg(Color::DarkGray),
                        )),
                    ];
                    let welcome = Paragraph::new(lines).alignment(Alignment::Center);
                    frame.render_widget(welcome, chunks[0]);
                }
            }
        }

        // Input box — always visible
        render_chat_input(frame, chunks[1], &self.state, is_chat_mode);
    }

    /// Render session messages (without input box) into the given area.
    fn render_session_messages(
        &self,
        frame: &mut Frame,
        area: Rect,
        chat: &crate::state::SessionChatState,
    ) {
        use super::components::render_chat_messages;

        if !chat.messages.is_empty() || chat.loading {
            render_chat_messages(
                frame,
                area,
                &self.state,
                &chat.messages,
                chat.loading,
                chat.scroll,
                chat.auto_scroll,
            );
        } else {
            let hint = Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "No messages yet. Press 'c' to chat.",
                    Style::default().fg(Color::DarkGray),
                )),
            ])
            .alignment(Alignment::Center);
            frame.render_widget(hint, area);
        }
    }

    /// Render the global persistent status strip between the card bar and main content.
    fn render_card_status_strip(&self, frame: &mut Frame, area: Rect) {
        let bg = Color::Rgb(20, 20, 40);
        let strip_style = Style::default().fg(Color::DarkGray).bg(bg);

        let gw = if self.state.gateway.connected {
            "gw:online"
        } else {
            "gw:offline"
        };
        let agents = format!("{}ag", self.state.agents.len());
        let sessions = format!("{}sess", self.state.sessions.len());
        let vault = if !self.state.vault.initialized {
            "vault:none"
        } else if self.state.vault.locked {
            "vault:locked"
        } else {
            "vault:open"
        };
        let chat_ctx = match &self.state.chat_target {
            ChatTarget::Butler => "butler".to_string(),
            ChatTarget::Session(k) => format!("->{k}"),
            ChatTarget::Agent(id) => format!("->@{id}"),
        };
        let text = format!(" {gw} | {agents} | {sessions} | {vault} | {chat_ctx}");

        let paragraph = Paragraph::new(text).style(strip_style);
        frame.render_widget(paragraph, area);
    }

    /// Render a card detail overlay modal (80% centered).
    fn render_card_detail_modal(&self, frame: &mut Frame, detail: &crate::state::CardDetailState) {
        use super::components::centered_rect;
        use ratatui::widgets::Clear;

        let area = centered_rect(80, 80, frame.area());
        frame.render_widget(Clear, area);

        let slot = self.state.slot_bar.slots.get(detail.slot_index);
        let title = slot.map(|s| s.label.as_str()).unwrap_or("Detail");

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                format!(" {title} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Per-mode detail rendering
        match detail.mode {
            MenuItem::Dashboard => self.render_dashboard_detail(frame, inner, detail),
            MenuItem::Agents => self.render_agents_detail(frame, inner, detail),
            MenuItem::Sessions => self.render_sessions_detail(frame, inner, detail),
            MenuItem::Models => self.render_models_detail(frame, inner, detail),
            MenuItem::Credentials => self.render_credentials_detail(frame, inner, detail),
            _ => self.render_generic_detail(frame, inner, detail),
        }
    }

    fn render_dashboard_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        use ratatui::widgets::Sparkline;

        let slot = self.state.slot_bar.slots.get(detail.slot_index);
        let label = slot.map(|s| s.label.as_str()).unwrap_or("");

        // Split: sparkline area (top) + stats text (bottom)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Fill(1)])
            .split(area);

        // Sparkline for gateway load / message history
        let (spark_data, spark_label): (Vec<u64>, &str) = match label {
            "Gateway" => (
                self.state.gateway_load_history.iter().copied().collect(),
                "Gateway Load",
            ),
            "Client" => (
                self.state.ws_latency_history.iter().copied().collect(),
                "WS RTT (ms)",
            ),
            "Exec" => (
                [
                    self.state.gateway_tool_exec_count,
                    self.state.local_tool_exec_count,
                    self.state.remote_tool_exec_count,
                ]
                .into_iter()
                .collect(),
                "Execution Mix",
            ),
            _ => (Vec::new(), ""),
        };

        if !spark_data.is_empty() {
            let sparkline = Sparkline::default()
                .data(&spark_data)
                .style(Style::default().fg(Color::Cyan));
            let spark_block = Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    format!(" {spark_label} "),
                    Style::default().fg(Color::DarkGray),
                ));
            let spark_inner = spark_block.inner(chunks[0]);
            frame.render_widget(spark_block, chunks[0]);
            frame.render_widget(sparkline, spark_inner);
        }

        // Stats text below sparkline
        let mut lines: Vec<Line<'_>> = Vec::new();
        match label {
            "Gateway" => {
                lines.push(Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        if self.state.gateway.connected {
                            "Online"
                        } else {
                            "Offline"
                        },
                        Style::default().fg(if self.state.gateway.connected {
                            Color::Green
                        } else {
                            Color::Red
                        }),
                    ),
                ]));
                if let Some(v) = &self.state.gateway.version {
                    lines.push(Line::from(vec![
                        Span::styled("Version: ", Style::default().fg(Color::Cyan)),
                        Span::raw(v.as_str()),
                    ]));
                }
                if let Some(up) = self.state.gateway.uptime_secs {
                    let hours = up / 3600;
                    let mins = (up % 3600) / 60;
                    lines.push(Line::from(vec![
                        Span::styled("Uptime: ", Style::default().fg(Color::Cyan)),
                        Span::raw(format!("{hours}h {mins}m")),
                    ]));
                }
                lines.push(Line::from(vec![
                    Span::styled("Sessions: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.gateway.active_sessions)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Agents: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.agents.len())),
                ]));
            }
            "Client" => {
                lines.push(Line::from(vec![
                    Span::styled("WS: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        if self.state.ws_connected {
                            "Connected"
                        } else {
                            "Disconnected"
                        },
                        Style::default().fg(if self.state.ws_connected {
                            Color::Green
                        } else {
                            Color::Yellow
                        }),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("RTT: ", Style::default().fg(Color::Cyan)),
                    Span::raw(
                        self.state
                            .ws_last_rtt_ms
                            .map(|ms| format!("{ms} ms"))
                            .unwrap_or_else(|| "--".to_string()),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Server Conns: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.gateway.active_connections)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Server Sessions: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.gateway.active_sessions)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Reconnects: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.ws_reconnect_count)),
                ]));
            }
            "Agents" => {
                for agent in &self.state.agents {
                    let status_color = match agent.status {
                        crate::state::AgentStatus::Active => Color::Green,
                        crate::state::AgentStatus::Pending => Color::Yellow,
                        crate::state::AgentStatus::Error => Color::Red,
                        crate::state::AgentStatus::Disabled => Color::DarkGray,
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("{:16}", agent.id),
                            Style::default().fg(Color::White),
                        ),
                        Span::styled(
                            format!(" {:?}", agent.status),
                            Style::default().fg(status_color),
                        ),
                        Span::styled(
                            format!("  {} sess", agent.session_count),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                if self.state.agents.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No agents configured",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            "Noise" => {
                lines.push(Line::from(vec![
                    Span::styled("WS: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        if self.state.ws_connected {
                            "connected"
                        } else {
                            "offline"
                        },
                        Style::default().fg(if self.state.ws_connected {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Noise: ", Style::default().fg(Color::Cyan)),
                    Span::styled(
                        if self.state.noise_registered {
                            "registered"
                        } else {
                            "pending"
                        },
                        Style::default().fg(if self.state.noise_registered {
                            Color::Green
                        } else {
                            Color::Yellow
                        }),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Executors: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.remote_executors.len())),
                ]));
                if let Some(message) = self.state.noise_status_message.as_deref() {
                    lines.push(Line::from(vec![
                        Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                        Span::raw(message.to_string()),
                    ]));
                }
            }
            "Exec" => {
                let active_target = if self.state.allow_local_tool_execution {
                    "active client".to_string()
                } else if let Some(target) = self.state.selected_executor_target.as_deref() {
                    self.state
                        .remote_executors
                        .iter()
                        .find(|executor| executor.target_id == target)
                        .map(RemoteExecutorInfo::display_name)
                        .unwrap_or_else(|| target.to_string())
                } else {
                    "gateway".to_string()
                };
                lines.push(Line::from(vec![
                    Span::styled("Current: ", Style::default().fg(Color::Cyan)),
                    Span::raw(active_target),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Last: ", Style::default().fg(Color::Cyan)),
                    Span::raw(
                        self.state
                            .last_tool_locality
                            .clone()
                            .unwrap_or_else(|| "idle".to_string()),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Counts: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!(
                        "local {} | gw {} | remote {}",
                        self.state.local_tool_exec_count,
                        self.state.gateway_tool_exec_count,
                        self.state.remote_tool_exec_count
                    )),
                ]));
                lines.push(Line::from(Span::styled(
                    "t toggle local  g gateway  enter select client",
                    Style::default().fg(Color::DarkGray),
                )));
                for (index, executor) in self.state.remote_executors.iter().enumerate() {
                    let selected = index == self.state.selected_executor_index;
                    let marker = if selected { ">" } else { " " };
                    let workdir = executor.working_dir.as_deref().unwrap_or("--");
                    lines.push(Line::from(vec![
                        Span::styled(marker, Style::default().fg(Color::Yellow)),
                        Span::raw(format!(" {}", executor.display_name())),
                        Span::styled(
                            format!("  {}", executor.client_type),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            format!("  {}", truncate_tool_result(workdir, 28)),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
                if self.state.remote_executors.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No remote executors registered",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            "Vault" => {
                let (status, color) = if !self.state.vault.initialized {
                    ("Not Initialized", Color::Yellow)
                } else if self.state.vault.locked {
                    ("Locked", Color::Yellow)
                } else {
                    ("Unlocked", Color::Green)
                };
                lines.push(Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                    Span::styled(status, Style::default().fg(color)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("Endpoints: ", Style::default().fg(Color::Cyan)),
                    Span::raw(format!("{}", self.state.vault.endpoint_count)),
                ]));
            }
            _ => {
                lines.push(Line::from("No additional details available."));
            }
        }

        let text_area = if spark_data.is_empty() {
            area
        } else {
            chunks[1]
        };
        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, text_area);
    }

    fn render_agents_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        let mut lines: Vec<Line<'_>> = Vec::new();
        if let Some(agent) = self.state.agents.get(detail.slot_index.saturating_sub(1)) {
            let status_color = match agent.status {
                crate::state::AgentStatus::Active => Color::Green,
                crate::state::AgentStatus::Pending => Color::Yellow,
                crate::state::AgentStatus::Error => Color::Red,
                crate::state::AgentStatus::Disabled => Color::DarkGray,
            };
            lines.push(Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                Span::raw(&agent.id),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Model: ", Style::default().fg(Color::Cyan)),
                Span::raw(agent.model.as_deref().unwrap_or("default")),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(agent.status.label(), Style::default().fg(status_color)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Sessions: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}", agent.session_count)),
            ]));
            if let Some(ref ws) = agent.workspace {
                lines.push(Line::from(vec![
                    Span::styled("Workspace: ", Style::default().fg(Color::Cyan)),
                    Span::raw(ws.as_str()),
                ]));
            }
            if let Some(ref parent) = agent.parent_id {
                lines.push(Line::from(vec![
                    Span::styled("Parent: ", Style::default().fg(Color::Cyan)),
                    Span::styled(parent.as_str(), Style::default().fg(Color::Yellow)),
                ]));
            }
            // Subagents
            let subagents: Vec<&str> = self
                .state
                .agents
                .iter()
                .filter(|a| a.parent_id.as_deref() == Some(&agent.id))
                .map(|a| a.id.as_str())
                .collect();
            if !subagents.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Subagents: ", Style::default().fg(Color::Cyan)),
                    Span::raw(subagents.join(", ")),
                ]));
            }
            if let Some(ref prompt) = agent.system_prompt {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "System Prompt:",
                    Style::default().fg(Color::Cyan),
                )));
                for line in prompt.lines().take(10) {
                    lines.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
                if prompt.lines().count() > 10 {
                    lines.push(Line::from(Span::styled(
                        "  ...",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        } else {
            lines.push(Line::from("No agent selected."));
        }

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_sessions_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        let mut lines: Vec<Line<'_>> = Vec::new();
        if let Some(session) = self.state.sessions.get(detail.slot_index.saturating_sub(1)) {
            lines.push(Line::from(vec![
                Span::styled("Session: ", Style::default().fg(Color::Cyan)),
                Span::raw(&session.key),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Agent: ", Style::default().fg(Color::Cyan)),
                Span::raw(&session.agent_id),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Messages: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}", session.message_count)),
            ]));
            if let Some(ref model) = session.model {
                lines.push(Line::from(vec![
                    Span::styled("Model: ", Style::default().fg(Color::Cyan)),
                    Span::raw(model.as_str()),
                ]));
            }
            // Show recent messages preview
            if let Some(chat) = self.state.session_chats.get(&session.key) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Recent Messages:",
                    Style::default().fg(Color::Cyan),
                )));
                for msg in chat
                    .messages
                    .iter()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    let role_color = match msg.role {
                        crate::state::ChatRole::User => Color::Green,
                        crate::state::ChatRole::Assistant => Color::Cyan,
                        crate::state::ChatRole::System => Color::Yellow,
                    };
                    let preview: String = msg.content.chars().take(60).collect();
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {:?}: ", msg.role),
                            Style::default().fg(role_color),
                        ),
                        Span::raw(preview),
                    ]));
                }
            }
        } else {
            lines.push(Line::from("No session selected."));
        }

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_models_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        let mut lines: Vec<Line<'_>> = Vec::new();
        let idx = detail.slot_index.saturating_sub(1);
        if let Some(provider) = self.state.providers.get(idx) {
            let status_color = if provider.available {
                Color::Green
            } else {
                Color::Red
            };
            lines.push(Line::from(vec![Span::styled(
                &provider.name,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    if provider.available {
                        "Available"
                    } else {
                        "Not Available"
                    },
                    Style::default().fg(status_color),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Auth: ", Style::default().fg(Color::Cyan)),
                Span::raw(&provider.auth_type),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Models ({}):", provider.models.len()),
                Style::default().fg(Color::Cyan),
            )));
            for model in &provider.models {
                let tokens_info = model.max_output_tokens.map_or_else(
                    || format!(" ({}k ctx)", model.context_window / 1000),
                    |t| format!(" ({}k ctx, {}k out)", model.context_window / 1000, t / 1000),
                );
                lines.push(Line::from(vec![
                    Span::styled("  • ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&model.name, Style::default().fg(Color::White)),
                    Span::styled(tokens_info, Style::default().fg(Color::DarkGray)),
                ]));
            }
        } else {
            lines.push(Line::from("No provider selected."));
        }

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_credentials_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        let mut lines: Vec<Line<'_>> = Vec::new();
        let slot = self.state.slot_bar.slots.get(detail.slot_index);
        let label = slot.map(|s| s.label.as_str()).unwrap_or("");
        match label {
            "Endpoints" => {
                lines.push(Line::from(Span::styled(
                    "Stored Credentials",
                    Style::default().fg(Color::Cyan),
                )));
                lines.push(Line::from(""));
                for ep in &self.state.endpoints {
                    lines.push(Line::from(vec![
                        Span::styled("  • ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&ep.name),
                    ]));
                }
                if self.state.endpoints.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "  No endpoints configured",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            "Providers" => {
                lines.push(Line::from(Span::styled(
                    "Credential Schemas",
                    Style::default().fg(Color::Cyan),
                )));
                lines.push(Line::from(""));
                for schema in &self.state.credential_schemas {
                    lines.push(Line::from(vec![
                        Span::styled("  • ", Style::default().fg(Color::DarkGray)),
                        Span::raw(&schema.provider_name),
                        Span::styled(
                            format!(" ({} methods)", schema.auth_methods.len()),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]));
                }
            }
            _ => {
                lines.push(Line::from("Detail view for this tab."));
            }
        }

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, area);
    }

    fn render_generic_detail(
        &self,
        frame: &mut Frame,
        area: Rect,
        detail: &crate::state::CardDetailState,
    ) {
        let lines = vec![Line::from("Detail view for this mode.")];
        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll as u16, 0));
        frame.render_widget(paragraph, area);
    }
}

/// Add a credential to the vault based on form state (standalone to avoid borrow issues)
fn add_credential_to_vault(
    vault: &mut rockbot_credentials::CredentialVault,
    state: &AddCredentialState,
) -> Result<String> {
    use rockbot_credentials::{CredentialType, EndpointType};

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
            let header_name = state
                .get_field_value("header_name")
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
    let base_url = state.get_field_value("url").unwrap_or("").to_string();

    // Create endpoint
    let endpoint = vault.create_endpoint(state.name.clone(), endpoint_type, base_url)?;

    // Store credential
    vault.store_credential(endpoint.id, credential_type, &secret_data)?;

    Ok(state.name.clone())
}

/// Update a credential in the vault based on edit form state
fn update_credential_in_vault(
    vault: &mut rockbot_credentials::CredentialVault,
    state: &EditCredentialState,
) -> Result<String> {
    let endpoint_id = uuid::Uuid::parse_str(&state.endpoint_id)?;

    // Get the existing endpoint
    let mut endpoint = vault.get_endpoint(endpoint_id)?.clone();

    // Update endpoint metadata
    endpoint.name = state.name.clone();
    endpoint.base_url = state
        .get_field_value("url")
        .unwrap_or(&state.base_url)
        .to_string();
    endpoint.updated_at = chrono::Utc::now();

    vault.update_endpoint(endpoint.clone())?;

    // If secret was modified, rotate the credential
    if state.secret_modified && endpoint.credential_id != uuid::Uuid::nil() {
        let secret_data = match state.endpoint_type {
            0 | 1 | 5 => {
                // Home Assistant / Generic REST / Bearer Token
                state
                    .get_field_value("token")
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec()
            }
            2 => {
                // OAuth2 Service
                state
                    .get_field_value("client_secret")
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec()
            }
            3 => {
                // API Key Service
                state
                    .get_field_value("api_key")
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec()
            }
            4 => {
                // Basic Auth
                state
                    .get_field_value("password")
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec()
            }
            _ => vec![],
        };

        if !secret_data.is_empty() {
            vault.rotate_credential(endpoint.credential_id, &secret_data)?;
        }
    }

    Ok(state.name.clone())
}

/// Run the main async TUI event loop.
///
/// Uses crossterm's `EventStream` for async terminal input (not poll/read),
/// a `TerminalGuard` for RAII cleanup, and a unified `AppEvent` bus.
pub async fn run_app(config_path: PathBuf, vault_path: PathBuf, gateway_url: String) -> Result<()> {
    use crate::event::{spawn_terminal_input, AppEvent, TerminalGuard};

    // TerminalGuard owns raw mode, alternate screen, keyboard enhancement,
    // bracketed paste, and focus change. Cleaned up on drop (including panic).
    let (_guard, mut terminal) = TerminalGuard::enter()?;

    // Create and initialize app
    let mut app = App::new(config_path, vault_path, gateway_url);
    app.init().await?;

    // Unified event channel — terminal input task sends AppEvents here
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<AppEvent>();
    spawn_terminal_input(event_tx);

    // Tick interval for animations and periodic updates
    let mut tick_interval = tokio::time::interval(Duration::from_millis(100));

    // Periodic refresh interval
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(15));

    // Main async event loop — all event sources are proper async streams
    loop {
        terminal.draw(|frame| {
            app.render(frame);
        })?;

        tokio::select! {
            // Terminal events (key, paste, resize, focus) from EventStream task
            Some(evt) = event_rx.recv() => {
                match evt {
                    AppEvent::Key(key) => app.handle_key(key)?,
                    AppEvent::Paste(text) => app.handle_paste(&text),
                    AppEvent::Mouse(mouse) => {
                        let area = terminal.size()?;
                        app.handle_mouse(mouse, Rect::new(0, 0, area.width, area.height))?;
                    }
                    AppEvent::Resize(_w, _h) => {
                        // ratatui handles resize automatically on next draw
                    }
                    AppEvent::FocusGained | AppEvent::FocusLost => {
                        // Could track focus state if needed
                    }
                    // These variants are only used if routed through the event bus
                    AppEvent::Tick | AppEvent::Msg(_) | AppEvent::Gateway(_) => {}
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
                app.handle_message(Message::Tick);
            }

            // Gateway events from WebSocket
            event = async {
                if let Some(ref mut rx) = app.gateway_events_rx {
                    rx.recv().await.ok()
                } else {
                    std::future::pending::<Option<rockbot_client::GatewayEvent>>().await
                }
            } => {
                if let Some(event) = event {
                    let ws_rtt_ms = if matches!(event, rockbot_client::GatewayEvent::Pong) {
                        app.last_ws_ping_sent_at
                            .take()
                            .map(|started| started.elapsed().as_millis() as u64)
                    } else {
                        None
                    };
                    handle_gateway_event(&app.state.tx, app.gateway_client.as_ref(), &event, ws_rtt_ms).await;
                }
            }

            // Periodic refresh (health check, reconnect)
            _ = refresh_interval.tick() => {
                if app.ws_connected() {
                    app.spawn_remote_executors_load();
                    if let Some(ref client) = app.gateway_client {
                        app.last_ws_ping_sent_at = Some(Instant::now());
                        let sender = client.sender();
                        tokio::spawn(async move {
                            let _ = sender.send(r#"{"type":"ping"}"#.to_string()).await;
                            let _ = sender.send(r#"{"type":"health_check"}"#.to_string()).await;
                        });
                    }
                } else {
                    app.last_ws_ping_sent_at = None;
                    if !app.state.gateway_loading {
                        app.spawn_gateway_check();
                    }
                    if app.state.gateway.connected && app.gateway_client.as_ref().is_none_or(|c| !c.is_connected()) {
                        app.spawn_ws_connect();
                    }
                }
            }
        }

        if app.state.should_exit {
            break;
        }
    }

    // _guard drops here, restoring terminal state
    Ok(())
}

// =============================================================================
// Background task implementations
// =============================================================================

/// Initiate Noise Protocol handshake — send step 1 (client -> server).
#[cfg(feature = "remote-exec")]
async fn initiate_noise_handshake(sender: &rockbot_client::GatewaySender) -> Result<()> {
    let client_key = rockbot_client::remote_exec::generate_keypair()
        .map_err(|e| anyhow::anyhow!("Noise keypair generation failed: {e}"))?;
    let mut initiator = rockbot_client::remote_exec::create_initiator(&client_key)
        .map_err(|e| anyhow::anyhow!("Noise initiator creation failed: {e}"))?;

    let mut buf = vec![0u8; 65535];
    let len = initiator
        .write_message(&[], &mut buf)
        .map_err(|e| anyhow::anyhow!("Noise step 1 write failed: {e}"))?;

    let payload_b64 = base64_encode_simple(&buf[..len]);
    let msg = serde_json::json!({
        "type": "noise_handshake",
        "payload": payload_b64,
        "step": 1
    });
    sender
        .send(serde_json::to_string(&msg)?)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to send Noise step 1: {e}"))?;

    // Store the handshake state + key for step 3
    {
        let mut guard = NOISE_HANDSHAKE_STATE.lock().await;
        *guard = Some((initiator, client_key));
    }

    Ok(())
}

/// In-progress Noise handshake state (client side).
#[cfg(feature = "remote-exec")]
static NOISE_HANDSHAKE_STATE: tokio::sync::Mutex<
    Option<(
        rockbot_client::remote_exec::HandshakeState,
        rockbot_client::remote_exec::Keypair,
    )>,
> = tokio::sync::Mutex::const_new(None);

/// Whether the Noise session is established (remote exec active).
#[cfg(feature = "remote-exec")]
static NOISE_SESSION_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Simple base64 encoding for Noise payloads.
#[cfg(feature = "remote-exec")]
fn base64_encode_simple(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        output.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        output.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

/// Simple base64 decoding for Noise payloads.
#[cfg(feature = "remote-exec")]
fn base64_decode_simple(input: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let input = input.trim_end_matches('=');
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in input.bytes() {
        let val = ALPHABET
            .iter()
            .position(|&b| b == c)
            .ok_or_else(|| anyhow::anyhow!("invalid base64 character"))? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(output)
}

/// Handle Noise handshake step 2 response from the server.
#[cfg(feature = "remote-exec")]
async fn handle_noise_step2(sender: &rockbot_client::GatewaySender, payload_b64: &str) {
    let payload = match base64_decode_simple(payload_b64) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Invalid base64 in Noise step 2: {e}");
            return;
        }
    };

    let mut state = NOISE_HANDSHAKE_STATE.lock().await;
    let Some((ref mut initiator, _)) = *state else {
        tracing::warn!("Received Noise step 2 but no handshake in progress");
        return;
    };

    let mut buf = vec![0u8; 65535];
    if let Err(e) = initiator.read_message(&payload, &mut buf) {
        tracing::warn!("Noise step 2 read failed: {e}");
        state.take();
        return;
    }

    // Write step 3 (client -> server)
    let len = match initiator.write_message(&[], &mut buf) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("Noise step 3 write failed: {e}");
            state.take();
            return;
        }
    };

    let payload_b64 = base64_encode_simple(&buf[..len]);
    let msg = serde_json::json!({
        "type": "noise_handshake",
        "payload": payload_b64,
        "step": 3
    });
    if sender
        .send(serde_json::to_string(&msg).unwrap_or_default())
        .await
        .is_err()
    {
        tracing::warn!("Failed to send Noise step 3");
        state.take();
        return;
    }

    tracing::info!("Noise handshake step 3 sent — awaiting capabilities ack");

    if initiator.is_handshake_finished() {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let caps_msg = serde_json::json!({
            "type": "remote_capabilities",
            "capabilities": ["filesystem", "shell", "network"],
            "client_type": "tui",
            "working_dir": cwd,
        });
        let _ = sender
            .send(serde_json::to_string(&caps_msg).unwrap_or_default())
            .await;
        tracing::info!("Sent TUI capabilities (filesystem, shell, network)");
        NOISE_SESSION_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    state.take();
}

/// Execute a tool locally on behalf of the remote gateway.
#[cfg(feature = "remote-exec")]
async fn handle_remote_tool_request(
    sender: &rockbot_client::GatewaySender,
    request_id: &str,
    tool_name: &str,
    params: &str,
    agent_id: &str,
    session_id: &str,
    workspace: &str,
) {
    tracing::info!("Executing remote tool locally: {tool_name} (request={request_id})");
    let start = std::time::Instant::now();

    if tool_name == "exec" {
        if let Err(error_output) =
            execute_streaming_remote_exec(sender, request_id, params, workspace, start).await
        {
            send_tool_response(
                sender,
                request_id,
                false,
                &format!("Tool execution error: {error_output}"),
                start.elapsed(),
            )
            .await;
        }
        return;
    }

    let tool_config = rockbot_tools::ToolConfig {
        profile: "standard".to_string(),
        deny: vec![],
        configs: std::collections::HashMap::new(),
    };
    let registry = match rockbot_tools::ToolRegistry::new(tool_config).await {
        Ok(r) => r,
        Err(e) => {
            send_tool_response(
                sender,
                request_id,
                false,
                &format!("Failed to create tool registry: {e}"),
                start.elapsed(),
            )
            .await;
            return;
        }
    };

    let workspace_path = std::path::PathBuf::from(workspace);
    let mut capabilities = rockbot_security::Capabilities::new();
    capabilities.add(rockbot_security::Capability::FilesystemRead(
        workspace_path.clone(),
    ));
    capabilities.add(rockbot_security::Capability::FilesystemWrite(
        workspace_path.clone(),
    ));
    capabilities.add(rockbot_security::Capability::ProcessExecute);
    capabilities.add(rockbot_security::Capability::FilesystemRead(
        std::path::PathBuf::from("/"),
    ));
    capabilities.add(rockbot_security::Capability::FilesystemWrite(
        std::path::PathBuf::from("/"),
    ));

    let context = rockbot_tools::ToolExecutionContext {
        session_id: session_id.to_string(),
        agent_id: agent_id.to_string(),
        workspace_path,
        security_context: rockbot_security::SecurityContext {
            session_id: "remote-exec".to_string(),
            capabilities,
            sandbox_enabled: false,
            restrictions: rockbot_security::SecurityRestrictions::default(),
        },
        credential_accessor: None,
        command_allowlist: vec![],
        approval_callback: None,
        agent_invoker: None,
        delegation_depth: 0,
        blackboard: None,
        swarm_id: None,
    };

    match registry.execute_tool(tool_name, params, context).await {
        Ok(result) => {
            let output = match &result.result {
                rockbot_tools::message::ToolResult::Text { content } => content.clone(),
                rockbot_tools::message::ToolResult::Json { data } => {
                    serde_json::to_string(data).unwrap_or_default()
                }
                rockbot_tools::message::ToolResult::Error { message, .. } => message.clone(),
                rockbot_tools::message::ToolResult::File { path, .. } => format!("[File: {path}]"),
                rockbot_tools::message::ToolResult::Handoff { .. } => {
                    "[Handoff — not applicable for remote exec]".to_string()
                }
            };
            send_tool_response(sender, request_id, result.success, &output, start.elapsed()).await;
        }
        Err(e) => {
            send_tool_response(
                sender,
                request_id,
                false,
                &format!("Tool execution error: {e}"),
                start.elapsed(),
            )
            .await;
        }
    }
}

#[cfg(feature = "remote-exec")]
async fn execute_streaming_remote_exec(
    sender: &rockbot_client::GatewaySender,
    request_id: &str,
    params: &str,
    workspace: &str,
    start: std::time::Instant,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let parsed: serde_json::Value =
        serde_json::from_str(params).map_err(|e| format!("Invalid exec params: {e}"))?;
    let command = parsed
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "command is required".to_string())?
        .to_string();
    let workdir = parsed
        .get("workdir")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(workspace)
        .to_string();
    let timeout_secs = parsed
        .get("timeout")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(30);

    let mut child = tokio::process::Command::new("sh");
    child
        .arg("-c")
        .arg(&command)
        .current_dir(&workdir)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = child
        .spawn()
        .map_err(|e| format!("Failed to execute command: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture stderr".to_string())?;

    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();

    let stdout_tx = chunk_tx.clone();
    let stdout_handle = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stdout_tx.send(("stdout".to_string(), format!("{line}\n")));
        }
    });

    let stderr_tx = chunk_tx.clone();
    let stderr_handle = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = stderr_tx.send(("stderr".to_string(), format!("{line}\n")));
        }
    });

    drop(chunk_tx);

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let mut wait_fut = Box::pin(child.wait());
    let deadline = std::time::Duration::from_secs(timeout_secs);
    let sleep = tokio::time::sleep(deadline);
    tokio::pin!(sleep);
    let exit_status = loop {
        tokio::select! {
            maybe_chunk = chunk_rx.recv() => {
                if let Some((stream, chunk)) = maybe_chunk {
                    if stream == "stderr" {
                        stderr_buf.push_str(&chunk);
                    } else {
                        stdout_buf.push_str(&chunk);
                    }
                    send_tool_output(sender, request_id, Some(&stream), &chunk).await;
                }
            }
            status = &mut wait_fut => {
                let status = status.map_err(|e| format!("Failed to wait for command: {e}"))?;
                break status;
            }
            _ = &mut sleep => {
                return Err("Command timed out".to_string());
            }
        }
    };

    while let Some((stream, chunk)) = chunk_rx.recv().await {
        if stream == "stderr" {
            stderr_buf.push_str(&chunk);
        } else {
            stdout_buf.push_str(&chunk);
        }
        send_tool_output(sender, request_id, Some(&stream), &chunk).await;
    }

    let _ = stdout_handle.await;
    let _ = stderr_handle.await;

    let output = serde_json::json!({
        "exit_code": exit_status.code().unwrap_or(-1),
        "stdout": stdout_buf,
        "stderr": stderr_buf,
        "success": exit_status.success()
    });

    send_tool_response(
        sender,
        request_id,
        exit_status.success(),
        &serde_json::to_string(&output).unwrap_or_default(),
        start.elapsed(),
    )
    .await;

    Ok(())
}

#[cfg(feature = "remote-exec")]
async fn send_tool_output(
    sender: &rockbot_client::GatewaySender,
    request_id: &str,
    stream: Option<&str>,
    output: &str,
) {
    let resp = serde_json::json!({
        "type": "remote_tool_output",
        "request_id": request_id,
        "stream": stream,
        "output": output,
    });
    let payload = serde_json::to_string(&resp).unwrap_or_default();
    if let Err(e) = sender.send(payload).await {
        tracing::warn!("Failed to send remote tool output: {e} (request={request_id})");
    }
}

#[cfg(feature = "remote-exec")]
async fn send_tool_response(
    sender: &rockbot_client::GatewaySender,
    request_id: &str,
    success: bool,
    output: &str,
    elapsed: std::time::Duration,
) {
    let resp = serde_json::json!({
        "type": "remote_tool_response",
        "request_id": request_id,
        "success": success,
        "output": output,
        "execution_time_ms": elapsed.as_millis() as u64,
    });
    let payload = serde_json::to_string(&resp).unwrap_or_default();

    // Retry with backoff if WS is temporarily disconnected (e.g. during reconnect)
    let mut attempts = 0u32;
    loop {
        match sender.send(payload.clone()).await {
            Ok(()) => {
                tracing::info!(
                    "Remote tool response sent: request={request_id}, success={success}, time={}ms",
                    elapsed.as_millis()
                );
                return;
            }
            Err(e) => {
                attempts += 1;
                if attempts >= 10 {
                    tracing::error!("Failed to send remote tool response after {attempts} attempts: {e} (request={request_id})");
                    return;
                }
                tracing::warn!("WS disconnected, retrying remote tool response (attempt {attempts}/10, request={request_id})");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

/// Map a GatewayEvent from rockbot-client into TUI Messages.
async fn handle_gateway_event(
    tx: &mpsc::UnboundedSender<Message>,
    client: Option<&rockbot_client::GatewayClient>,
    event: &rockbot_client::GatewayEvent,
    ws_rtt_ms: Option<u64>,
) {
    use rockbot_client::GatewayEvent;
    match event {
        GatewayEvent::StreamChunk { session_key, delta } => {
            let _ = tx.send(Message::ChatStreamChunk(format!("{session_key}:{delta}")));
        }
        GatewayEvent::ToolCall {
            tool_name,
            locality: _,
        } => {
            let _ = tx.send(Message::SetStatus(
                format!("Running: {tool_name}..."),
                false,
            ));
        }
        GatewayEvent::ToolResult {
            session_key,
            tool_name,
            result,
            success,
            duration_ms,
            locality,
        } => {
            let status = if *success { "✓" } else { "✗" };
            let locality_suffix = locality
                .as_ref()
                .map(|value| format!(", executed on: {value}"))
                .unwrap_or_default();
            let _ = tx.send(Message::SetStatus(
                format!("{status} {tool_name} ({duration_ms}ms{locality_suffix})"),
                !success,
            ));
            let _ = tx.send(Message::ToolExecutionObserved {
                locality: locality.clone(),
            });
            if !result.is_empty() && !session_key.is_empty() {
                let prefix = locality
                    .as_ref()
                    .map(|value| format!("\n[{tool_name} | executed on: {value}]\n"))
                    .unwrap_or_else(|| format!("\n[{tool_name}]\n"));
                let _ = tx.send(Message::ChatStreamChunk(format!(
                    "{session_key}:{}{result}",
                    prefix
                )));
            }
        }
        GatewayEvent::AgentResponse {
            session_key,
            content,
            tool_calls,
            tokens_used,
            processing_time_ms,
        } => {
            let tui_tool_calls: Vec<ToolCallInfo> = tool_calls
                .iter()
                .map(|tc| ToolCallInfo {
                    tool_name: tc.tool_name.clone(),
                    arguments: String::new(),
                    result: truncate_tool_result(&tc.result, 500),
                    success: tc.success,
                    duration_ms: tc.duration_ms,
                    locality: tc.locality.clone(),
                    expanded: false,
                })
                .collect();

            if tui_tool_calls.is_empty() {
                let _ = tx.send(Message::ChatResponse(session_key.clone(), content.clone()));
            } else {
                let _ = tx.send(Message::ChatAgentResponse(
                    session_key.clone(),
                    content.clone(),
                    tui_tool_calls,
                ));
            }

            if let Some(tokens) = tokens_used {
                if tokens.total_tokens > 0 {
                    let status = if let Some(time_ms) = processing_time_ms {
                        format!(
                            "Tokens: {} ({} prompt + {} completion) | {time_ms}ms",
                            tokens.total_tokens, tokens.prompt_tokens, tokens.completion_tokens
                        )
                    } else {
                        format!(
                            "Tokens: {} ({} prompt + {} completion)",
                            tokens.total_tokens, tokens.prompt_tokens, tokens.completion_tokens
                        )
                    };
                    let _ = tx.send(Message::SetStatus(status, false));
                }
            }
        }
        GatewayEvent::AgentError { session_key, error } => {
            let _ = tx.send(Message::ChatError(session_key.clone(), error.clone()));
        }
        GatewayEvent::TokenUsage {
            session_key,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cumulative_total,
        } => {
            let _ = tx.send(Message::ChatTokenUsage {
                session_key: session_key.clone(),
                prompt_tokens: *prompt_tokens,
                completion_tokens: *completion_tokens,
                total_tokens: *total_tokens,
                cumulative_total: *cumulative_total,
            });
        }
        GatewayEvent::ThinkingStatus {
            session_key,
            phase,
            tool_name,
            iteration,
        } => {
            let _ = tx.send(Message::ChatThinkingStatus {
                session_key: session_key.clone(),
                phase: phase.clone(),
                tool_name: tool_name.clone(),
                iteration: *iteration,
            });
        }
        GatewayEvent::Pong => {
            if let Some(rtt_ms) = ws_rtt_ms {
                let _ = tx.send(Message::WsLatencySample(rtt_ms));
            }
        }
        GatewayEvent::HealthStatus {
            version,
            uptime_secs,
            active_connections,
            active_sessions,
            pending_agents,
            ..
        } => {
            let gateway_status = super::state::GatewayStatus {
                connected: true,
                version: version.clone(),
                uptime_secs: *uptime_secs,
                active_connections: *active_connections,
                active_sessions: *active_sessions,
                pending_agents: *pending_agents,
            };
            let _ = tx.send(Message::GatewayStatus(gateway_status));
        }
        GatewayEvent::Connected => {
            tracing::info!("GatewayClient connected");
            let _ = tx.send(Message::WsConnectionChanged {
                connected: true,
                reason: None,
            });
            if let Some(client) = client {
                let sender = client.sender();
                tokio::spawn(async move {
                    let hostname = std::env::var("HOSTNAME")
                        .ok()
                        .or_else(|| std::env::var("COMPUTERNAME").ok())
                        .unwrap_or_else(|| "unknown-host".to_string());
                    let identify_msg = serde_json::json!({
                        "type": "client_identify",
                        "hostname": hostname,
                    });
                    let _ = sender.send(identify_msg.to_string()).await;
                });
            }
        }
        GatewayEvent::ClientIdentityAssigned {
            client_uuid,
            hostname,
            label,
        } => {
            let display = label.clone().unwrap_or_else(|| hostname.clone());
            let _ = tx.send(Message::SetStatus(
                format!("Connected as {display} ({client_uuid})"),
                false,
            ));
        }
        GatewayEvent::Disconnected { reason } => {
            let _ = tx.send(Message::WsConnectionChanged {
                connected: false,
                reason: Some(reason.clone()),
            });
            let _ = tx.send(Message::SetStatus(
                format!("WebSocket disconnected: {reason}"),
                false,
            ));
        }
        GatewayEvent::Error { message } => {
            tracing::warn!("Gateway WebSocket error: {message}");
        }
        #[cfg(feature = "remote-exec")]
        GatewayEvent::NoiseHandshakeStep { step, payload } => {
            if *step == 2 {
                if let Some(c) = client {
                    let sender = c.sender();
                    handle_noise_step2(&sender, payload).await;
                }
            }
        }
        #[cfg(feature = "remote-exec")]
        GatewayEvent::RemoteCapabilitiesAck { accepted, message } => {
            let _ = tx.send(Message::NoiseStatusChanged {
                registered: *accepted,
                message: message.clone(),
            });
            if *accepted {
                tracing::info!("Remote execution registered: {message}");
            } else {
                tracing::warn!("Remote execution rejected: {message}");
            }
        }
        #[cfg(feature = "remote-exec")]
        GatewayEvent::RemoteToolRequest {
            request_id,
            tool_name,
            params,
            agent_id,
            session_id,
            workspace_path,
        } => {
            if let Some(c) = client {
                let sender = c.sender();
                let request_id = request_id.clone();
                let tool_name = tool_name.clone();
                let params = params.clone();
                let agent_id = agent_id.clone();
                let session_id = session_id.clone();
                let workspace_path = workspace_path.clone();
                tokio::spawn(async move {
                    handle_remote_tool_request(
                        &sender,
                        &request_id,
                        &tool_name,
                        &params,
                        &agent_id,
                        &session_id,
                        &workspace_path,
                    )
                    .await;
                });
            }
        }
        // When remote-exec is not enabled, ignore these events
        #[cfg(not(feature = "remote-exec"))]
        GatewayEvent::NoiseHandshakeStep { .. }
        | GatewayEvent::RemoteCapabilitiesAck { .. }
        | GatewayEvent::RemoteToolRequest { .. } => {}
    }
}

use crate::state::{
    AgentInfo, AgentStatus, AuthMethodInfo, CredentialFieldInfo, CredentialSchemaInfo, CronJobInfo,
    GatewayStatus, ModelProvider, ModelProviderModel, RemoteExecutorInfo, VaultStatus,
};

/// Build a reqwest client that accepts self-signed TLS certificates.
///
/// All TUI HTTP calls go through this so that self-signed gateway certs work.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

async fn check_gateway_status(gateway_url: &str) -> Result<GatewayStatus> {
    use tokio::time::timeout;

    // Try to fetch actual status from the gateway API
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| http_client());
    let status_result = timeout(
        Duration::from_secs(3),
        client.get(format!("{gateway_url}/api/status")).send(),
    )
    .await;

    match status_result {
        Ok(Ok(response)) if response.status().is_success() => {
            // Parse the JSON response
            if let Ok(json) = response.json::<serde_json::Value>().await {
                return Ok(GatewayStatus {
                    connected: true,
                    version: json
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    uptime_secs: json.get("uptime_secs").and_then(serde_json::Value::as_u64),
                    active_connections: json
                        .get("active_connections")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize,
                    active_sessions: json
                        .get("active_sessions")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize,
                    pending_agents: json
                        .get("pending_agents")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize,
                });
            }
            // Connected but couldn't parse response
            Ok(GatewayStatus {
                connected: true,
                version: Some("unknown".to_string()),
                uptime_secs: None,
                active_connections: 0,
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
                active_connections: 0,
                active_sessions: 0,
                pending_agents: 0,
            })
        }
    }
}

async fn load_remote_executors_from_gateway(gateway_url: &str) -> Result<Vec<RemoteExecutorInfo>> {
    let response = http_client()
        .get(format!("{gateway_url}/api/executors"))
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("gateway returned {}", response.status());
    }
    Ok(response.json::<Vec<RemoteExecutorInfo>>().await?)
}

/// Load agents from the gateway API, falling back to the config file if the gateway is unreachable.
async fn load_agents(config_path: &PathBuf, gateway_url: &str) -> Result<Vec<AgentInfo>> {
    // Try loading from gateway first
    if let Ok(agents) = load_agents_from_gateway(gateway_url).await {
        if !agents.is_empty() {
            return Ok(agents);
        }
    }

    // Fallback: read from config file directly
    load_agents_from_config(config_path).await
}

/// Load agents from the gateway's /api/agents endpoint
async fn load_agents_from_gateway(gateway_url: &str) -> Result<Vec<AgentInfo>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let resp = client
        .get(format!("{gateway_url}/api/agents"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }

    let items: Vec<serde_json::Value> = resp.json().await?;
    let mut agents = Vec::new();

    for entry in &items {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let model = entry
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from);
        let parent_id = entry
            .get("parent_id")
            .and_then(|v| v.as_str())
            .map(String::from);
        let system_prompt = entry
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(String::from);
        let workspace = entry
            .get("workspace")
            .and_then(|v| v.as_str())
            .map(String::from);
        let max_tool_calls = entry
            .get("max_tool_calls")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as u32);
        let temperature = entry
            .get("temperature")
            .and_then(serde_json::Value::as_f64)
            .map(|n| n as f32);
        let max_tokens = entry
            .get("max_tokens")
            .and_then(serde_json::Value::as_u64)
            .map(|n| n as u32);
        let enabled = entry
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let session_count = entry
            .get("session_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;

        let status = match entry.get("status").and_then(|v| v.as_str()) {
            Some("active") => AgentStatus::Active,
            Some("pending") => AgentStatus::Pending,
            Some("error") => AgentStatus::Error,
            Some("disabled") => AgentStatus::Disabled,
            _ if !enabled => AgentStatus::Disabled,
            _ => AgentStatus::Active,
        };

        agents.push(AgentInfo {
            id,
            model,
            status,
            session_count,
            parent_id,
            system_prompt,
            workspace,
            max_tool_calls,
            temperature,
            max_tokens,
            enabled,
        });
    }

    Ok(agents)
}

/// Load agents from the TOML config file (fallback when gateway is unavailable)
async fn load_agents_from_config(config_path: &PathBuf) -> Result<Vec<AgentInfo>> {
    let content = tokio::fs::read_to_string(config_path).await?;
    let doc: toml::Value = content
        .parse()
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));

    let mut agents = Vec::new();

    if let Some(agents_table) = doc.get("agents") {
        if let Some(list) = agents_table.get("list").and_then(|v| v.as_array()) {
            for entry in list {
                let id = entry
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }

                let model = entry
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let parent_id = entry
                    .get("parent_id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let system_prompt = entry
                    .get("system_prompt")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let workspace = entry
                    .get("workspace")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let max_tool_calls = entry
                    .get("max_tool_calls")
                    .and_then(toml::Value::as_integer)
                    .map(|n| n as u32);
                let temperature = entry
                    .get("temperature")
                    .and_then(toml::Value::as_float)
                    .map(|n| n as f32);
                let max_tokens = entry
                    .get("max_tokens")
                    .and_then(toml::Value::as_integer)
                    .map(|n| n as u32);
                let enabled = entry
                    .get("enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true);

                let status = if enabled {
                    AgentStatus::Active
                } else {
                    AgentStatus::Disabled
                };

                agents.push(AgentInfo {
                    id,
                    model,
                    status,
                    session_count: 0,
                    parent_id,
                    system_prompt,
                    workspace,
                    max_tool_calls,
                    temperature,
                    max_tokens,
                    enabled,
                });
            }
        }
    }

    if agents.is_empty() {
        agents.push(AgentInfo {
            id: "default".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            status: AgentStatus::Active,
            session_count: 0,
            parent_id: None,
            system_prompt: None,
            workspace: None,
            max_tool_calls: None,
            temperature: Some(0.3),
            max_tokens: Some(16000),
            enabled: true,
        });
    }

    Ok(agents)
}

async fn check_vault_status(vault_path: &PathBuf) -> Result<VaultStatus> {
    use rockbot_credentials::CredentialVault;

    // Debug logging
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/rockbot_debug.log")
    {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: path={vault_path:?}");
    }

    let exists = CredentialVault::exists(vault_path);

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/rockbot_debug.log")
    {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: exists={exists}");
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
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/rockbot_debug.log")
            {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: raw unlock_method={method:?}");
            }
            match method {
                Some(rockbot_credentials::UnlockMethod::Password { .. }) => UnlockMethod::Password,
                Some(rockbot_credentials::UnlockMethod::Keyfile { path_hint }) => {
                    UnlockMethod::Keyfile {
                        path: path_hint.clone(),
                    }
                }
                Some(rockbot_credentials::UnlockMethod::Age { public_key, .. }) => {
                    UnlockMethod::Age {
                        public_key: Some(public_key.clone()),
                    }
                }
                Some(rockbot_credentials::UnlockMethod::SshKey {
                    public_key_path, ..
                }) => UnlockMethod::SshKey {
                    path: Some(public_key_path.clone()),
                },
                None => UnlockMethod::Unknown,
            }
        }
        Err(e) => {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/rockbot_debug.log")
            {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: open error={e:?}");
            }
            UnlockMethod::Unknown
        }
    };

    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/rockbot_debug.log")
    {
        use std::io::Write;
        let _ = writeln!(
            f,
            "check_vault_status: final unlock_method={unlock_method:?}"
        );
    }

    Ok(VaultStatus {
        enabled: true,
        initialized: true,
        locked: true, // Assume locked until unlocked
        endpoint_count: 0,
        unlock_method,
    })
}

/// Send a chat message via the gateway
/// Truncate a tool result string for display
/// Load cron jobs from the gateway API
async fn load_cron_jobs_from_gateway(gateway_url: &str) -> Result<Vec<CronJobInfo>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let resp = client
        .get(format!("{gateway_url}/api/cron/jobs"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }

    let items: Vec<serde_json::Value> = resp.json().await?;
    let mut jobs = Vec::new();

    for entry in &items {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let schedule_val = entry.get("schedule");
        let schedule_str = match schedule_val
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
        {
            Some("cron") => schedule_val
                .and_then(|s| s.get("expression"))
                .and_then(|e| e.as_str())
                .unwrap_or("?")
                .to_string(),
            Some("every") => {
                let ms = schedule_val
                    .and_then(|s| s.get("interval_ms"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if ms >= 3_600_000 {
                    format!("every {}h", ms / 3_600_000)
                } else if ms >= 60_000 {
                    format!("every {}m", ms / 60_000)
                } else {
                    format!("every {}s", ms / 1000)
                }
            }
            Some("at") => {
                let at_ms = schedule_val
                    .and_then(|s| s.get("at_ms"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                format!("once @{at_ms}")
            }
            _ => "unknown".to_string(),
        };

        let state_val = entry.get("state");
        let last_run = state_val
            .and_then(|s| s.get("last_run_at_ms"))
            .and_then(|v| v.as_u64())
            .map(|ms| {
                chrono::DateTime::from_timestamp_millis(ms as i64)
                    .map(|dt| dt.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{ms}"))
            });
        let last_status = state_val
            .and_then(|s| s.get("last_run_status"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let next_run = state_val
            .and_then(|s| s.get("next_run_at_ms"))
            .and_then(|v| v.as_u64())
            .map(|ms| {
                chrono::DateTime::from_timestamp_millis(ms as i64)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{ms}"))
            });

        jobs.push(CronJobInfo {
            id,
            name: entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            enabled: entry
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            agent_id: entry
                .get("agent_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            schedule: schedule_str,
            last_run,
            last_status,
            next_run,
        });
    }

    Ok(jobs)
}

async fn toggle_cron_job(gateway_url: &str, job_id: &str, enabled: bool) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .put(format!("{gateway_url}/api/cron/jobs/{job_id}"))
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }
    Ok(())
}

async fn delete_cron_job(gateway_url: &str, job_id: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .delete(format!("{gateway_url}/api/cron/jobs/{job_id}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }
    Ok(())
}

async fn trigger_cron_job(gateway_url: &str, job_id: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .post(format!("{gateway_url}/api/cron/jobs/{job_id}/trigger"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }
    Ok(())
}

fn truncate_tool_result(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Run gateway control command (start/stop/restart)
async fn run_gateway_control(action: &str) -> Result<String> {
    use tokio::process::Command;

    let output = Command::new("rockbot")
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
        Err(anyhow::anyhow!("Gateway {action} failed: {stderr}"))
    }
}

/// Test a provider connection via the gateway API
async fn test_provider_via_gateway(gateway_url: &str, provider_id: &str) -> Result<(u64, String)> {
    let client = http_client();
    let response = client
        .post(format!("{gateway_url}/api/providers/{provider_id}/test"))
        .timeout(Duration::from_secs(10))
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    let status = body["status"].as_str().unwrap_or("error");
    if status == "ok" {
        let models = body["models_found"].as_u64().unwrap_or(0);
        Ok((models, provider_id.to_string()))
    } else {
        let error = body["error"].as_str().unwrap_or("Unknown error");
        Err(anyhow::anyhow!("{error}"))
    }
}

/// Kill a session via gateway API
async fn kill_session(gateway_url: &str, session_key: &str) -> Result<()> {
    let client = http_client();
    let response = client
        .delete(format!("{gateway_url}/api/sessions/{session_key}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if response.status().is_success() || response.status().as_u16() == 404 {
        // 404 means session already gone, which is fine
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Failed to kill session: {}",
            response.status()
        ))
    }
}

/// Load message history for a session from the gateway
async fn load_session_messages(gateway_url: &str, session_key: &str) -> Result<Vec<ChatMessage>> {
    use tokio::time::timeout;

    let client = http_client();
    let result = timeout(
        Duration::from_secs(3),
        client
            .get(format!("{gateway_url}/api/sessions/{session_key}/messages"))
            .send(),
    )
    .await;

    match result {
        Ok(Ok(response)) if response.status().is_success() => {
            let json: serde_json::Value = response.json().await?;
            let messages = json
                .get("messages")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let msg = m.get("message")?;
                            let content = msg.get("content")?;
                            // Content can be a string or an object with "text" field
                            let text = if let Some(s) = content.as_str() {
                                s.to_string()
                            } else if let Some(t) = content.get("text").and_then(|v| v.as_str()) {
                                t.to_string()
                            } else {
                                return None;
                            };
                            let role_str = content
                                .get("role")
                                .or_else(|| msg.get("role"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("user");
                            let role = match role_str {
                                "assistant" => super::state::ChatRole::Assistant,
                                "system" => super::state::ChatRole::System,
                                _ => super::state::ChatRole::User,
                            };
                            let timestamp = msg
                                .get("created_at")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            Some(ChatMessage {
                                role,
                                content: text,
                                timestamp,
                                tool_calls: Vec::new(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(messages)
        }
        _ => Ok(vec![]),
    }
}

/// Simple base64 encoding (standard alphabet)
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Save provider credentials via gateway API
async fn save_provider_via_gateway(
    gateway_url: &str,
    provider_name: &str,
    endpoint_type: &str,
    base_url: &str,
    secret: &str,
) -> Result<()> {
    let client = http_client();

    // Step 1: Create endpoint
    let ep_response = client
        .post(format!("{gateway_url}/api/credentials/endpoints"))
        .json(&serde_json::json!({
            "name": provider_name,
            "endpoint_type": endpoint_type,
            "base_url": base_url,
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if !ep_response.status().is_success() {
        let status = ep_response.status();
        let body = ep_response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Failed to create endpoint ({status}): {body}"
        ));
    }

    let ep: serde_json::Value = ep_response.json().await?;
    let ep_id = ep["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No endpoint ID in response"))?;

    // Step 2: Store credential (base64 encoded)
    let encoded_secret = base64_encode(secret.as_bytes());
    let cred_response = client
        .post(format!(
            "{gateway_url}/api/credentials/endpoints/{ep_id}/credential"
        ))
        .json(&serde_json::json!({
            "credential_type": "bearer_token",
            "secret": encoded_secret,
        }))
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    if !cred_response.status().is_success() {
        let status = cred_response.status();
        let body = cred_response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Failed to store credential ({status}): {body}"
        ));
    }

    Ok(())
}

/// Load providers from the gateway API
async fn load_providers_from_gateway(gateway_url: &str) -> Result<Vec<ModelProvider>> {
    use tokio::time::timeout;

    let client = http_client();
    let result = timeout(
        Duration::from_secs(2),
        client.get(format!("{gateway_url}/api/providers")).send(),
    )
    .await;

    match result {
        Ok(Ok(response)) if response.status().is_success() => {
            let json: serde_json::Value = response.json().await?;
            let providers = json
                .get("providers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            let id = p.get("id")?.as_str()?.to_string();
                            let name = p.get("name")?.as_str()?.to_string();
                            let available = p
                                .get("available")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            let auth_type = p
                                .get("auth_type")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let supports_streaming = p
                                .get("supports_streaming")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            let supports_tools = p
                                .get("supports_tools")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);
                            let supports_vision = p
                                .get("supports_vision")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false);

                            let models = p
                                .get("models")
                                .and_then(|v| v.as_array())
                                .map(|models| {
                                    models
                                        .iter()
                                        .filter_map(|m| {
                                            Some(ModelProviderModel {
                                                id: m.get("id")?.as_str()?.to_string(),
                                                name: m.get("name")?.as_str()?.to_string(),
                                                description: m
                                                    .get("description")?
                                                    .as_str()?
                                                    .to_string(),
                                                context_window: m.get("context_window")?.as_u64()?
                                                    as u32,
                                                max_output_tokens: m
                                                    .get("max_output_tokens")
                                                    .and_then(serde_json::Value::as_u64)
                                                    .map(|v| v as u32),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            Some(ModelProvider {
                                id,
                                name,
                                available,
                                auth_type,
                                models,
                                supports_streaming,
                                supports_tools,
                                supports_vision,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(providers)
        }
        _ => Ok(vec![]),
    }
}

/// Load credential schemas from the gateway API
async fn load_credential_schemas(gateway_url: &str) -> Result<Vec<CredentialSchemaInfo>> {
    use tokio::time::timeout;

    let client = http_client();
    let result = timeout(
        Duration::from_secs(2),
        client
            .get(format!("{gateway_url}/api/credentials/schemas"))
            .send(),
    )
    .await;

    match result {
        Ok(Ok(response)) if response.status().is_success() => {
            let json: serde_json::Value = response.json().await?;
            let schemas = json
                .get("schemas")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| {
                            let provider_id = s.get("provider_id")?.as_str()?.to_string();
                            let provider_name = s.get("provider_name")?.as_str()?.to_string();
                            let category = s.get("category")?.as_str()?.to_string();

                            let auth_methods = s
                                .get("auth_methods")
                                .and_then(|v| v.as_array())
                                .map(|methods| {
                                    methods
                                        .iter()
                                        .filter_map(|m| {
                                            Some(AuthMethodInfo {
                                                id: m.get("id")?.as_str()?.to_string(),
                                                label: m.get("label")?.as_str()?.to_string(),
                                                fields: m
                                                    .get("fields")
                                                    .and_then(|v| v.as_array())
                                                    .map(|fields| {
                                                        fields
                                                            .iter()
                                                            .filter_map(|f| {
                                                                Some(CredentialFieldInfo {
                                                                    id: f.get("id")?.as_str()?.to_string(),
                                                                    label: f.get("label")?.as_str()?.to_string(),
                                                                    secret: f.get("secret").and_then(serde_json::Value::as_bool).unwrap_or(false),
                                                                    default: f.get("default").and_then(|v| v.as_str()).map(String::from),
                                                                    placeholder: f.get("placeholder").and_then(|v| v.as_str()).map(String::from),
                                                                    required: f.get("required").and_then(serde_json::Value::as_bool).unwrap_or(true),
                                                                    env_var: f.get("env_var").and_then(|v| v.as_str()).map(String::from),
                                                                })
                                                            })
                                                            .collect()
                                                    })
                                                    .unwrap_or_default(),
                                                hint: m.get("hint").and_then(|v| v.as_str()).map(String::from),
                                                docs_url: m.get("docs_url").and_then(|v| v.as_str()).map(String::from),
                                            })
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            Some(CredentialSchemaInfo {
                                provider_id,
                                provider_name,
                                category,
                                auth_methods,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(schemas)
        }
        _ => Ok(vec![]),
    }
}

/// Load sessions from the gateway API
async fn load_sessions_from_gateway(gateway_url: &str) -> Result<Vec<super::state::SessionInfo>> {
    use tokio::time::timeout;

    let client = http_client();
    let result = timeout(
        Duration::from_secs(2),
        client.get(format!("{gateway_url}/api/sessions")).send(),
    )
    .await;

    match result {
        Ok(Ok(response)) if response.status().is_success() => {
            let json: serde_json::Value = response.json().await?;
            let sessions = if let Some(arr) = json.as_array() {
                arr.iter()
                    .filter_map(|s| {
                        let id = s.get("id")?.as_str()?.to_string();
                        let agent_id = s.get("agent_id")?.as_str()?.to_string();
                        let session_key = s.get("session_key")?.as_str()?.to_string();
                        let created_at = s
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let model = s
                            .get("metadata")
                            .and_then(|m| m.get("model"))
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let channel = if agent_id == "ad-hoc" {
                            model.as_ref().map(|m| format!("model:{m}"))
                        } else {
                            Some(format!("agent:{agent_id}"))
                        };

                        Some(super::state::SessionInfo {
                            key: id,
                            agent_id: if agent_id == "ad-hoc" {
                                format!("ad-hoc ({})", session_key.get(..8).unwrap_or(&session_key))
                            } else {
                                agent_id
                            },
                            channel,
                            started_at: created_at,
                            message_count: 0,
                            model,
                        })
                    })
                    .collect()
            } else {
                vec![]
            };
            Ok(sessions)
        }
        _ => Ok(vec![]),
    }
}

/// Create a session via the gateway API
async fn create_session_via_gateway(
    gateway_url: &str,
    agent_id: Option<&str>,
    model: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut body = serde_json::Map::new();
    if let Some(id) = agent_id {
        body.insert(
            "agent_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
    }
    if let Some(m) = model {
        body.insert(
            "model".to_string(),
            serde_json::Value::String(m.to_string()),
        );
    }

    let response = client
        .post(format!("{gateway_url}/api/sessions"))
        .json(&serde_json::Value::Object(body))
        .send()
        .await?;

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await?;
        let session_id = json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        Ok(session_id)
    } else {
        let err_text = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("Gateway error: {err_text}"))
    }
}

/// Render the butler chat as the main view area.
///
/// Shows butler conversation messages or a welcome screen if no messages yet.
fn render_butler_main(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let chat = &state.butler_chat;

    if chat.messages.is_empty() && !chat.loading {
        // Welcome screen
        let lines = vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                "Butler",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Your RockBot companion. Sassy. Helpful. Queer.",
                Style::default().fg(Color::Gray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Navigate with the card strip above, or type a message below.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let welcome = Paragraph::new(lines).alignment(Alignment::Center);
        frame.render_widget(welcome, area);
        return;
    }

    // Render butler chat messages using the shared chat renderer
    super::components::render_chat_messages(
        frame,
        area,
        state,
        &chat.messages,
        chat.loading,
        chat.scroll,
        chat.auto_scroll,
    );
}
