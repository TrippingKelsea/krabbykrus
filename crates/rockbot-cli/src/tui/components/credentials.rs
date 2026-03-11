//! Credentials/Vault management component
//!
//! Provider categories are populated dynamically from the gateway's credential
//! schema registries (LLM, Channel, Tool). When the gateway is not running,
//! the provider list shows an empty state message.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;

/// Credential categories for organization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialCategory {
    #[default]
    All,
    ModelProviders,
    CommunicationProviders,
    ToolProviders,
}

impl CredentialCategory {
    pub fn all() -> Vec<Self> {
        vec![
            Self::All,
            Self::ModelProviders,
            Self::CommunicationProviders,
            Self::ToolProviders,
        ]
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::ModelProviders => "Model Providers",
            Self::CommunicationProviders => "Communication",
            Self::ToolProviders => "Tools",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::All => "📋",
            Self::ModelProviders => "🧠",
            Self::CommunicationProviders => "💬",
            Self::ToolProviders => "🔧",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::All => "All configured credentials",
            Self::ModelProviders => "LLM API keys (Anthropic, OpenAI, Google, AWS)",
            Self::CommunicationProviders => "Messaging services (Discord, Telegram, Signal)",
            Self::ToolProviders => "Tool integrations (MCP servers, etc.)",
        }
    }
}

/// Credential tabs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsTab {
    Endpoints,
    Providers,
    Permissions,
    Audit,
}

impl CredentialsTab {
    pub fn all() -> Vec<Self> {
        vec![Self::Endpoints, Self::Providers, Self::Permissions, Self::Audit]
    }
    
    pub fn title(&self) -> &'static str {
        match self {
            Self::Endpoints => "Endpoints",
            Self::Providers => "Providers",
            Self::Permissions => "Permissions",
            Self::Audit => "Audit",
        }
    }
}

/// Render the credentials page
pub fn render_credentials(
    frame: &mut Frame, 
    area: Rect, 
    state: &AppState, 
    selected_tab: usize,
    effect_state: &EffectState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Tabs
            Constraint::Min(0),     // Content
        ])
        .split(area);

    // Render tabs with active styling when content is focused
    let titles: Vec<Line> = CredentialsTab::all()
        .iter()
        .map(|t| Line::from(t.title()))
        .collect();
    
    let tab_border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
    let tabs = Tabs::new(titles)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(tab_border_style)
            .title("Credentials"))
        .select(selected_tab)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(palette::ACTIVE_PRIMARY)
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
            0 => render_endpoints(frame, chunks[1], state, effect_state),
            1 => render_providers(frame, chunks[1], state, effect_state),
            2 => render_permissions(frame, chunks[1], state),
            3 => render_audit(frame, chunks[1], state),
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
            "Or use CLI: rockbot credentials init",
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

fn render_endpoints(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // Active border style for list
    let list_border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };

    // Endpoint list with categories
    let items: Vec<ListItem> = if state.endpoints.is_empty() {
        vec![ListItem::new(Span::styled(
            "No endpoints. Press 'a' to add.",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        state.endpoints.iter().map(|e| {
            let (icon, category_color) = get_endpoint_category_style(&e.endpoint_type);
            let cred_indicator = if e.has_credential {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::Yellow))
            };
            
            ListItem::new(Line::from(vec![
                cred_indicator,
                Span::styled(format!("{} ", icon), Style::default().fg(category_color)),
                Span::raw(&e.name),
                Span::styled(format!(" ({})", &e.endpoint_type), Style::default().fg(Color::DarkGray)),
            ]))
        }).collect()
    };

    let highlight_style = if !state.sidebar_focus {
        Style::default()
            .bg(palette::ACTIVE_PRIMARY)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    };

    let list = List::new(items)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(list_border_style)
            .title("Endpoints"))
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

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
        let (icon, _) = get_endpoint_category_style(&endpoint.endpoint_type);
        
        let details = vec![
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                Span::raw(&endpoint.id),
            ]),
            Line::from(vec![
                Span::styled("Type: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{} {}", icon, &endpoint.endpoint_type)),
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

/// Render the Providers tab - categorized credential templates
fn render_providers(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    // Category list - active when provider_list_focus is false
    let category_active = !state.sidebar_focus && !state.provider_list_focus;
    let cat_border_style = if category_active {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
    let categories = CredentialCategory::all();
    let items: Vec<ListItem> = categories.iter().map(|cat| {
        ListItem::new(Line::from(vec![
            Span::raw(format!("{} ", cat.icon())),
            Span::raw(cat.title()),
        ]))
    }).collect();
    
    let cat_highlight = if category_active {
        Style::default()
            .bg(palette::ACTIVE_PRIMARY)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    };
    
    let list = List::new(items)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(cat_border_style)
            .title("Categories"))
        .highlight_style(cat_highlight)
        .highlight_symbol("▶ ");
    
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_category));
    
    frame.render_stateful_widget(list, chunks[0], &mut list_state);
    
    // Provider list - active when provider_list_focus is true
    let selected_category = categories.get(state.selected_category).copied().unwrap_or(CredentialCategory::All);
    render_category_providers(frame, chunks[1], state, selected_category, effect_state);
}

/// Render provider list for a category
fn render_category_providers(frame: &mut Frame, area: Rect, state: &AppState, category: CredentialCategory, effect_state: &EffectState) {
    // Provider list is active when provider_list_focus is true
    let provider_active = !state.sidebar_focus && state.provider_list_focus;
    let border_style = if provider_active {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!("{} {} (→ to select, a to add)", category.icon(), category.title()));
    
    // Build provider list dynamically from credential_schemas
    let schemas = &state.credential_schemas;

    // If no schemas are loaded (gateway not running), show helpful empty state
    if schemas.is_empty() {
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Start the gateway to see providers",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Run: rockbot gateway start",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content).block(block).alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
        return;
    }

    let filtered: Vec<&crate::tui::state::CredentialSchemaInfo> = match category {
        CredentialCategory::All => schemas.iter().collect(),
        CredentialCategory::ModelProviders => schemas.iter().filter(|s| s.category == "model").collect(),
        CredentialCategory::CommunicationProviders => schemas.iter().filter(|s| s.category == "communication").collect(),
        CredentialCategory::ToolProviders => schemas.iter().filter(|s| s.category == "tool").collect(),
    };

    if filtered.is_empty() {
        let content = vec![
            Line::from(Span::styled(category.description(), Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled(
                "No providers registered for this category",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, area);
        return;
    }

    // Render as a navigable list
    let items: Vec<ListItem> = filtered.iter().enumerate().map(|(_idx, schema)| {
        let configured = check_provider_configured(state, &schema.provider_id);
        let indicator = if configured { "●" } else { "○" };
        let ind_color = if configured { Color::Green } else { Color::Yellow };

        let cat_icon = match schema.category.as_str() {
            "model" => "🧠 ",
            "communication" => "💬 ",
            "tool" => "🔧 ",
            _ => "",
        };

        let prefix = if category == CredentialCategory::All { cat_icon } else { "" };

        ListItem::new(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("{} ", indicator), Style::default().fg(ind_color)),
            Span::styled(schema.provider_name.as_str(), Style::default().fg(Color::White)),
            Span::styled(format!(" ({})", schema.provider_id), Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();
    
    let highlight_style = if provider_active {
        Style::default()
            .bg(palette::ACTIVE_PRIMARY)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    };
    
    let list = List::new(items)
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");
    
    let mut list_state = ListState::default();
    if provider_active || state.provider_list_focus {
        list_state.select(Some(state.selected_provider_index.min(filtered.len().saturating_sub(1))));
    }
    
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Check if a provider is configured in state
fn check_provider_configured(state: &AppState, provider_id: &str) -> bool {
    state.endpoints.iter().any(|e| {
        e.id.to_lowercase().contains(provider_id) || 
        e.name.to_lowercase().contains(provider_id)
    })
}

/// Get icon and color for endpoint type category
fn get_endpoint_category_style(endpoint_type: &str) -> (&'static str, Color) {
    let et_lower = endpoint_type.to_lowercase();
    
    // Model providers
    if et_lower.contains("anthropic") || et_lower.contains("openai") || 
       et_lower.contains("google") || et_lower.contains("bedrock") ||
       et_lower.contains("ollama") || et_lower.contains("llm") {
        return ("🧠", palette::ACTIVE_PRIMARY);
    }
    
    // Communication providers
    if et_lower.contains("discord") || et_lower.contains("telegram") ||
       et_lower.contains("signal") || et_lower.contains("slack") ||
       et_lower.contains("whatsapp") {
        return ("💬", Color::Blue);
    }
    
    // Tool providers
    if et_lower.contains("home_assistant") || et_lower.contains("homeassistant") ||
       et_lower.contains("github") || et_lower.contains("gitlab") ||
       et_lower.contains("jira") || et_lower.contains("notion") {
        return ("🔧", Color::Cyan);
    }
    
    // OAuth2
    if et_lower.contains("oauth") {
        return ("🔐", palette::VAULT_HINT);
    }
    
    // Generic
    ("🔑", Color::Gray)
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
