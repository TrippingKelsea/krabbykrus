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
    render_agents, render_credentials, render_models, render_password_modal,
    render_sessions, render_settings, render_sidebar, render_status_bar,
};
use super::effects::EffectState;
use super::state::{
    AddCredentialState, AppState, ConfirmAction, InputMode,
    MenuItem, Message, PasswordAction, UnlockMethod,
};

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
}

impl App {
    pub fn new(config_path: PathBuf, vault_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        
        Self {
            state: AppState::new(config_path, vault_path, tx),
            rx,
            effect_state: EffectState::new(),
            models_tab: 0,
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
        self.state.update(msg);
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
            InputMode::Confirm { action, .. } => {
                let action = action.clone();
                self.handle_confirm(key, action)
            }
            InputMode::ChatInput => self.handle_chat_input(key),
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
                // Esc returns to sidebar
                self.state.sidebar_focus = true;
                self.effect_state.set_active(false);
            }
            KeyCode::Up | KeyCode::Char('k') if !self.state.sidebar_focus => {
                self.state.select_prev();
            }
            KeyCode::Down | KeyCode::Char('j') if !self.state.sidebar_focus => {
                self.state.select_next();
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
            MenuItem::Credentials if self.state.vault.initialized && !self.state.vault.locked => {
                self.state.input_mode = InputMode::AddCredential(AddCredentialState::new());
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
        self.state.status_message = Some(("Refreshing...".to_string(), false));
        self.spawn_gateway_check();
        self.spawn_agents_load();
        self.spawn_vault_check();
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
                // Auto-unlock with keyfile - no password needed
                let keyfile_path = path.clone().or_else(|| {
                    dirs::config_dir().map(|d| d.join("krabbykrus").join("vault.key").to_string_lossy().to_string())
                });
                
                if let Some(kf_path) = keyfile_path {
                    if std::path::Path::new(&kf_path).exists() {
                        // TODO: Actually unlock with keyfile via background task
                        self.state.vault.locked = false;
                        self.state.status_message = Some((format!("Unlocked with keyfile: {}", kf_path), false));
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
                            // TODO: Actually initialize vault
                            self.state.vault.initialized = true;
                            self.state.vault.locked = false;
                            self.state.status_message = Some(("✅ Vault initialized!".to_string(), false));
                        }
                    }
                    PasswordAction::UnlockVault => {
                        // TODO: Actually unlock vault
                        self.state.vault.locked = false;
                        self.state.status_message = Some(("Vault unlocked".to_string(), false));
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
                        // TODO: Actually add credential
                        self.state.status_message = Some((format!("Added: {}", state.name), false));
                        self.state.input_mode = InputMode::Normal;
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

    fn handle_confirm(&mut self, key: KeyEvent, action: ConfirmAction) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                match action {
                    ConfirmAction::DeleteEndpoint(id) => {
                        // TODO: Actually delete
                        self.state.status_message = Some((format!("Deleted endpoint: {}", id), false));
                    }
                    ConfirmAction::DeleteAgent(id) => {
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
                // TODO: Send chat message
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
            InputMode::Confirm { message, .. } => {
                render_confirm_modal(frame, frame.area(), message);
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
                                "a:Add │ d:Delete │ u:Unlock │ l:Lock │ {{}}:Tabs ({}) │ Esc:←",
                                self.credentials_tab().label()
                            )
                        }
                        MenuItem::Agents => {
                            "r:Reload │ e:Edit │ d:Disable │ Esc/Tab:←Sidebar".to_string()
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
        }
    }
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
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    
    let connected = timeout(
        Duration::from_millis(200),
        TcpStream::connect("127.0.0.1:18080")
    ).await.is_ok();
    
    // TODO: If connected, fetch actual status from gateway API
    Ok(GatewayStatus {
        connected,
        version: if connected { Some("0.1.0".to_string()) } else { None },
        uptime_secs: None,
        active_sessions: 0,
        pending_agents: 0,
    })
}

async fn load_agents(config_path: &PathBuf) -> Result<Vec<AgentInfo>> {
    // Read config file and parse agents
    let content = tokio::fs::read_to_string(config_path).await?;
    
    let mut agents = Vec::new();
    let mut in_agent = false;
    let mut current_id = String::new();
    let mut current_model = None;
    
    for line in content.lines() {
        let line = line.trim();
        if line == "[[agents.list]]" {
            if !current_id.is_empty() {
                agents.push(AgentInfo {
                    id: current_id.clone(),
                    model: current_model.take(),
                    status: AgentStatus::Active,
                    session_count: 0,
                });
            }
            in_agent = true;
            current_id.clear();
        } else if in_agent {
            if line.starts_with("id") {
                if let Some(val) = line.split('=').nth(1) {
                    current_id = val.trim().trim_matches('"').to_string();
                }
            } else if line.starts_with("model") {
                if let Some(val) = line.split('=').nth(1) {
                    current_model = Some(val.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    
    // Don't forget the last agent
    if !current_id.is_empty() {
        agents.push(AgentInfo {
            id: current_id,
            model: current_model,
            status: AgentStatus::Active,
            session_count: 0,
        });
    }
    
    // If no agents found, add a default
    if agents.is_empty() {
        agents.push(AgentInfo {
            id: "default".to_string(),
            model: Some("claude-sonnet-4-20250514".to_string()),
            status: AgentStatus::Active,
            session_count: 0,
        });
    }
    
    Ok(agents)
}

async fn check_vault_status(vault_path: &PathBuf) -> Result<VaultStatus> {
    use krabbykrus_credentials::CredentialVault;
    
    let exists = CredentialVault::exists(vault_path);
    
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
            match vault.unlock_method() {
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
        Err(_) => UnlockMethod::Unknown,
    };
    
    Ok(VaultStatus {
        enabled: true,
        initialized: true,
        locked: true, // Assume locked until unlocked
        endpoint_count: 0,
        unlock_method,
    })
}
