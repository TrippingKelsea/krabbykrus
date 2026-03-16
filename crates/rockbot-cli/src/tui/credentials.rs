//! Credentials management TUI
//!
//! Provides an interactive interface for managing the credential vault.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs},
    Frame,
};
use rockbot_credentials::{CredentialVault, UnlockMethod};
use std::path::PathBuf;

/// Tab selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsTab {
    Endpoints,
    Permissions,
    Audit,
    Settings,
}

impl CredentialsTab {
    pub fn titles() -> Vec<&'static str> {
        vec!["Endpoints", "Permissions", "Audit", "Settings"]
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Endpoints => 0,
            Self::Permissions => 1,
            Self::Audit => 2,
            Self::Settings => 3,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            0 => Self::Endpoints,
            1 => Self::Permissions,
            2 => Self::Audit,
            _ => Self::Settings,
        }
    }
}

/// Input mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    /// Adding new credential - modal dialog
    AddCredential(AddCredentialState),
    /// Password input (for unlocking)
    PasswordInput {
        prompt: String,
        masked: bool,
    },
    /// Confirmation dialog
    Confirm(String),
}

/// State for add credential modal
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddCredentialState {
    pub field: AddCredentialField,
    pub name: String,
    pub endpoint_type: usize, // Index into ENDPOINT_TYPES
    pub url: String,
    pub secret: String,
    pub expiration: String, // Optional, e.g., "30d", "2025-12-31"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddCredentialField {
    Name,
    EndpointType,
    Url,
    Secret,
    Expiration,
}

impl AddCredentialField {
    fn next(&self) -> Self {
        match self {
            Self::Name => Self::EndpointType,
            Self::EndpointType => Self::Url,
            Self::Url => Self::Secret,
            Self::Secret => Self::Expiration,
            Self::Expiration => Self::Name,
        }
    }

    fn prev(&self) -> Self {
        match self {
            Self::Name => Self::Expiration,
            Self::EndpointType => Self::Name,
            Self::Url => Self::EndpointType,
            Self::Secret => Self::Url,
            Self::Expiration => Self::Secret,
        }
    }
}

/// Service types - determines what kind of endpoint this is
const ENDPOINT_TYPES: &[(&str, &str)] = &[
    ("home_assistant", "Home Assistant"),
    ("generic_rest", "Generic REST API"),
    ("generic_oauth2", "OAuth2 Service"),
    ("api_key_service", "API Key Service"),
    ("basic_auth_service", "Basic Auth Service"),
    ("bearer_token", "Bearer Token"),
];

impl Default for AddCredentialState {
    fn default() -> Self {
        Self {
            field: AddCredentialField::Name,
            name: String::new(),
            endpoint_type: 0,
            url: String::new(),
            secret: String::new(),
            expiration: String::new(),
        }
    }
}

/// Credentials TUI state
pub struct CredentialsTui {
    /// Vault path
    vault_path: PathBuf,
    /// Current tab
    pub tab: CredentialsTab,
    /// Whether vault exists (has been initialized)
    pub vault_exists: bool,
    /// Whether vault is unlocked
    pub unlocked: bool,
    /// Unlock method used
    pub unlock_method: Option<UnlockMethod>,
    /// List of endpoints
    pub endpoints: Vec<EndpointInfo>,
    /// Selected endpoint index
    pub endpoint_state: ListState,
    /// Input mode
    pub input_mode: InputMode,
    /// Status message
    pub status: Option<(String, bool)>, // (message, is_error)
    /// Whether to exit
    pub should_exit: bool,
    /// Whether running standalone or embedded
    pub standalone: bool,
    /// Temporary input buffer for password
    input_buffer: String,
}

/// Endpoint display info
#[derive(Debug, Clone)]
pub struct EndpointInfo {
    pub id: String,
    pub name: String,
    pub endpoint_type: String,
    pub url: String,
    pub has_credential: bool,
    pub expiration: Option<String>,
}

impl CredentialsTui {
    pub fn new(vault_path: PathBuf, standalone: bool) -> Self {
        let mut endpoint_state = ListState::default();
        endpoint_state.select(Some(0));
        let vault_exists = CredentialVault::exists(&vault_path);

        Self {
            vault_path,
            tab: CredentialsTab::Endpoints,
            vault_exists,
            unlocked: false,
            unlock_method: None,
            endpoints: Vec::new(),
            endpoint_state,
            input_mode: InputMode::Normal,
            status: None,
            should_exit: false,
            standalone,
            input_buffer: String::new(),
        }
    }

    /// Returns true if the TUI is in an input mode (modal dialog, text input, etc.)
    /// When true, the parent should NOT intercept navigation keys (Tab, h, etc.)
    pub fn is_in_input_mode(&self) -> bool {
        !matches!(self.input_mode, InputMode::Normal)
    }

    /// Check vault status and load unlock method
    pub fn load_vault_info(&mut self) -> Result<()> {
        self.vault_exists = CredentialVault::exists(&self.vault_path);
        if self.vault_exists {
            let vault = CredentialVault::open(&self.vault_path)?;
            self.unlock_method = vault.unlock_method().cloned();
        }
        Ok(())
    }

    /// Initialize the vault with a password
    pub fn init_vault_with_password(&mut self, password: &str) -> Result<()> {
        if self.vault_exists {
            anyhow::bail!("Vault already exists");
        }
        if password.len() < 8 {
            anyhow::bail!("Password must be at least 8 characters");
        }

        CredentialVault::init_with_password(&self.vault_path, password)?;
        self.vault_exists = true;
        self.load_vault_info()?;
        Ok(())
    }

    /// Initialize the vault with a keyfile (auto-generates if needed)
    pub fn init_vault_with_keyfile(&mut self) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;

        if self.vault_exists {
            anyhow::bail!("Vault already exists");
        }

        // Default keyfile path
        let keyfile_path = self
            .vault_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("vault.key");

        // Create parent directory if needed
        if let Some(parent) = keyfile_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Generate keyfile if it doesn't exist
        if !keyfile_path.exists() {
            use rockbot_credentials::crypto::generate_salt;
            let key_bytes = generate_salt();

            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&keyfile_path)?;

            use std::io::Write;
            file.write_all(&key_bytes)?;
        }

        CredentialVault::init_with_keyfile(&self.vault_path, &keyfile_path)?;
        self.vault_exists = true;
        self.load_vault_info()?;
        Ok(())
    }

    /// Handle key events
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Global keybindings
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('q') => {
                    self.should_exit = true;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Extract values before calling methods to avoid borrow conflicts
        let mode_info = match &self.input_mode {
            InputMode::Normal => (0, None, false),
            InputMode::AddCredential(state) => (1, Some(state.clone()), false),
            InputMode::PasswordInput { masked, .. } => (2, None, *masked),
            InputMode::Confirm(_) => (3, None, false),
        };

        match mode_info {
            (0, _, _) => self.handle_normal_mode(key),
            (1, Some(state), _) => self.handle_add_credential_mode(key, state),
            (2, _, masked) => self.handle_password_input(key, masked),
            (3, _, _) => self.handle_confirm_mode(key),
            _ => Ok(()),
        }
    }

    fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.standalone {
                    self.should_exit = true;
                }
            }
            KeyCode::Tab | KeyCode::Right => {
                let next = (self.tab.index() + 1) % 4;
                self.tab = CredentialsTab::from_index(next);
            }
            KeyCode::BackTab | KeyCode::Left => {
                let prev = if self.tab.index() == 0 {
                    3
                } else {
                    self.tab.index() - 1
                };
                self.tab = CredentialsTab::from_index(prev);
            }
            KeyCode::Char('1') => self.tab = CredentialsTab::Endpoints,
            KeyCode::Char('2') => self.tab = CredentialsTab::Permissions,
            KeyCode::Char('3') => self.tab = CredentialsTab::Audit,
            KeyCode::Char('4') => self.tab = CredentialsTab::Settings,
            KeyCode::Up | KeyCode::Char('k') => self.previous_item(),
            KeyCode::Down | KeyCode::Char('j') => self.next_item(),
            KeyCode::Char('a') => {
                if self.tab == CredentialsTab::Endpoints {
                    self.input_mode = InputMode::AddCredential(AddCredentialState::default());
                    self.status = None;
                }
            }
            KeyCode::Char('d') => {
                if self.tab == CredentialsTab::Endpoints && !self.endpoints.is_empty() {
                    if let Some(idx) = self.endpoint_state.selected() {
                        if let Some(ep) = self.endpoints.get(idx) {
                            self.input_mode = InputMode::Confirm(format!("Delete '{}'?", ep.name));
                        }
                    }
                }
            }
            KeyCode::Char('u') => {
                if !self.unlocked {
                    self.try_unlock()?;
                }
            }
            KeyCode::Char('l') => {
                self.unlocked = false;
                self.status = Some(("Vault locked".to_string(), false));
            }
            KeyCode::Char('r') | KeyCode::F(5) => {
                self.status = Some(("Refreshing...".to_string(), false));
                // TODO: Reload endpoints
            }
            KeyCode::Char('i') => {
                // Initialize vault (only if it doesn't exist)
                if !self.vault_exists {
                    self.input_mode = InputMode::PasswordInput {
                        prompt: "Create vault password (min 8 chars):".to_string(),
                        masked: true,
                    };
                    self.input_buffer.clear();
                    self.status = Some(("Initializing new vault...".to_string(), false));
                } else {
                    self.status = Some(("Vault already initialized".to_string(), true));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn try_unlock(&mut self) -> Result<()> {
        match &self.unlock_method {
            Some(UnlockMethod::Password { .. }) => {
                self.input_mode = InputMode::PasswordInput {
                    prompt: "Enter vault password:".to_string(),
                    masked: true,
                };
                self.input_buffer.clear();
            }
            Some(UnlockMethod::Keyfile { path_hint }) => {
                // Try auto-unlock with keyfile
                let kf_path = path_hint
                    .as_ref()
                    .map(PathBuf::from)
                    .or_else(|| dirs::config_dir().map(|d| d.join("rockbot").join("vault.key")));

                if let Some(path) = kf_path {
                    if path.exists() {
                        self.status =
                            Some((format!("Unlocking with keyfile: {}", path.display()), false));
                        // TODO: Actually unlock
                        self.unlocked = true;
                    } else {
                        self.status =
                            Some((format!("Keyfile not found: {}", path.display()), true));
                    }
                } else {
                    self.status = Some(("No keyfile path configured".to_string(), true));
                }
            }
            Some(UnlockMethod::Age { public_key, .. }) => {
                self.input_mode = InputMode::PasswordInput {
                    prompt: format!(
                        "Enter Age identity (pub: {}...):",
                        &public_key[..20.min(public_key.len())]
                    ),
                    masked: false,
                };
                self.input_buffer.clear();
            }
            Some(UnlockMethod::SshKey {
                public_key_path, ..
            }) => {
                self.status = Some((format!("SSH unlock: {public_key_path}"), false));
                // TODO: SSH unlock flow
            }
            None => {
                self.status = Some(("Vault not initialized".to_string(), true));
            }
        }
        Ok(())
    }

    fn handle_add_credential_mode(
        &mut self,
        key: KeyEvent,
        mut state: AddCredentialState,
    ) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
                self.status = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Tab | KeyCode::Down => {
                state.field = state.field.next();
                self.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::BackTab | KeyCode::Up => {
                state.field = state.field.prev();
                self.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Enter => {
                if state.field == AddCredentialField::Expiration {
                    // Submit the form
                    if state.name.is_empty() {
                        self.status = Some(("Name is required".to_string(), true));
                    } else if state.url.is_empty() {
                        self.status = Some(("URL is required".to_string(), true));
                    } else {
                        self.status = Some((format!("Added credential: {}", state.name), false));
                        self.input_mode = InputMode::Normal;
                        // TODO: Actually add the credential
                    }
                } else {
                    state.field = state.field.next();
                    self.input_mode = InputMode::AddCredential(state);
                }
            }
            KeyCode::Left if state.field == AddCredentialField::EndpointType => {
                if state.endpoint_type > 0 {
                    state.endpoint_type -= 1;
                } else {
                    state.endpoint_type = ENDPOINT_TYPES.len() - 1;
                }
                self.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Right if state.field == AddCredentialField::EndpointType => {
                state.endpoint_type = (state.endpoint_type + 1) % ENDPOINT_TYPES.len();
                self.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Char(c) => {
                match state.field {
                    AddCredentialField::Name => state.name.push(c),
                    AddCredentialField::Url => state.url.push(c),
                    AddCredentialField::Secret => state.secret.push(c),
                    AddCredentialField::Expiration => state.expiration.push(c),
                    AddCredentialField::EndpointType => {} // Use arrow keys
                }
                self.input_mode = InputMode::AddCredential(state);
            }
            KeyCode::Backspace => {
                match state.field {
                    AddCredentialField::Name => {
                        state.name.pop();
                    }
                    AddCredentialField::Url => {
                        state.url.pop();
                    }
                    AddCredentialField::Secret => {
                        state.secret.pop();
                    }
                    AddCredentialField::Expiration => {
                        state.expiration.pop();
                    }
                    AddCredentialField::EndpointType => {}
                }
                self.input_mode = InputMode::AddCredential(state);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_password_input(&mut self, key: KeyEvent, masked: bool) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let password = self.input_buffer.clone();
                self.input_buffer.clear();

                if masked {
                    if password.is_empty() {
                        self.status = Some(("Password cannot be empty".to_string(), true));
                        self.input_mode = InputMode::Normal;
                        return Ok(());
                    }

                    // Check if we're initializing or unlocking based on vault existence
                    if !self.vault_exists {
                        // Initialize vault with password
                        if password.len() < 8 {
                            self.status =
                                Some(("Password must be at least 8 characters".to_string(), true));
                        } else {
                            match self.init_vault_with_password(&password) {
                                Ok(()) => {
                                    self.status =
                                        Some(("✅ Vault initialized!".to_string(), false));
                                }
                                Err(e) => {
                                    self.status = Some((format!("Init failed: {e}"), true));
                                }
                            }
                        }
                    } else {
                        // Unlock existing vault
                        // TODO: Actually unlock with password
                        self.unlocked = true;
                        self.status = Some(("Vault unlocked".to_string(), false));
                    }
                } else {
                    // Age identity input
                    // TODO: Unlock with Age identity
                    self.status = Some(("Age unlock not yet implemented".to_string(), true));
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Esc => {
                self.input_buffer.clear();
                self.input_mode = InputMode::Normal;
                self.status = Some(("Cancelled".to_string(), false));
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_confirm_mode(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.status = Some(("Deleted".to_string(), false));
                // TODO: Actually delete
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.status = Some(("Cancelled".to_string(), false));
                self.input_mode = InputMode::Normal;
            }
            _ => {}
        }
        Ok(())
    }

    fn next_item(&mut self) {
        if self.endpoints.is_empty() {
            return;
        }
        let i = match self.endpoint_state.selected() {
            Some(i) => {
                if i >= self.endpoints.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.endpoint_state.select(Some(i));
    }

    fn previous_item(&mut self) {
        if self.endpoints.is_empty() {
            return;
        }
        let i = match self.endpoint_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.endpoints.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.endpoint_state.select(Some(i));
    }

    /// Render the credentials UI
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Tabs
                Constraint::Min(0),    // Content
                Constraint::Length(3), // Status/Help
            ])
            .split(area);

        self.render_tabs(frame, chunks[0]);
        self.render_content(frame, chunks[1]);
        self.render_status(frame, chunks[2]);

        // Render modal overlays
        match &self.input_mode {
            InputMode::AddCredential(state) => {
                self.render_add_credential_modal(frame, area, state);
            }
            InputMode::PasswordInput { prompt, masked } => {
                self.render_password_modal(frame, area, prompt, *masked);
            }
            InputMode::Confirm(msg) => {
                self.render_confirm_modal(frame, area, msg);
            }
            InputMode::Normal => {}
        }
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line> = CredentialsTab::titles()
            .iter()
            .map(|t| Line::from(*t))
            .collect();

        let tabs = Tabs::new(titles)
            .block(Block::default().borders(Borders::ALL).title("Credentials"))
            .select(self.tab.index())
            .style(Style::default().fg(Color::White))
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_widget(tabs, area);
    }

    fn render_content(&mut self, frame: &mut Frame, area: Rect) {
        match self.tab {
            CredentialsTab::Endpoints => self.render_endpoints(frame, area),
            CredentialsTab::Permissions => self.render_permissions(frame, area),
            CredentialsTab::Audit => self.render_audit(frame, area),
            CredentialsTab::Settings => self.render_settings(frame, area),
        }
    }

    fn render_endpoints(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        // Endpoint list
        let items: Vec<ListItem> = self
            .endpoints
            .iter()
            .map(|e| {
                let style = if e.has_credential {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(&e.name, style),
                    Span::raw(" "),
                    Span::styled(
                        format!("({})", e.endpoint_type),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect();

        let items = if items.is_empty() {
            vec![ListItem::new(Span::styled(
                "No endpoints. Press 'a' to add.",
                Style::default().fg(Color::DarkGray),
            ))]
        } else {
            items
        };

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Endpoints"))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        frame.render_stateful_widget(list, chunks[0], &mut self.endpoint_state);

        // Endpoint details
        let detail_block = Block::default().borders(Borders::ALL).title("Details");

        if let Some(selected) = self.endpoint_state.selected() {
            if let Some(endpoint) = self.endpoints.get(selected) {
                let details = vec![
                    Line::from(vec![
                        Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&endpoint.id),
                    ]),
                    Line::from(vec![
                        Span::styled("Type: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&endpoint.endpoint_type),
                    ]),
                    Line::from(vec![
                        Span::styled("URL: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&endpoint.url),
                    ]),
                    Line::from(vec![
                        Span::styled("Credential: ", Style::default().fg(Color::Cyan)),
                        Span::raw(if endpoint.has_credential {
                            "✓ Stored"
                        } else {
                            "✗ Missing"
                        }),
                    ]),
                    Line::from(vec![
                        Span::styled("Expires: ", Style::default().fg(Color::Cyan)),
                        Span::raw(endpoint.expiration.as_deref().unwrap_or("Never")),
                    ]),
                ];
                let paragraph = Paragraph::new(details).block(detail_block);
                frame.render_widget(paragraph, chunks[1]);
            } else {
                frame.render_widget(detail_block, chunks[1]);
            }
        } else {
            frame.render_widget(detail_block, chunks[1]);
        }
    }

    #[allow(clippy::unused_self)]
    fn render_permissions(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Permission Rules");

        let text = vec![
            Line::from("Permission rules control agent access to credentials."),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'a' to add a rule",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
    }

    #[allow(clippy::unused_self)]
    fn render_audit(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title("Audit Log");

        let text = vec![
            Line::from("Audit log tracks all credential access."),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'v' to verify integrity",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
    }

    fn render_settings(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Vault Settings");

        if !self.vault_exists {
            // Vault not initialized - show init instructions
            let text = vec![
                Line::from(vec![Span::styled(
                    "⚠️  Vault not initialized",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("Vault Path: ", Style::default().fg(Color::Cyan)),
                    Span::raw(self.vault_path.display().to_string()),
                ]),
                Line::from(""),
                Line::from("The credential vault needs to be initialized before"),
                Line::from("you can store API keys and secrets."),
                Line::from(""),
                Line::from(Span::styled(
                    "Press 'i' to initialize with password",
                    Style::default().fg(Color::Green),
                )),
                Line::from(Span::styled(
                    "Or use CLI: rockbot credentials init",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let paragraph = Paragraph::new(text).block(block);
            frame.render_widget(paragraph, area);
            return;
        }

        let unlock_str = match &self.unlock_method {
            Some(UnlockMethod::Password { .. }) => "Password (Argon2id)",
            Some(UnlockMethod::Keyfile { path_hint }) => path_hint
                .as_ref()
                .map_or("Keyfile", std::string::String::as_str),
            Some(UnlockMethod::Age { .. }) => "Age encryption",
            Some(UnlockMethod::SshKey { .. }) => "SSH key",
            None => "Unknown",
        };

        let status_str = if self.unlocked {
            "🔓 Unlocked"
        } else {
            "🔒 Locked"
        };

        let text = vec![
            Line::from(vec![
                Span::styled("Initialized: ", Style::default().fg(Color::Cyan)),
                Span::styled("✓ Yes", Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::raw(status_str),
            ]),
            Line::from(vec![
                Span::styled("Unlock Method: ", Style::default().fg(Color::Cyan)),
                Span::raw(unlock_str),
            ]),
            Line::from(vec![
                Span::styled("Vault Path: ", Style::default().fg(Color::Cyan)),
                Span::raw(self.vault_path.display().to_string()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'u' to unlock, 'l' to lock",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let (status_text, style) = if let Some((msg, is_error)) = &self.status {
            let style = if *is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            (msg.clone(), style)
        } else {
            let help = match &self.input_mode {
                InputMode::Normal => {
                    if self.vault_exists {
                        "q:Quit | Tab:Switch | j/k:Navigate | a:Add | d:Delete | u:Unlock | l:Lock"
                    } else {
                        "q:Quit | Tab:Switch | i:Initialize vault"
                    }
                }
                InputMode::AddCredential(_) => {
                    "Tab/↑↓:Fields | ←→:Select | Enter:Next/Submit | Esc:Cancel"
                }
                InputMode::PasswordInput { .. } => "Enter:Submit | Esc:Cancel",
                InputMode::Confirm(_) => "y:Yes | n:No | Esc:Cancel",
            };
            (help.to_string(), Style::default().fg(Color::DarkGray))
        };

        let status = Paragraph::new(status_text)
            .style(style)
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(status, area);
    }

    #[allow(clippy::unused_self)]
    fn render_add_credential_modal(
        &self,
        frame: &mut Frame,
        area: Rect,
        state: &AddCredentialState,
    ) {
        let modal_area = centered_rect(60, 70, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Add Service Endpoint");

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Name
                Constraint::Length(3), // Type
                Constraint::Length(3), // URL
                Constraint::Length(3), // Secret
                Constraint::Length(3), // Expiration
                Constraint::Min(1),    // Spacer
            ])
            .split(inner);

        // Helper to create input field
        let render_field = |frame: &mut Frame,
                            area: Rect,
                            label: &str,
                            value: &str,
                            active: bool,
                            masked: bool| {
            let style = if active {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            let display_value = if masked && !value.is_empty() {
                "*".repeat(value.len())
            } else {
                value.to_string()
            };

            let cursor = if active { "█" } else { "" };

            let text = format!("{label}: {display_value}{cursor}");
            let paragraph = Paragraph::new(text)
                .style(style)
                .block(Block::default().borders(Borders::ALL));
            frame.render_widget(paragraph, area);
        };

        render_field(
            frame,
            chunks[0],
            "Endpoint Name",
            &state.name,
            state.field == AddCredentialField::Name,
            false,
        );

        // Service Type selector
        let type_style = if state.field == AddCredentialField::EndpointType {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        let type_name = ENDPOINT_TYPES
            .get(state.endpoint_type)
            .map_or("Unknown", |(_, n)| *n);
        let type_text = format!("Service Type: ◀ {type_name} ▶");
        let type_para = Paragraph::new(type_text)
            .style(type_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(type_para, chunks[1]);

        render_field(
            frame,
            chunks[2],
            "Base URL",
            &state.url,
            state.field == AddCredentialField::Url,
            false,
        );
        render_field(
            frame,
            chunks[3],
            "Token/Secret",
            &state.secret,
            state.field == AddCredentialField::Secret,
            true,
        );
        render_field(
            frame,
            chunks[4],
            "Expires (opt)",
            &state.expiration,
            state.field == AddCredentialField::Expiration,
            false,
        );
    }

    fn render_password_modal(&self, frame: &mut Frame, area: Rect, prompt: &str, masked: bool) {
        let modal_area = centered_rect(50, 20, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Unlock Vault");

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(1), // Prompt
                Constraint::Length(3), // Input
            ])
            .split(inner);

        let prompt_para = Paragraph::new(prompt).style(Style::default().fg(Color::White));
        frame.render_widget(prompt_para, chunks[0]);

        let display_value = if masked {
            "*".repeat(self.input_buffer.len())
        } else {
            self.input_buffer.clone()
        };

        let input_para = Paragraph::new(format!("{display_value}█"))
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(input_para, chunks[1]);
    }

    #[allow(clippy::unused_self)]
    fn render_confirm_modal(&self, frame: &mut Frame, area: Rect, message: &str) {
        let modal_area = centered_rect(40, 15, area);
        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title("Confirm");

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let text = vec![
            Line::from(message.to_string()),
            Line::from(""),
            Line::from(Span::styled(
                "[y]es  [n]o",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let para = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(para, inner);
    }
}

/// Create a centered rectangle
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Run the credentials TUI standalone
pub async fn run_credentials_tui(vault_path: PathBuf) -> Result<()> {
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

    // Create app state
    let mut tui = CredentialsTui::new(vault_path, true);
    tui.load_vault_info()?;

    // Event handler
    let events = super::event::EventHandler::new(250);

    // Main loop
    loop {
        terminal.draw(|frame| {
            tui.render(frame, frame.area());
        })?;

        match events.next()? {
            super::event::Event::Key(key) => {
                tui.handle_key(key)?;
            }
            super::event::Event::Tick => {}
            _ => {}
        }

        if tui.should_exit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
