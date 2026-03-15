//! Main RockBot TUI application with async event handling
//!
//! Uses tokio::select! for responsive concurrent event + background task handling.

use anyhow::Result;
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders},
    Frame,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

use super::components::{
    render_add_credential_modal, render_confirm_modal, render_dashboard,
    render_agents, render_credentials, render_edit_credential_modal, render_edit_provider_modal,
    render_edit_agent_modal,
    render_cron_jobs, render_models, render_password_modal, render_sessions, render_settings, render_sidebar,
    render_status_bar, render_view_session_modal,
    render_view_endpoint_modal, render_view_provider_modal,
    render_view_model_list_modal, render_edit_permission_modal,
    render_view_permission_modal,
    render_view_context_files_modal, render_edit_context_file_modal,
};
use super::effects::EffectState;
use super::state::{
    AddCredentialState, AppState, ChatMessage, ConfirmAction, ContextFileInfo, CreateSessionState,
    EditAgentState, EditCredentialState, EditProviderState, EndpointInfo, InputMode, MenuItem,
    Message, PasswordAction, SessionMode, ToolCallInfo, UnlockMethod, ViewContextFilesState,
};

/// Check if Claude Code OAuth credentials are available
pub fn has_claude_credentials() -> bool {
    #[cfg(feature = "anthropic")]
    { rockbot_llm::AnthropicProvider::has_credentials() }
    #[cfg(not(feature = "anthropic"))]
    { false }
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
    vault: Option<rockbot_credentials::CredentialVault>,
    /// WebSocket sender for the persistent gateway connection (None if not connected)
    ws_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// Pending oneshot receiver — the WS task sends the channel here once connected
    ws_pending_rx: Option<tokio::sync::oneshot::Receiver<tokio::sync::mpsc::UnboundedSender<String>>>,
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
            ws_tx: None,
            ws_pending_rx: None,
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
        self.spawn_providers_load();
        self.spawn_cron_jobs_load();
        self.spawn_credential_schemas_load();
        self.spawn_sessions_load();
        self.spawn_ws_connect();
        Ok(())
    }

    /// Spawn a background task that connects to the gateway via WebSocket.
    ///
    /// The connection is established *before* we set `ws_tx`, so that
    /// `ws_connected()` only returns `true` when the socket is actually open.
    /// If the connection fails, a retry loop keeps trying every 5 seconds
    /// until it succeeds or the app shuts down.
    fn spawn_ws_connect(&mut self) {
        // If there is already an active WS, don't spawn again
        if self.ws_connected() {
            return;
        }

        let tx = self.state.tx.clone();
        // ws_ready_tx lets the spawned task hand back the send channel once
        // the WebSocket connection is actually established.
        let (ws_ready_tx, ws_ready_rx) = tokio::sync::oneshot::channel::<
            tokio::sync::mpsc::UnboundedSender<String>,
        >();
        // We clear the old channel immediately so ws_connected() returns false
        // while we are still connecting.
        self.ws_tx = None;

        // Store the receiver so the main loop can pick up the channel once
        // the connection succeeds.
        self.ws_pending_rx = Some(ws_ready_rx);

        tokio::spawn(async move {
            let ws_url = "ws://127.0.0.1:18080/ws";

            // Retry loop: try to connect with back-off
            let mut attempt = 0u32;
            let ws_stream = loop {
                attempt += 1;
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    tokio_tungstenite::connect_async(ws_url),
                ).await {
                    Ok(Ok((stream, _))) => {
                        tracing::info!("WebSocket connected to gateway (attempt {attempt})");
                        break stream;
                    }
                    Ok(Err(e)) => {
                        tracing::debug!("WebSocket connect attempt {attempt} failed: {e}");
                    }
                    Err(_) => {
                        tracing::debug!("WebSocket connect attempt {attempt} timed out");
                    }
                }
                // After 6 failed attempts (~30s total), give up and let the
                // periodic refresh interval trigger a new `spawn_ws_connect`.
                if attempt >= 6 {
                    tracing::debug!("WebSocket gave up after {attempt} attempts, will retry later");
                    let _ = tx.send(Message::SetStatus(
                        "WebSocket unavailable, using HTTP polling".to_string(), false,
                    ));
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };

            use futures_util::{SinkExt, StreamExt};
            let (mut ws_sink, mut ws_source) = ws_stream.split();

            // Create the outbound channel *now* that we know we are connected
            let (ws_send_tx, mut ws_send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let _ = ws_ready_tx.send(ws_send_tx);

            loop {
                tokio::select! {
                    // Outbound: messages from the app to send over WebSocket
                    outbound = ws_send_rx.recv() => {
                        match outbound {
                            Some(text) => {
                                use tokio_tungstenite::tungstenite::Message as WsMsg;
                                if ws_sink.send(WsMsg::Text(text)).await.is_err() {
                                    tracing::warn!("WebSocket send failed, disconnecting");
                                    break;
                                }
                            }
                            None => break, // Channel closed
                        }
                    }
                    // Inbound: messages from the gateway
                    inbound = ws_source.next() => {
                        match inbound {
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                handle_ws_response(&tx, &text);
                            }
                            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => {
                                tracing::info!("WebSocket closed by server");
                                break;
                            }
                            Some(Err(e)) => {
                                tracing::debug!("WebSocket error: {e}");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }

            tracing::info!("WebSocket disconnected");
        });
    }

    /// Check if a WebSocket connection is active
    fn ws_connected(&self) -> bool {
        self.ws_tx.as_ref().is_some_and(|tx| !tx.is_closed())
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

    /// Spawn a task to load cron jobs from gateway
    fn spawn_cron_jobs_load(&mut self) {
        let tx = self.state.tx.clone();
        self.state.cron_loading = true;
        tokio::spawn(async move {
            match load_cron_jobs_from_gateway().await {
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
        tokio::spawn(async move {
            match toggle_cron_job(&job_id, enabled).await {
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
        tokio::spawn(async move {
            match delete_cron_job(&job_id).await {
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
        tokio::spawn(async move {
            match trigger_cron_job(&job_id).await {
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

    /// Spawn a task to load credential schemas from gateway
    fn spawn_credential_schemas_load(&self) {
        let tx = self.state.tx.clone();
        tokio::spawn(async move {
            match load_credential_schemas().await {
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
        tokio::spawn(async move {
            match load_providers_from_gateway().await {
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
        tokio::spawn(async move {
            match load_sessions_from_gateway().await {
                Ok(sessions) => {
                    let _ = tx.send(Message::SessionsLoaded(sessions));
                }
                Err(e) => {
                    let _ = tx.send(Message::SessionsError(e.to_string()));
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

        self.state.update(msg);
    }

    /// Auto-unlock a keyfile-protected vault (no user interaction needed)
    fn auto_unlock_keyfile_vault(&mut self, path_hint: Option<String>) {
        let keyfile_path = path_hint.or_else(|| {
            dirs::config_dir().map(|d| d.join("rockbot").join("vault.key").to_string_lossy().to_string())
        });

        if let Some(kf_path) = keyfile_path {
            let kf_pathbuf = std::path::PathBuf::from(&kf_path);
            if kf_pathbuf.exists() {
                match rockbot_credentials::CredentialVault::open(&self.state.vault_path) {
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
                                self.state.status_message = Some((format!("❌ Auto-unlock failed: {e}"), true));
                            }
                        }
                    }
                    Err(e) => {
                        self.state.status_message = Some((format!("❌ Failed to open vault: {e}"), true));
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
            InputMode::ViewModelList { provider_index, scroll } => {
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
                self.state.sidebar_focus = true;
                self.effect_state.set_active(false);
            }
            // Left/Right for horizontal card navigation in content panes
            KeyCode::Left | KeyCode::Char('h') if !self.state.sidebar_focus => {
                self.state.select_prev();
                if self.state.menu_item == MenuItem::Sessions {
                    self.on_session_selection_changed();
                }
            }
            KeyCode::Right | KeyCode::Char('l') if !self.state.sidebar_focus => {
                self.state.select_next();
                if self.state.menu_item == MenuItem::Sessions {
                    self.on_session_selection_changed();
                }
            }
            // Up/Down for list navigation within credential tabs / chat scroll
            KeyCode::Up | KeyCode::Char('k') if !self.state.sidebar_focus => {
                match self.state.menu_item {
                    MenuItem::Credentials => self.state.credential_list_prev(),
                    MenuItem::Sessions => {
                        if let Some(chat) = self.state.active_chat_mut() {
                            if chat.auto_scroll {
                                // Transition: start from bottom
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
                }
            }
            KeyCode::Down | KeyCode::Char('j') if !self.state.sidebar_focus => {
                match self.state.menu_item {
                    MenuItem::Credentials => self.state.credential_list_next(),
                    MenuItem::Sessions => {
                        if let Some(chat) = self.state.active_chat_mut() {
                            if chat.auto_scroll {
                                // Already at bottom in auto-scroll
                                return Ok(());
                            }
                            chat.scroll = chat.scroll.saturating_add(1);
                            // If we've scrolled to the bottom, re-enable auto-scroll
                            if chat.scroll >= chat.max_scroll.get() {
                                chat.auto_scroll = true;
                            }
                        }
                    }
                    MenuItem::CronJobs => {
                        if !self.state.cron_jobs.is_empty() {
                            self.state.selected_cron_job = (self.state.selected_cron_job + 1) % self.state.cron_jobs.len();
                        }
                    }
                    _ => {}
                }
            }
            // Page Up/Down for faster scrolling
            KeyCode::PageUp if !self.state.sidebar_focus && self.state.menu_item == MenuItem::Sessions => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if chat.auto_scroll {
                        chat.scroll = chat.max_scroll.get();
                        chat.auto_scroll = false;
                    }
                    chat.scroll = chat.scroll.saturating_sub(10);
                }
            }
            KeyCode::PageDown if !self.state.sidebar_focus && self.state.menu_item == MenuItem::Sessions => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if !chat.auto_scroll {
                        chat.scroll = chat.scroll.saturating_add(10);
                        if chat.scroll >= chat.max_scroll.get() {
                            chat.auto_scroll = true;
                        }
                    }
                }
            }
            // End key re-enables auto-scroll for chat
            KeyCode::End if !self.state.sidebar_focus && self.state.menu_item == MenuItem::Sessions => {
                if let Some(chat) = self.state.active_chat_mut() {
                    chat.auto_scroll = true;
                }
            }
            // Enter to view details
            KeyCode::Enter if !self.state.sidebar_focus => {
                match self.state.menu_item {
                    MenuItem::Credentials => {
                        match self.state.credentials_tab {
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
                        }
                    }
                    MenuItem::Models if !self.state.providers.is_empty() => {
                        self.state.input_mode = InputMode::ViewModelList {
                            provider_index: self.state.selected_provider,
                            scroll: 0,
                        };
                    }
                    _ => {}
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
            KeyCode::Char('5') => self.state.menu_item = MenuItem::CronJobs,
            KeyCode::Char('6') => self.state.menu_item = MenuItem::Models,
            KeyCode::Char('7') => self.state.menu_item = MenuItem::Settings,

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
            KeyCode::Char('n') if !self.state.sidebar_focus && self.state.menu_item == MenuItem::Sessions => {
                self.handle_new_session_action();
            }
            KeyCode::Char('e') if !self.state.sidebar_focus => {
                self.handle_edit_action();
            }
            KeyCode::Char('f') if !self.state.sidebar_focus && self.state.menu_item == MenuItem::Agents => {
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
            KeyCode::Char('p') if !self.state.sidebar_focus => {
                self.handle_permission_action();
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
                    if let Some(schema) = self.state.credential_schemas.get(self.state.selected_provider_index).cloned() {
                        let idx = self.state.selected_provider_index;
                        self.state.input_mode = InputMode::EditProvider(
                            EditProviderState::from_schema(&schema, idx)
                        );
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
            MenuItem::Sessions if !self.state.sidebar_focus => {
                self.state.status_message = Some(("Reloading sessions...".to_string(), false));
                self.spawn_sessions_load();
            }
            MenuItem::CronJobs if !self.state.sidebar_focus => {
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
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
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
                self.state.clear_input();
            }
            UnlockMethod::Keyfile { path } => {
                // Debug: log keyfile unlock attempt
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
                    use std::io::Write;
                    let _ = writeln!(f, "Keyfile unlock: path={path:?}");
                }

                // Auto-unlock with keyfile - no password needed
                let keyfile_path = path.clone().or_else(|| {
                    dirs::config_dir().map(|d| d.join("rockbot").join("vault.key").to_string_lossy().to_string())
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
                                        self.state.status_message = Some(("✅ Unlocked with keyfile".to_string(), false));
                                    }
                                    Err(e) => {
                                        self.state.status_message = Some((format!("❌ Keyfile unlock failed: {e}"), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message = Some((format!("❌ Failed to open vault: {e}"), true));
                            }
                        }
                    } else {
                        self.state.status_message = Some((format!("Keyfile not found: {kf_path}"), true));
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
                self.state.clear_input();
            }
            UnlockMethod::SshKey { path } => {
                // Try SSH agent unlock
                let ssh_path = path.clone().unwrap_or_else(|| "~/.ssh/id_ed25519".to_string());
                // TODO: Actually unlock via SSH agent
                self.state.status_message = Some((format!("SSH unlock not yet implemented (key: {ssh_path})"), true));
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
                "No LLM providers available — configure one in Models or Credentials → Providers".to_string(),
                true
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
                self.state.chat_model = self.state.agents.iter()
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
                "No LLM providers available — configure one in Models or Credentials → Providers".to_string(),
                true
            ));
            return;
        }
        let create_state = CreateSessionState::new(
            &self.state.providers,
            &self.state.agents,
        );
        self.state.input_mode = InputMode::CreateSession(create_state);
    }

    /// Open the edit credential modal for the given endpoint index (used from view modals)
    fn edit_endpoint_at(&mut self, endpoint_index: usize) {
        use super::state::EditCredentialState;

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
                if endpoint.has_credential { Some(&endpoint.id) } else { None },
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
        use super::state::EditPermissionState;
        if let Some(rule) = self.state.permissions.get(permission_index) {
            let edit_state = EditPermissionState::from_rule(rule, &self.state.endpoints, &self.state.agents);
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
                    self.state.credential_schemas.iter().find(|s| s.provider_id == p.id)
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
                    session_key: session.key.clone()
                };
                // Spawn async task to load session details
                self.spawn_session_details(&session.key);
            } else {
                self.state.status_message = Some(("No session selected".to_string(), true));
            }
        }
    }

    fn handle_permission_action(&mut self) {
        use super::state::EditPermissionState;

        if self.state.menu_item != MenuItem::Credentials || self.state.endpoints.is_empty() {
            return;
        }

        if self.state.credentials_tab == 2 && !self.state.permissions.is_empty() {
            // On permissions tab with a selected permission — edit it
            if let Some(rule) = self.state.permissions.get(self.state.selected_permission) {
                let edit_state = EditPermissionState::from_rule(rule, &self.state.endpoints, &self.state.agents);
                self.state.input_mode = InputMode::EditPermission(edit_state);
            }
        } else {
            // New permission — preselect endpoint if on endpoints tab
            let preselect = if self.state.credentials_tab == 0 {
                Some(self.state.selected_endpoint)
            } else {
                None
            };
            let edit_state = EditPermissionState::new(&self.state.endpoints, &self.state.agents, preselect);
            self.state.input_mode = InputMode::EditPermission(edit_state);
        }
    }

    fn handle_edit_permission(&mut self, key: KeyEvent, mut state: super::state::EditPermissionState) -> Result<()> {
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
                state.field_index = if state.field_index == 0 { field_count - 1 } else { state.field_index - 1 };
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
                r.endpoint_id == state.selected_endpoint_id() && r.source == state.sources[state.selected_source]
            }) {
                rule.access = state.access;
            } else {
                // Source changed — remove old, add new
                let next_priority = self.state.permissions.iter().map(|r| r.priority).max().unwrap_or(0) + 1;
                self.state.permissions.push(state.to_rule(next_priority));
            }
        } else {
            // New rule — remove existing rule for same endpoint+source combo
            self.state.permissions.retain(|r| {
                !(r.endpoint_id == state.selected_endpoint_id() && r.source == state.sources[state.selected_source])
            });
            let next_priority = self.state.permissions.iter().map(|r| r.priority).max().unwrap_or(0) + 1;
            self.state.permissions.push(state.to_rule(next_priority));
        }
        // Re-sort by priority
        self.state.permissions.sort_by_key(|r| r.priority);
        self.state.status_message = Some((
            format!("Permission set for '{}'", state.selected_endpoint_name()),
            false
        ));
        self.state.input_mode = InputMode::Normal;
    }

    fn move_permission(&mut self, up: bool) {
        let idx = self.state.selected_permission;
        let len = self.state.permissions.len();
        if len < 2 { return; }
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
            let idx = self.state.selected_provider.min(self.state.providers.len().saturating_sub(1));
            if let Some(provider) = self.state.providers.get(idx) {
                self.state.status_message = Some((format!("Testing {} connection...", provider.name), false));
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

        tokio::spawn(async move {
            match test_provider_via_gateway(&id).await {
                Ok((models_found, _)) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("✅ {name}: connection OK ({models_found} models)"),
                        false,
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("❌ {name}: {e}"),
                        true,
                    ));
                }
            }
        });
    }

    /// Load message history for a session from the gateway
    fn spawn_load_session_messages(&self, session_key: &str) {
        let tx = self.state.tx.clone();
        let key = session_key.to_string();
        tokio::spawn(async move {
            match load_session_messages(&key).await {
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
                    self.state.agents.iter()
                        .find(|a| a.id == session.agent_id)
                        .and_then(|a| a.model.clone())
                }
            });
            // Set agent_id for agent-bound sessions
            self.state.chat_agent_id = if !session.agent_id.is_empty() && !session.agent_id.starts_with("ad-hoc") {
                Some(session.agent_id.clone())
            } else {
                None
            };
            // Load messages if not already loaded
            let already_loaded = self.state.session_chats
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
        tokio::spawn(async move {
            match kill_session(&key).await {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus(format!("✅ Session archived: {key}"), false));
                    let _ = tx.send(Message::ReloadSessions);
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(format!("❌ Failed to archive session: {e}"), true));
                }
            }
        });
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_password_input(&mut self, key: KeyEvent, _masked: bool, action: PasswordAction) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let password = self.state.input_buffer.clone();
                self.state.clear_input();
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
                            match rockbot_credentials::CredentialVault::init_with_password(
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
                                    self.state.status_message = Some((format!("❌ Init failed: {e}"), true));
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
                                        self.state.status_message = Some((format!("❌ Wrong password: {e}"), true));
                                    }
                                }
                            }
                            Err(e) => {
                                self.state.status_message = Some((format!("❌ Failed to open vault: {e}"), true));
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
                    // Create default permission: Any Source with HIL access
                    if let Some(ep) = self.state.endpoints.iter().find(|e| e.name == endpoint_name) {
                        use super::state::{PermissionRule, PermissionSource, AccessLevel};
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

        if state.auth_type == ProviderAuthType::None {
            self.state.status_message = Some((
                format!("✅ {} - no authentication needed", state.provider_name),
                false
            ));
            return;
        }

        // Collect the secret value from form fields
        let secret_value = state.api_key(); // Checks api_key, bot_token, access_token, token, first secret field

        // For AWS credentials, also check specific field IDs
        let secret_value = if secret_value.is_empty() && state.auth_type == ProviderAuthType::AwsCredentials {
            // For AWS, store all secret fields as a JSON object
            let mut aws_creds = serde_json::Map::new();
            for field_id in &["access_key_id", "secret_access_key", "session_token", "bearer_token"] {
                if let Some(val) = state.get_field_value_by_id(field_id) {
                    if !val.is_empty() {
                        aws_creds.insert(field_id.to_string(), serde_json::Value::String(val.to_string()));
                    }
                }
            }
            if aws_creds.is_empty() {
                self.state.status_message = Some((
                    format!("💡 Set AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY, and AWS_REGION={}", state.aws_region()),
                    false
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
                    false
                ));
            }
            return;
        }

        // Determine base URL
        let base_url = if state.provider_id == "bedrock" {
            state.get_field_value_by_id("endpoint_url")
                .filter(|v| !v.is_empty())
                .map(|v| v.to_string())
                .unwrap_or_else(|| format!("https://bedrock-runtime.{}.amazonaws.com", state.aws_region()))
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
        tokio::spawn(async move {
            match save_provider_via_gateway(&provider_name, &endpoint_type, &base_url, &secret).await {
                Ok(()) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("✅ {provider_name} credentials saved"),
                        false
                    ));
                    // Reload providers to reflect updated availability
                    let _ = tx.send(Message::ReloadProviders);
                }
                Err(e) => {
                    let _ = tx.send(Message::SetStatus(
                        format!("❌ Failed to save {provider_name} credentials: {e}"),
                        true
                    ));
                }
            }
        });
    }

    fn handle_edit_agent(&mut self, key: KeyEvent, mut state: EditAgentState) -> Result<()> {
        let set_mode = |s: EditAgentState| -> InputMode {
            if s.is_edit { InputMode::EditAgent(s) } else { InputMode::AddAgent(s) }
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
                    self.state.status_message = Some((
                        format!("Agent '{}' already exists", state.id), true
                    ));
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
                        self.state.status_message = Some((
                            format!("Agent '{}' already exists", state.id), true
                        ));
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

    fn handle_create_session(&mut self, key: KeyEvent, mut state: CreateSessionState) -> Result<()> {
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
                            self.state.status_message = Some(("No model available".to_string(), true));
                            self.state.input_mode = InputMode::CreateSession(state);
                        }
                    }
                    SessionMode::AgentBound => {
                        if let Some(agent_id) = state.selected_agent_id() {
                            let agent_id = agent_id.to_string();
                            let agent_model = self.state.agents.iter()
                                .find(|a| a.id == agent_id)
                                .and_then(|a| a.model.clone());
                            self.spawn_create_session(Some(agent_id.clone()), agent_model.clone());
                            self.state.chat_model = agent_model;
                            self.state.chat_agent_id = Some(agent_id);
                            self.state.input_mode = InputMode::ChatInput;
                            self.state.clear_input();
                        } else {
                            self.state.status_message = Some(("No agent available".to_string(), true));
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
        tokio::spawn(async move {
            match create_session_via_gateway(agent_id.as_deref(), model.as_deref()).await {
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
            body.insert("id".to_string(), serde_json::Value::String(state.id.clone()));
        }
        if !state.model.is_empty() {
            body.insert("model".to_string(), serde_json::Value::String(state.model.clone()));
        }
        if !state.parent_id.is_empty() {
            body.insert("parent_id".to_string(), serde_json::Value::String(state.parent_id.clone()));
        }
        if !state.workspace.is_empty() {
            body.insert("workspace".to_string(), serde_json::Value::String(state.workspace.clone()));
        }
        if !state.max_tool_calls.is_empty() {
            if let Ok(n) = state.max_tool_calls.parse::<u32>() {
                body.insert("max_tool_calls".to_string(), serde_json::Value::Number(n.into()));
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
                body.insert("max_tokens".to_string(), serde_json::Value::Number(n.into()));
            }
        }
        if !state.system_prompt.is_empty() {
            body.insert("system_prompt".to_string(), serde_json::Value::String(state.system_prompt.clone()));
        }
        body.insert("enabled".to_string(), serde_json::Value::Bool(state.enabled));

        let json_body = serde_json::Value::Object(body);
        let is_edit = state.is_edit;
        let agent_id = state.id.clone();
        let tx = self.state.tx.clone();
        let config_path = self.state.config_path.clone();

        // Try gateway API first, fall back to direct config file edit
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();

            let gateway_result = if is_edit {
                client.put(format!("http://127.0.0.1:18080/api/agents/{agent_id}"))
                    .json(&json_body)
                    .send()
                    .await
            } else {
                client.post("http://127.0.0.1:18080/api/agents")
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
                    let _ = tx.send(Message::AgentSaveError(format!("Gateway error: {err_text}")));
                }
                Err(_) => {
                    // Gateway unreachable — fall back to direct config file edit
                    match save_agent_to_config_file(&config_path, &json_body, is_edit, &agent_id) {
                        Ok(()) => {
                            let action = if is_edit { "updated" } else { "created" };
                            let _ = tx.send(Message::AgentSaved(agent_id));
                            let _ = tx.send(Message::SetStatus(format!("Agent {action} (offline)"), false));
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
    let mut doc: toml_edit::DocumentMut = content.parse()
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
        if model.is_empty() { table.remove("model"); } else { table["model"] = toml_edit::value(model); }
    }
    if let Some(parent_id) = json.get("parent_id").and_then(|v| v.as_str()) {
        if parent_id.is_empty() { table.remove("parent_id"); } else { table["parent_id"] = toml_edit::value(parent_id); }
    }
    if let Some(workspace) = json.get("workspace").and_then(|v| v.as_str()) {
        if workspace.is_empty() { table.remove("workspace"); } else { table["workspace"] = toml_edit::value(workspace); }
    }
    if let Some(max_tool_calls) = json.get("max_tool_calls").and_then(serde_json::Value::as_i64) {
        table["max_tool_calls"] = toml_edit::value(max_tool_calls);
    }
    if let Some(system_prompt) = json.get("system_prompt").and_then(|v| v.as_str()) {
        if system_prompt.is_empty() { table.remove("system_prompt"); } else { table["system_prompt"] = toml_edit::value(system_prompt); }
    }
    if let Some(enabled) = json.get("enabled").and_then(serde_json::Value::as_bool) {
        table["enabled"] = toml_edit::value(enabled);
    }
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
                self.state.input_mode = InputMode::ViewPermission { permission_index: new_idx };
            }
            KeyCode::Char('-') | KeyCode::Char('J') => {
                // Move rule down in priority
                self.state.selected_permission = permission_index;
                self.move_permission(false);
                let new_idx = self.state.selected_permission;
                self.state.input_mode = InputMode::ViewPermission { permission_index: new_idx };
            }
            KeyCode::Char('d') => {
                if permission_index < self.state.permissions.len() {
                    let rule_name = self.state.permissions[permission_index].endpoint_name.clone();
                    self.state.permissions.remove(permission_index);
                    // Renumber priorities
                    for (i, rule) in self.state.permissions.iter_mut().enumerate() {
                        rule.priority = i + 1;
                    }
                    self.state.status_message = Some((format!("Removed rule for '{rule_name}'"), false));
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

    fn handle_view_model_list(&mut self, key: KeyEvent, provider_index: usize, scroll: usize) -> Result<()> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                self.state.input_mode = InputMode::Normal;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let model_count = self.state.providers.get(provider_index)
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
                                            self.state.status_message = Some(("✅ Deleted endpoint".to_string(), false));
                                        }
                                        Err(e) => {
                                            self.state.status_message = Some((format!("❌ Delete failed: {e}"), true));
                                        }
                                    }
                                }
                                Err(e) => {
                                    self.state.status_message = Some((format!("❌ Invalid endpoint ID: {e}"), true));
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
                        self.state.status_message = Some((format!("Disabled agent: {id} (edit config to persist)"), false));
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

    fn handle_view_context_files(&mut self, key: KeyEvent, state: ViewContextFilesState) -> Result<()> {
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
                        self.state.input_mode = InputMode::EditContextFile(
                            super::state::EditContextFileState::new(agent_id, filename, String::new())
                        );
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_edit_context_file(&mut self, key: KeyEvent, mut state: super::state::EditContextFileState) -> Result<()> {
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
                let line_len = state.content.split('\n')
                    .nth(state.cursor_line).unwrap_or("").len();
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
        let url = format!("http://127.0.0.1:18080/api/agents/{}/files", agent_id);
        let agent_id = agent_id.to_string();
        tokio::spawn(async move {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(files) = resp.json::<Vec<ContextFileInfo>>().await {
                        let _ = tx.send(Message::ContextFilesLoaded(agent_id, files));
                    }
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(
                        format!("Failed to list files: {}", resp.status())
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!("Failed to list files: {e}")));
                }
            }
        });
    }

    fn fetch_context_file(&self, agent_id: &str, filename: &str) {
        let tx = self.state.tx.clone();
        let url = format!("http://127.0.0.1:18080/api/agents/{}/files/{}", agent_id, filename);
        let agent_id = agent_id.to_string();
        let filename = filename.to_string();
        tokio::spawn(async move {
            match reqwest::get(&url).await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let _ = tx.send(Message::ContextFileLoaded(agent_id, filename, content));
                    }
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(
                        format!("Failed to load {filename}: {}", resp.status())
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!("Failed to load {filename}: {e}")));
                }
            }
        });
    }

    fn save_context_file(&self, agent_id: &str, filename: &str, content: &str) {
        let tx = self.state.tx.clone();
        let url = format!("http://127.0.0.1:18080/api/agents/{}/files/{}", agent_id, filename);
        let agent_id = agent_id.to_string();
        let filename = filename.to_string();
        let body = serde_json::json!({ "content": content });
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            match client.put(&url).json(&body).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let _ = tx.send(Message::ContextFileSaved(agent_id, filename));
                }
                Ok(resp) => {
                    let _ = tx.send(Message::ContextFileError(
                        format!("Failed to save {filename}: {}", resp.status())
                    ));
                }
                Err(e) => {
                    let _ = tx.send(Message::ContextFileError(format!("Failed to save {filename}: {e}")));
                }
            }
        });
    }

    fn handle_chat_input(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
            }
            // Shift+Enter or Alt+Enter inserts a newline (up to 10 lines)
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert_chat_newline();
            }
            // Ctrl+J (LF) or Ctrl+N inserts a newline — universally supported fallback
            KeyCode::Char('j') | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_chat_newline();
            }
            // Plain Enter sends the message
            KeyCode::Enter => {
                self.send_chat_buffer();
            }
            // Ctrl+R to retry the last user message
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.retry_last_message();
            }
            // Ctrl+T to toggle tool call expand/collapse
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.toggle_tool_expansion();
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.insert(self.state.input_cursor, c);
                self.state.input_cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.state.input_cursor > 0 {
                    // Find the previous char boundary
                    let prev = self.state.input_buffer[..self.state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                    self.state.input_buffer.remove(prev);
                    self.state.input_cursor = prev;
                }
            }
            KeyCode::Delete => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            KeyCode::Left => {
                if self.state.input_cursor > 0 {
                    // Move to previous char boundary
                    self.state.input_cursor = self.state.input_buffer[..self.state.input_cursor]
                        .char_indices()
                        .next_back()
                        .map(|(i, _)| i)
                        .unwrap_or(0);
                }
            }
            KeyCode::Right => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    // Move to next char boundary
                    self.state.input_cursor = self.state.input_buffer[self.state.input_cursor..]
                        .char_indices()
                        .nth(1)
                        .map(|(i, _)| self.state.input_cursor + i)
                        .unwrap_or(self.state.input_buffer.len());
                }
            }
            KeyCode::Home => {
                self.state.input_cursor = 0;
            }
            KeyCode::End => {
                self.state.input_cursor = self.state.input_buffer.len();
            }
            // Scroll chat while typing: PageUp/PageDown, Ctrl+Up/Down
            KeyCode::PageUp => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if chat.auto_scroll {
                        chat.scroll = chat.max_scroll.get();
                        chat.auto_scroll = false;
                    }
                    chat.scroll = chat.scroll.saturating_sub(10);
                }
            }
            KeyCode::PageDown => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if !chat.auto_scroll {
                        chat.scroll = chat.scroll.saturating_add(10);
                        if chat.scroll >= chat.max_scroll.get() {
                            chat.auto_scroll = true;
                        }
                    }
                }
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if chat.auto_scroll {
                        chat.scroll = chat.max_scroll.get();
                        chat.auto_scroll = false;
                    }
                    chat.scroll = chat.scroll.saturating_sub(3);
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(chat) = self.state.active_chat_mut() {
                    if !chat.auto_scroll {
                        chat.scroll = chat.scroll.saturating_add(3);
                        if chat.scroll >= chat.max_scroll.get() {
                            chat.auto_scroll = true;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn insert_chat_newline(&mut self) {
        let newline_count = self.state.input_buffer.chars().filter(|&c| c == '\n').count();
        if newline_count < 9 {
            self.state.input_buffer.insert(self.state.input_cursor, '\n');
            self.state.input_cursor += 1;
        }
    }

    fn send_chat_buffer(&mut self) {
        let message = self.state.input_buffer.trim().to_string();
        if !message.is_empty() {
            if let Some(chat) = self.state.active_chat_mut() {
                chat.messages.push(ChatMessage::user(message.clone()));
                chat.loading = true;
                chat.auto_scroll = true;
            }
            self.spawn_chat_request(message);
        }
        self.state.clear_input();
    }

    /// Retry the last user message (removes error message and re-sends)
    fn retry_last_message(&mut self) {
        let last_user_msg = if let Some(chat) = self.state.active_chat() {
            if chat.loading {
                return; // Already processing
            }
            chat.messages.iter().rev()
                .find(|m| m.role == super::state::ChatRole::User)
                .map(|m| m.content.clone())
        } else {
            None
        };

        if let Some(message) = last_user_msg {
            if let Some(chat) = self.state.active_chat_mut() {
                // Remove trailing error/system messages
                while chat.messages.last().is_some_and(|m| m.role == super::state::ChatRole::System) {
                    chat.messages.pop();
                }
                // Remove the previous assistant response too
                if chat.messages.last().is_some_and(|m| m.role == super::state::ChatRole::Assistant) {
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
        use super::state::ProviderAuthType;

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
        if !doc["providers"].as_table().is_some_and(|t| t.contains_key(provider_section)) {
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
                true
            ));
        } else {
            tracing::info!("Saved {} auth mode: {}", provider_section, auth_mode);
        }
    }

    /// Spawn an async task to send a chat message via the gateway
    fn spawn_chat_request(&self, user_message: String) {
        let tx = self.state.tx.clone();
        let session_key = self.state.active_session_key().unwrap_or("").to_string();
        // Resolve agent_id from chat_agent_id or the selected session's agent_id
        let agent_id = self.state.chat_agent_id.clone().or_else(|| {
            self.state.sessions.get(self.state.selected_session)
                .map(|s| &s.agent_id)
                .filter(|id| !id.is_empty() && !id.starts_with("ad-hoc"))
                .cloned()
        });
        let launch_dir = self.state.launch_dir.to_string_lossy().to_string();

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
        });
        if let Some(ref ws_tx) = self.ws_tx {
            if ws_tx.send(ws_msg.to_string()).is_err() {
                let _ = tx.send(Message::ChatError(
                    session_key,
                    "Failed to send message over WebSocket.".to_string(),
                ));
            }
        }
    }

    /// Render the entire UI
    fn render(&mut self, frame: &mut Frame) {
        // Layout: top strip (menu + cards) | main content | status bar
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(0), Constraint::Length(1)])
            .split(frame.area());

        // Top strip: menu (left) | 1-col gap | cards (right)
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Length(1), Constraint::Min(0)])
            .split(rows[0]);

        // Sidebar menu — same height as cards
        render_sidebar(frame, top[0], &self.state, &self.effect_state);

        let cards_area = top[2];

        // Render a top border on the main content pane for visual separation
        let border_block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray));
        let detail_area = border_block.inner(rows[1]);
        frame.render_widget(border_block, rows[1]);

        // Content: page cards in top strip, detail in main area
        match self.state.menu_item {
            MenuItem::Dashboard => render_dashboard(frame, cards_area, detail_area, &self.state, &self.effect_state),
            MenuItem::Credentials => render_credentials(frame, cards_area, detail_area, &self.state, self.state.credentials_tab, &self.effect_state),
            MenuItem::Agents => render_agents(frame, cards_area, detail_area, &self.state, &self.effect_state),
            MenuItem::Sessions => render_sessions(frame, cards_area, detail_area, &self.state, &self.effect_state),
            MenuItem::CronJobs => render_cron_jobs(frame, cards_area, detail_area, &self.state, &self.effect_state),
            MenuItem::Models => render_models(frame, cards_area, detail_area, &self.state, &self.effect_state),
            MenuItem::Settings => render_settings(frame, cards_area, detail_area, &self.state, &self.effect_state),
        }

        // Status bar
        let help_text = self.get_help_text();
        render_status_bar(frame, rows[2], self.state.status_message.as_ref(), &help_text);

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
                render_view_endpoint_modal(frame, frame.area(), *endpoint_index, &self.state.endpoints);
            }
            InputMode::ViewProvider { provider_index } => {
                render_view_provider_modal(frame, frame.area(), *provider_index, &self.state.credential_schemas, &self.state.endpoints);
            }
            InputMode::ViewModelList { provider_index, scroll } => {
                render_view_model_list_modal(frame, frame.area(), *provider_index, *scroll, &self.state.providers);
            }
            InputMode::ViewPermission { permission_index } => {
                render_view_permission_modal(frame, frame.area(), *permission_index, &self.state.permissions);
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
            _ => {}
        }
    }

    fn get_help_text(&self) -> String {
        match &self.state.input_mode {
            InputMode::Normal => {
                if self.state.sidebar_focus {
                    "q:Quit │ ↑↓/jk:Navigate │ Enter:Select │ Tab:→Content │ 1-7:Quick".to_string()
                } else {
                    match self.state.menu_item {
                        MenuItem::Dashboard => {
                            "←→:Select │ r:Refresh │ Esc/Tab:←Sidebar".to_string()
                        }
                        MenuItem::Credentials => {
                            let tab_help = match self.state.credentials_tab {
                                0 => "Enter:View │ a:Add │ d:Delete │ p:Permission",
                                1 => "Enter:View │ a:Configure",
                                2 => "Enter:View │ p:Add Rule │ ↑↓:Navigate",
                                _ => "Enter:View",
                            };
                            format!("←→:Tab │ {tab_help} │ Esc:← ({})", self.credentials_tab().label())
                        }
                        MenuItem::Agents => {
                            "←→:Select │ a:Add │ e:Edit │ d:Disable │ r:Reload │ Esc:←".to_string()
                        }
                        MenuItem::Sessions => {
                            "←→:Select │ n:New │ c:Chat │ k:Kill │ Esc:←".to_string()
                        }
                        MenuItem::CronJobs => {
                            "←→:Filter │ ↑↓:Select │ e:Enable/Disable │ d:Delete │ t:Trigger │ r:Refresh".to_string()
                        }
                        MenuItem::Models => {
                            "←→:Select │ Enter:Models │ e:Edit │ t:Test │ Esc:←".to_string()
                        }
                        MenuItem::Settings => {
                            "←→:Select │ s:Start │ S:Stop │ r:Restart │ Esc:←".to_string()
                        }
                    }
                }
            }
            InputMode::PasswordInput { .. } => "Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::AddCredential(_) => "↑↓/Tab:Navigate │ ←→:Type │ Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::Confirm { .. } => "y:Yes │ n:No │ Esc:Cancel".to_string(),
            InputMode::ChatInput => "Enter:Send │ Ctrl+J:Newline │ PgUp/Dn:Scroll │ Ctrl+R:Retry │ Esc:Close".to_string(),
            InputMode::EditCredential(_) => "↑↓/Tab:Navigate │ Enter:Submit │ Esc:Cancel".to_string(),
            InputMode::EditProvider(_) => "↑↓/Tab:Navigate │ ←→:Auth Type │ Enter:Save │ Esc:Cancel".to_string(),
            InputMode::AddAgent(_) | InputMode::EditAgent(_) => "↑↓/Tab:Navigate │ ←→:Cycle Model │ Ctrl+S:Save │ Esc:Cancel".to_string(),
            InputMode::CreateSession(_) => "↑↓/Tab:Navigate │ ←→:Cycle │ Enter:Create │ Esc:Cancel".to_string(),
            InputMode::ViewSession { .. } => "Esc/Enter:Close".to_string(),
            InputMode::ViewEndpoint { .. } => "e:Edit │ Esc:Close".to_string(),
            InputMode::ViewProvider { .. } => "e:Configure │ Esc:Close".to_string(),
            InputMode::ViewModelList { .. } => "↑↓:Scroll │ Esc:Close".to_string(),
            InputMode::ViewPermission { .. } => "e:Edit │ +/-:Reorder │ d:Delete │ Esc:Close".to_string(),
            InputMode::EditPermission(_) => "↑↓:Field │ ←→:Cycle │ Enter/Ctrl+S:Save │ Esc:Cancel".to_string(),
            InputMode::ViewContextFiles(_) => "↑↓:Select │ Enter:Edit │ Esc:Close".to_string(),
            InputMode::EditContextFile(_) => "Ctrl+S:Save │ ↑↓←→:Move │ Esc:Back".to_string(),
        }
    }
}

/// Add a credential to the vault based on form state (standalone to avoid borrow issues)
fn add_credential_to_vault(
    vault: &mut rockbot_credentials::CredentialVault,
    state: &AddCredentialState,
) -> Result<String> {
    use rockbot_credentials::{EndpointType, CredentialType};

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
    vault: &mut rockbot_credentials::CredentialVault,
    state: &EditCredentialState,
) -> Result<String> {
    

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
        event::{PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags, KeyboardEnhancementFlags},
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::backend::CrosstermBackend;
    use ratatui::Terminal;
    use std::io;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Try to enable keyboard enhancement for Shift+Enter detection
    let has_keyboard_enhancement = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    ).is_ok();
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
                // Check if a pending WS connection has completed
                if let Some(ref mut rx) = app.ws_pending_rx {
                    if let Ok(ws_send_tx) = rx.try_recv() {
                        tracing::info!("WebSocket channel ready — activating");
                        app.ws_tx = Some(ws_send_tx);
                        app.ws_pending_rx = None;
                    }
                }

                if app.ws_connected() {
                    // WebSocket is active — send a ping instead of HTTP poll
                    if let Some(ref ws_tx) = app.ws_tx {
                        let _ = ws_tx.send(r#"{"type":"health_check"}"#.to_string());
                    }
                } else {
                    // No WebSocket — fall back to HTTP status check
                    if !app.state.gateway_loading {
                        app.spawn_gateway_check();
                    }
                    // Try to reconnect WebSocket if gateway is up and no
                    // connect attempt is already in progress
                    if app.state.gateway.connected && app.ws_pending_rx.is_none() {
                        app.spawn_ws_connect();
                    }
                }
            }
        }

        if app.state.should_exit {
            break;
        }
    }

    // Restore terminal
    if has_keyboard_enhancement {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

// =============================================================================
// Background task implementations
// =============================================================================

/// Parse an incoming WebSocket message from the gateway and dispatch to the TUI state
fn handle_ws_response(tx: &mpsc::UnboundedSender<Message>, text: &str) {
    let json: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Invalid WebSocket JSON from gateway: {e}");
            return;
        }
    };

    let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match msg_type {
        "stream_chunk" => {
            let session_key = json.get("session_key").and_then(|v| v.as_str()).unwrap_or("");
            let delta = json.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if !session_key.is_empty() && !delta.is_empty() {
                // ChatStreamChunk format is "session_key:text"
                let _ = tx.send(Message::ChatStreamChunk(
                    format!("{session_key}:{delta}")
                ));
            }
        }
        "tool_call" => {
            let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
            let _ = tx.send(Message::SetStatus(format!("Running: {tool_name}..."), false));
        }
        "tool_result" => {
            let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown");
            let success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
            let duration = json.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            let status = if success { "✓" } else { "✗" };
            let _ = tx.send(Message::SetStatus(
                format!("{status} {tool_name} ({duration}ms)"), !success
            ));
        }
        "agent_response" => {
            let session_key = json.get("session_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tool_calls: Vec<ToolCallInfo> = json.get("tool_calls")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|tc| {
                        let raw_result = tc.get("result").and_then(|v| v.as_str()).unwrap_or("");
                        Some(ToolCallInfo {
                            tool_name: tc.get("tool_name")?.as_str()?.to_string(),
                            arguments: String::new(),
                            result: truncate_tool_result(raw_result, 500),
                            success: tc.get("success").and_then(|v| v.as_bool()).unwrap_or(true),
                            duration_ms: tc.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                            expanded: false,
                        })
                    }).collect()
                })
                .unwrap_or_default();

            if tool_calls.is_empty() {
                let _ = tx.send(Message::ChatResponse(session_key.clone(), content));
            } else {
                let _ = tx.send(Message::ChatAgentResponse(session_key.clone(), content, tool_calls));
            }

            // Show token usage in status bar if available
            if let Some(tokens) = json.get("tokens_used") {
                let total = tokens.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let prompt = tokens.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let completion = tokens.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let time_ms = json.get("processing_time_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                if total > 0 {
                    let status = if time_ms > 0 {
                        format!("Tokens: {total} ({prompt} prompt + {completion} completion) | {time_ms}ms")
                    } else {
                        format!("Tokens: {total} ({prompt} prompt + {completion} completion)")
                    };
                    let _ = tx.send(Message::SetStatus(status, false));
                }
            }
        }
        "agent_error" => {
            let session_key = json.get("session_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let error = json.get("error").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string();
            let _ = tx.send(Message::ChatError(session_key, error));
        }
        "token_usage" => {
            let session_key = json.get("session_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let prompt = json.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let completion = json.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = json.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cumulative = json.get("cumulative_total").and_then(|v| v.as_u64()).unwrap_or(0);
            if !session_key.is_empty() {
                let _ = tx.send(Message::ChatTokenUsage {
                    session_key,
                    prompt_tokens: prompt,
                    completion_tokens: completion,
                    total_tokens: total,
                    cumulative_total: cumulative,
                });
            }
        }
        "thinking_status" => {
            let session_key = json.get("session_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let phase = json.get("phase").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let tool_name = json.get("tool_name").and_then(|v| v.as_str()).map(String::from);
            let iteration = json.get("iteration").and_then(|v| v.as_u64()).map(|v| v as usize);
            if !session_key.is_empty() {
                let _ = tx.send(Message::ChatThinkingStatus {
                    session_key,
                    phase,
                    tool_name,
                    iteration,
                });
            }
        }
        "pong" => {
            // Silently handle keepalive responses
        }
        "health_status" => {
            // Update gateway status from WebSocket health check
            if let Some(status) = json.get("status") {
                let gateway_status = super::state::GatewayStatus {
                    connected: true,
                    version: status.get("version").and_then(|v| v.as_str()).map(String::from),
                    uptime_secs: status.get("uptime_seconds").and_then(|v| v.as_u64()),
                    active_sessions: status.get("active_sessions").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                    pending_agents: status.get("pending_agents").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                };
                let _ = tx.send(Message::GatewayStatus(gateway_status));
            }
        }
        "error" => {
            let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            tracing::warn!("Gateway WebSocket error: {message}");
        }
        other => {
            tracing::debug!("Unhandled WebSocket message type: {other}");
        }
    }
}

use super::state::{AgentInfo, AgentStatus, AuthMethodInfo, CredentialFieldInfo, CredentialSchemaInfo, CronJobInfo, GatewayStatus, ModelProvider, ModelProviderModel, VaultStatus};

async fn check_gateway_status() -> Result<GatewayStatus> {
    use tokio::time::timeout;

    // Try to fetch actual status from the gateway API
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let status_result = timeout(
        Duration::from_secs(3),
        client.get("http://127.0.0.1:18080/api/status").send()
    ).await;

    match status_result {
        Ok(Ok(response)) if response.status().is_success() => {
            // Parse the JSON response
            if let Ok(json) = response.json::<serde_json::Value>().await {
                return Ok(GatewayStatus {
                    connected: true,
                    version: json.get("version").and_then(|v| v.as_str()).map(String::from),
                    uptime_secs: json.get("uptime_secs").and_then(serde_json::Value::as_u64),
                    active_sessions: json.get("active_sessions").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize,
                    pending_agents: json.get("pending_agents").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize,
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

/// Load agents from the gateway API, falling back to the config file if the gateway is unreachable.
async fn load_agents(config_path: &PathBuf) -> Result<Vec<AgentInfo>> {
    // Try loading from gateway first
    if let Ok(agents) = load_agents_from_gateway().await {
        if !agents.is_empty() {
            return Ok(agents);
        }
    }

    // Fallback: read from config file directly
    load_agents_from_config(config_path).await
}

/// Load agents from the gateway's /api/agents endpoint
async fn load_agents_from_gateway() -> Result<Vec<AgentInfo>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let resp = client.get("http://127.0.0.1:18080/api/agents").send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }

    let items: Vec<serde_json::Value> = resp.json().await?;
    let mut agents = Vec::new();

    for entry in &items {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() { continue; }

        let model = entry.get("model").and_then(|v| v.as_str()).map(String::from);
        let parent_id = entry.get("parent_id").and_then(|v| v.as_str()).map(String::from);
        let system_prompt = entry.get("system_prompt").and_then(|v| v.as_str()).map(String::from);
        let workspace = entry.get("workspace").and_then(|v| v.as_str()).map(String::from);
        let max_tool_calls = entry.get("max_tool_calls").and_then(serde_json::Value::as_u64).map(|n| n as u32);
        let temperature = entry.get("temperature").and_then(serde_json::Value::as_f64).map(|n| n as f32);
        let max_tokens = entry.get("max_tokens").and_then(serde_json::Value::as_u64).map(|n| n as u32);
        let enabled = entry.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(true);
        let session_count = entry.get("session_count").and_then(serde_json::Value::as_u64).unwrap_or(0) as usize;

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
                let max_tool_calls = entry.get("max_tool_calls").and_then(toml::Value::as_integer).map(|n| n as u32);
                let temperature = entry.get("temperature").and_then(toml::Value::as_float).map(|n| n as f32);
                let max_tokens = entry.get("max_tokens").and_then(toml::Value::as_integer).map(|n| n as u32);
                let enabled = entry.get("enabled").and_then(toml::Value::as_bool).unwrap_or(true);

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
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: path={vault_path:?}");
    }

    let exists = CredentialVault::exists(vault_path);

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
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
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: raw unlock_method={method:?}");
            }
            match method {
                Some(rockbot_credentials::UnlockMethod::Password { .. }) => {
                    UnlockMethod::Password
                }
                Some(rockbot_credentials::UnlockMethod::Keyfile { path_hint }) => {
                    UnlockMethod::Keyfile { path: path_hint.clone() }
                }
                Some(rockbot_credentials::UnlockMethod::Age { public_key, .. }) => {
                    UnlockMethod::Age { public_key: Some(public_key.clone()) }
                }
                Some(rockbot_credentials::UnlockMethod::SshKey { public_key_path, .. }) => {
                    UnlockMethod::SshKey { path: Some(public_key_path.clone()) }
                }
                None => UnlockMethod::Unknown,
            }
        }
        Err(e) => {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
                use std::io::Write;
                let _ = writeln!(f, "check_vault_status: open error={e:?}");
            }
            UnlockMethod::Unknown
        }
    };

    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open("/tmp/rockbot_debug.log") {
        use std::io::Write;
        let _ = writeln!(f, "check_vault_status: final unlock_method={unlock_method:?}");
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
async fn load_cron_jobs_from_gateway() -> Result<Vec<CronJobInfo>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let resp = client.get("http://127.0.0.1:18080/api/cron/jobs").send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }

    let items: Vec<serde_json::Value> = resp.json().await?;
    let mut jobs = Vec::new();

    for entry in &items {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if id.is_empty() { continue; }

        let schedule_val = entry.get("schedule");
        let schedule_str = match schedule_val.and_then(|s| s.get("type")).and_then(|t| t.as_str()) {
            Some("cron") => schedule_val.and_then(|s| s.get("expression")).and_then(|e| e.as_str())
                .unwrap_or("?").to_string(),
            Some("every") => {
                let ms = schedule_val.and_then(|s| s.get("interval_ms")).and_then(|v| v.as_u64()).unwrap_or(0);
                if ms >= 3_600_000 { format!("every {}h", ms / 3_600_000) }
                else if ms >= 60_000 { format!("every {}m", ms / 60_000) }
                else { format!("every {}s", ms / 1000) }
            }
            Some("at") => {
                let at_ms = schedule_val.and_then(|s| s.get("at_ms")).and_then(|v| v.as_u64()).unwrap_or(0);
                format!("once @{at_ms}")
            }
            _ => "unknown".to_string(),
        };

        let state_val = entry.get("state");
        let last_run = state_val.and_then(|s| s.get("last_run_at_ms")).and_then(|v| v.as_u64())
            .map(|ms| {
                chrono::DateTime::from_timestamp_millis(ms as i64)
                    .map(|dt| dt.format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{ms}"))
            });
        let last_status = state_val.and_then(|s| s.get("last_run_status")).and_then(|v| v.as_str()).map(String::from);
        let next_run = state_val.and_then(|s| s.get("next_run_at_ms")).and_then(|v| v.as_u64())
            .map(|ms| {
                chrono::DateTime::from_timestamp_millis(ms as i64)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{ms}"))
            });

        jobs.push(CronJobInfo {
            id,
            name: entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            enabled: entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            agent_id: entry.get("agent_id").and_then(|v| v.as_str()).map(String::from),
            schedule: schedule_str,
            last_run,
            last_status,
            next_run,
        });
    }

    Ok(jobs)
}

async fn toggle_cron_job(job_id: &str, enabled: bool) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .put(format!("http://127.0.0.1:18080/api/cron/jobs/{job_id}"))
        .json(&serde_json::json!({ "enabled": enabled }))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }
    Ok(())
}

async fn delete_cron_job(job_id: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .delete(format!("http://127.0.0.1:18080/api/cron/jobs/{job_id}"))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("Gateway returned {}", resp.status());
    }
    Ok(())
}

async fn trigger_cron_job(job_id: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .post(format!("http://127.0.0.1:18080/api/cron/jobs/{job_id}/trigger"))
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
async fn test_provider_via_gateway(provider_id: &str) -> Result<(u64, String)> {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:18080/api/providers/{provider_id}/test"))
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
async fn kill_session(session_key: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .delete(format!("http://127.0.0.1:18080/api/sessions/{session_key}"))
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

/// Load message history for a session from the gateway
async fn load_session_messages(session_key: &str) -> Result<Vec<ChatMessage>> {
    use tokio::time::timeout;

    let client = reqwest::Client::new();
    let result = timeout(
        Duration::from_secs(3),
        client.get(format!("http://127.0.0.1:18080/api/sessions/{session_key}/messages")).send(),
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
                            let role_str = content.get("role")
                                .or_else(|| msg.get("role"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("user");
                            let role = match role_str {
                                "assistant" => super::state::ChatRole::Assistant,
                                "system" => super::state::ChatRole::System,
                                _ => super::state::ChatRole::User,
                            };
                            let timestamp = msg.get("created_at")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            Some(ChatMessage { role, content: text, timestamp, tool_calls: Vec::new() })
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
    provider_name: &str,
    endpoint_type: &str,
    base_url: &str,
    secret: &str,
) -> Result<()> {
    let client = reqwest::Client::new();

    // Step 1: Create endpoint
    let ep_response = client
        .post("http://127.0.0.1:18080/api/credentials/endpoints")
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
        return Err(anyhow::anyhow!("Failed to create endpoint ({status}): {body}"));
    }

    let ep: serde_json::Value = ep_response.json().await?;
    let ep_id = ep["id"].as_str()
        .ok_or_else(|| anyhow::anyhow!("No endpoint ID in response"))?;

    // Step 2: Store credential (base64 encoded)
    let encoded_secret = base64_encode(secret.as_bytes());
    let cred_response = client
        .post(format!("http://127.0.0.1:18080/api/credentials/endpoints/{ep_id}/credential"))
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
        return Err(anyhow::anyhow!("Failed to store credential ({status}): {body}"));
    }

    Ok(())
}

/// Load providers from the gateway API
async fn load_providers_from_gateway() -> Result<Vec<ModelProvider>> {
    use tokio::time::timeout;

    let client = reqwest::Client::new();
    let result = timeout(
        Duration::from_secs(2),
        client.get("http://127.0.0.1:18080/api/providers").send(),
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
                            let available = p.get("available").and_then(serde_json::Value::as_bool).unwrap_or(false);
                            let auth_type = p.get("auth_type").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
                            let supports_streaming = p.get("supports_streaming").and_then(serde_json::Value::as_bool).unwrap_or(false);
                            let supports_tools = p.get("supports_tools").and_then(serde_json::Value::as_bool).unwrap_or(false);
                            let supports_vision = p.get("supports_vision").and_then(serde_json::Value::as_bool).unwrap_or(false);

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
                                                description: m.get("description")?.as_str()?.to_string(),
                                                context_window: m.get("context_window")?.as_u64()? as u32,
                                                max_output_tokens: m.get("max_output_tokens").and_then(serde_json::Value::as_u64).map(|v| v as u32),
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
async fn load_credential_schemas() -> Result<Vec<CredentialSchemaInfo>> {
    use tokio::time::timeout;

    let client = reqwest::Client::new();
    let result = timeout(
        Duration::from_secs(2),
        client.get("http://127.0.0.1:18080/api/credentials/schemas").send(),
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
async fn load_sessions_from_gateway() -> Result<Vec<super::state::SessionInfo>> {
    use tokio::time::timeout;

    let client = reqwest::Client::new();
    let result = timeout(
        Duration::from_secs(2),
        client.get("http://127.0.0.1:18080/api/sessions").send(),
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
                        let created_at = s.get("created_at").and_then(|v| v.as_str()).map(String::from);
                        let model = s.get("metadata")
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
async fn create_session_via_gateway(agent_id: Option<&str>, model: Option<&str>) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut body = serde_json::Map::new();
    if let Some(id) = agent_id {
        body.insert("agent_id".to_string(), serde_json::Value::String(id.to_string()));
    }
    if let Some(m) = model {
        body.insert("model".to_string(), serde_json::Value::String(m.to_string()));
    }

    let response = client
        .post("http://127.0.0.1:18080/api/sessions")
        .json(&serde_json::Value::Object(body))
        .send()
        .await?;

    if response.status().is_success() {
        let json: serde_json::Value = response.json().await?;
        let session_id = json.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        Ok(session_id)
    } else {
        let err_text = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("Gateway error: {err_text}"))
    }
}
