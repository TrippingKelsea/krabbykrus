//! Credentials/Vault management component

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
    Frame,
};

use crate::tui::state::AppState;

/// Credential tabs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsTab {
    Endpoints,
    Permissions,
    Audit,
    Settings,
}

impl CredentialsTab {
    pub fn all() -> Vec<Self> {
        vec![Self::Endpoints, Self::Permissions, Self::Audit, Self::Settings]
    }
    
    pub fn title(&self) -> &'static str {
        match self {
            Self::Endpoints => "Endpoints",
            Self::Permissions => "Permissions",
            Self::Audit => "Audit",
            Self::Settings => "Settings",
        }
    }
}

/// Render the credentials page
pub fn render_credentials(frame: &mut Frame, area: Rect, state: &AppState, selected_tab: usize) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Tabs
            Constraint::Min(0),     // Content
        ])
        .split(area);

    // Render tabs
    let titles: Vec<Line> = CredentialsTab::all()
        .iter()
        .map(|t| Line::from(t.title()))
        .collect();
    
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("Credentials"))
        .select(selected_tab)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    
    frame.render_widget(tabs, chunks[0]);

    // Render content based on vault state
    if !state.vault.initialized {
        render_vault_init(frame, chunks[1], state);
    } else if state.vault.locked {
        render_vault_locked(frame, chunks[1], state);
    } else {
        match selected_tab {
            0 => render_endpoints(frame, chunks[1], state),
            1 => render_permissions(frame, chunks[1], state),
            2 => render_audit(frame, chunks[1], state),
            3 => render_settings(frame, chunks[1], state),
            _ => {}
        }
    }
}

fn render_vault_init(frame: &mut Frame, area: Rect, state: &AppState) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "⚠️  Vault not initialized",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Vault Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
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
            "Or use CLI: krabbykrus credentials init",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Initialize Vault");
    
    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center);
    
    frame.render_widget(paragraph, area);
}

fn render_vault_locked(frame: &mut Frame, area: Rect, _state: &AppState) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled("🔒 Vault Locked", Style::default().fg(Color::Yellow))),
        Line::from(""),
        Line::from("Enter your password to unlock the vault."),
        Line::from(""),
        Line::from(Span::styled("Press 'u' to unlock", Style::default().fg(Color::Green))),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Unlock Vault");
    
    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center);
    
    frame.render_widget(paragraph, area);
}

fn render_endpoints(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // Endpoint list
    let items: Vec<ListItem> = if state.endpoints.is_empty() {
        vec![ListItem::new(Span::styled(
            "No endpoints. Press 'a' to add.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        state.endpoints.iter().map(|e| {
            let style = if e.has_credential {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Yellow)
            };
            ListItem::new(Line::from(vec![
                Span::styled(&e.name, style),
                Span::raw(" "),
                Span::styled(format!("({})", e.endpoint_type), Style::default().fg(Color::DarkGray)),
            ]))
        }).collect()
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Endpoints"))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    if !state.endpoints.is_empty() {
        list_state.select(Some(state.selected_endpoint));
    }
    
    frame.render_stateful_widget(list, chunks[0], &mut list_state);

    // Endpoint details
    let detail_block = Block::default()
        .borders(Borders::ALL)
        .title("Details");

    if let Some(endpoint) = state.endpoints.get(state.selected_endpoint) {
        let cred_status = if endpoint.has_credential { "✓ Stored" } else { "✗ Missing" };
        let cred_color = if endpoint.has_credential { Color::Green } else { Color::Red };
        
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
                Span::raw(&endpoint.base_url),
            ]),
            Line::from(vec![
                Span::styled("Credential: ", Style::default().fg(Color::Cyan)),
                Span::styled(cred_status, Style::default().fg(cred_color)),
            ]),
            Line::from(vec![
                Span::styled("Expires: ", Style::default().fg(Color::Cyan)),
                Span::raw(endpoint.expiration.as_deref().unwrap_or("Never")),
            ]),
            Line::from(""),
            Line::from(Span::styled("[d]elete  [e]dit  [r]efresh", Style::default().fg(Color::DarkGray))),
        ];
        let paragraph = Paragraph::new(details).block(detail_block);
        frame.render_widget(paragraph, chunks[1]);
    } else {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("Select an endpoint", Style::default().fg(Color::DarkGray))),
        ])
        .block(detail_block)
        .alignment(Alignment::Center);
        frame.render_widget(empty, chunks[1]);
    }
}

fn render_permissions(frame: &mut Frame, area: Rect, _state: &AppState) {
    let content = vec![
        Line::from(""),
        Line::from("Permission rules control agent access to credentials."),
        Line::from(""),
        Line::from(Span::styled(
            "Press 'a' to add a rule",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Permission Rules");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_audit(frame: &mut Frame, area: Rect, _state: &AppState) {
    let content = vec![
        Line::from(""),
        Line::from("Audit log tracks all credential access."),
        Line::from(""),
        Line::from(Span::styled(
            "Press 'v' to verify integrity",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Audit Log");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_settings(frame: &mut Frame, area: Rect, state: &AppState) {
    let unlock_method = state.vault.unlock_method.label();
    let lock_status = if state.vault.locked { "🔒 Locked" } else { "🔓 Unlocked" };
    
    // Show different help text based on unlock method
    let unlock_hint = if state.vault.locked {
        match &state.vault.unlock_method {
            crate::tui::state::UnlockMethod::Keyfile { .. } => "Press 'u' to auto-unlock with keyfile",
            crate::tui::state::UnlockMethod::Password => "Press 'u' to enter password",
            _ => "Press 'u' to unlock",
        }
    } else {
        "Press 'l' to lock"
    };
    
    let content = vec![
        Line::from(vec![
            Span::styled("Initialized: ", Style::default().fg(Color::Cyan)),
            Span::styled("✓ Yes", Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::raw(lock_status),
        ]),
        Line::from(vec![
            Span::styled("Unlock Method: ", Style::default().fg(Color::Cyan)),
            Span::raw(unlock_method),
        ]),
        Line::from(vec![
            Span::styled("Vault Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            unlock_hint,
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Vault Settings");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}
