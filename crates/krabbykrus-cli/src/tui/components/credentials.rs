//! Credentials/Vault management component
//!
//! Organized by credential categories:
//! - Model Providers (Anthropic, OpenAI, Google, AWS Bedrock, Ollama)
//! - Communication Providers (Discord, Telegram, Signal, Slack)
//! - Tool Providers (Home Assistant, GitHub, etc.)
//! - OAuth2 Services (Google, Microsoft, etc.)
//! - Generic (API Keys, Bearer Tokens, Basic Auth)

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
    OAuth2Services,
    Generic,
}

impl CredentialCategory {
    pub fn all() -> Vec<Self> {
        vec![
            Self::All,
            Self::ModelProviders,
            Self::CommunicationProviders,
            Self::ToolProviders,
            Self::OAuth2Services,
            Self::Generic,
        ]
    }
    
    pub fn title(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::ModelProviders => "Model Providers",
            Self::CommunicationProviders => "Communication",
            Self::ToolProviders => "Tools",
            Self::OAuth2Services => "OAuth2",
            Self::Generic => "Generic",
        }
    }
    
    pub fn icon(&self) -> &'static str {
        match self {
            Self::All => "📋",
            Self::ModelProviders => "🧠",
            Self::CommunicationProviders => "💬",
            Self::ToolProviders => "🔧",
            Self::OAuth2Services => "🔐",
            Self::Generic => "🔑",
        }
    }
    
    pub fn description(&self) -> &'static str {
        match self {
            Self::All => "All configured credentials",
            Self::ModelProviders => "LLM API keys (Anthropic, OpenAI, Google, AWS)",
            Self::CommunicationProviders => "Messaging services (Discord, Telegram, Signal)",
            Self::ToolProviders => "Tool integrations (Home Assistant, GitHub)",
            Self::OAuth2Services => "OAuth2 authenticated services",
            Self::Generic => "API keys, tokens, and basic auth",
        }
    }
}

/// Known model provider templates
pub const MODEL_PROVIDERS: &[(&str, &str, &str)] = &[
    ("anthropic", "Anthropic", "Claude API (Opus, Sonnet, Haiku)"),
    ("openai", "OpenAI", "GPT-4, GPT-4o, o1 models"),
    ("google", "Google AI", "Gemini models"),
    ("bedrock", "AWS Bedrock", "Claude, Llama, Titan via AWS"),
    ("ollama", "Ollama", "Local models (no API key)"),
];

/// Known communication provider templates
pub const COMMUNICATION_PROVIDERS: &[(&str, &str, &str)] = &[
    ("discord", "Discord", "Discord bot token"),
    ("telegram", "Telegram", "Telegram bot token"),
    ("signal", "Signal", "Signal credentials"),
    ("slack", "Slack", "Slack bot/app token"),
    ("whatsapp", "WhatsApp", "WhatsApp Business API"),
];

/// Known tool provider templates
pub const TOOL_PROVIDERS: &[(&str, &str, &str)] = &[
    ("home_assistant", "Home Assistant", "Long-lived access token"),
    ("github", "GitHub", "Personal access token"),
    ("gitlab", "GitLab", "Personal access token"),
    ("jira", "Jira", "API token"),
    ("notion", "Notion", "Integration token"),
];

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
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    // Category list
    let list_border_style = if !state.sidebar_focus {
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
            .title("Categories"))
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");
    
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_category));
    
    frame.render_stateful_widget(list, chunks[0], &mut list_state);
    
    // Provider details - show providers for selected category
    let selected_category = categories.get(state.selected_category).copied().unwrap_or(CredentialCategory::All);
    render_category_providers(frame, chunks[1], state, selected_category);
}

/// Render provider list for a category
fn render_category_providers(frame: &mut Frame, area: Rect, state: &AppState, category: CredentialCategory) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{} {}", category.icon(), category.title()));
    
    let mut content = vec![
        Line::from(Span::styled(category.description(), Style::default().fg(Color::DarkGray))),
        Line::from(""),
    ];
    
    // Show providers based on category
    match category {
        CredentialCategory::All => {
            content.push(Line::from(Span::styled("── Model Providers ──", Style::default().fg(Color::Cyan))));
            for (id, name, desc) in MODEL_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                    Span::styled(format!(" - {}", desc), Style::default().fg(Color::DarkGray)),
                ]));
            }
            content.push(Line::from(""));
            
            content.push(Line::from(Span::styled("── Communication ──", Style::default().fg(Color::Cyan))));
            for (id, name, desc) in COMMUNICATION_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                    Span::styled(format!(" - {}", desc), Style::default().fg(Color::DarkGray)),
                ]));
            }
            content.push(Line::from(""));
            
            content.push(Line::from(Span::styled("── Tools ──", Style::default().fg(Color::Cyan))));
            for (id, name, desc) in TOOL_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                    Span::styled(format!(" - {}", desc), Style::default().fg(Color::DarkGray)),
                ]));
            }
        }
        CredentialCategory::ModelProviders => {
            for (id, name, desc) in MODEL_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                ]));
                content.push(Line::from(Span::styled(format!("   {}", desc), Style::default().fg(Color::DarkGray))));
            }
        }
        CredentialCategory::CommunicationProviders => {
            for (id, name, desc) in COMMUNICATION_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                ]));
                content.push(Line::from(Span::styled(format!("   {}", desc), Style::default().fg(Color::DarkGray))));
            }
        }
        CredentialCategory::ToolProviders => {
            for (id, name, desc) in TOOL_PROVIDERS {
                let configured = check_provider_configured(state, id);
                let indicator = if configured { "●" } else { "○" };
                let color = if configured { Color::Green } else { Color::Yellow };
                content.push(Line::from(vec![
                    Span::styled(format!(" {} ", indicator), Style::default().fg(color)),
                    Span::styled(*name, Style::default().fg(Color::White)),
                ]));
                content.push(Line::from(Span::styled(format!("   {}", desc), Style::default().fg(Color::DarkGray))));
            }
        }
        CredentialCategory::OAuth2Services | CredentialCategory::Generic => {
            content.push(Line::from(Span::styled(
                "Configure via Endpoints tab with appropriate type",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Press 'a' to add credentials for selected provider",
        Style::default().fg(Color::DarkGray),
    )));
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
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
