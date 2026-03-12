//! Models/Providers component
//!
//! Shows LLM provider configuration status dynamically loaded from the gateway.
//! The gateway is the single source of truth for which providers are registered
//! and available.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;

/// Render the models page
pub fn render_models(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    if state.providers.is_empty() {
        render_no_providers(frame, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    render_provider_list(frame, chunks[0], state, effect_state);
    render_provider_details(frame, chunks[1], state, effect_state);
}

fn render_no_providers(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("LLM Providers");

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "No providers loaded",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(""),
        Line::from("Providers are loaded from the gateway."),
        Line::from("Make sure the gateway is running:"),
        Line::from(""),
        Line::from(Span::styled(
            "  rockbot gateway run",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Providers will appear here once the gateway is started.",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_provider_list(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("Models");

    // Build a tree view: provider header + indented models
    let mut items: Vec<ListItem> = Vec::new();
    let mut tree_index_to_provider: Vec<usize> = Vec::new(); // maps list row -> provider index

    for (pi, provider) in state.providers.iter().enumerate() {
        let (indicator, indicator_style) = if provider.available {
            ("● ", Style::default().fg(palette::CONFIGURED))
        } else {
            ("○ ", Style::default().fg(palette::UNCONFIGURED))
        };

        // Provider header row
        items.push(ListItem::new(Line::from(vec![
            Span::styled(indicator, indicator_style),
            Span::styled(
                &provider.name,
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({})", provider.models.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ])));
        tree_index_to_provider.push(pi);

        // Model rows (indented)
        for model in &provider.models {
            let ctx = format!(" {}k", model.context_window / 1000);
            items.push(ListItem::new(Line::from(vec![
                Span::raw("    "),
                Span::styled("• ", Style::default().fg(Color::DarkGray)),
                Span::raw(&model.name),
                Span::styled(ctx, Style::default().fg(Color::DarkGray)),
            ])));
            tree_index_to_provider.push(pi);
        }
    }

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
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    // Find the row index for the selected provider
    let selected_row = tree_index_to_provider
        .iter()
        .position(|&pi| pi == state.selected_provider)
        .unwrap_or(0);

    let mut list_state = ListState::default();
    list_state.select(Some(selected_row));

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_provider_details(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Provider Details");

    let idx = state.selected_provider.min(state.providers.len().saturating_sub(1));
    let Some(provider) = state.providers.get(idx) else {
        let paragraph = Paragraph::new("No provider selected").block(block);
        frame.render_widget(paragraph, area);
        return;
    };

    let status_color = if provider.available {
        palette::CONFIGURED
    } else {
        palette::UNCONFIGURED
    };
    let status_text = if provider.available {
        "✓ Available"
    } else {
        "○ Not Available"
    };

    let mut content = vec![
        Line::from(Span::styled(
            &provider.name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("Provider ID: {}", provider.id),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]),
        Line::from(vec![
            Span::styled("Auth: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                auth_type_label(&provider.auth_type),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    // Capabilities
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Capabilities:",
        Style::default().fg(Color::Cyan),
    )));

    let cap_items = [
        ("Streaming", provider.supports_streaming),
        ("Tool Use", provider.supports_tools),
        ("Vision", provider.supports_vision),
    ];

    for (name, supported) in cap_items {
        let (icon, color) = if supported {
            ("✓", Color::Green)
        } else {
            ("✗", Color::DarkGray)
        };
        content.push(Line::from(vec![
            Span::styled(format!("  {icon} "), Style::default().fg(color)),
            Span::raw(name),
        ]));
    }

    // Models
    if !provider.models.is_empty() {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            format!("Models ({}):", provider.models.len()),
            Style::default().fg(Color::Cyan),
        )));

        for model in provider.models.iter().take(8) {
            let tokens_info = model
                .max_output_tokens.map_or_else(|| format!(" ({}k ctx)", model.context_window / 1000), |t| format!(" ({}k ctx, {}k out)", model.context_window / 1000, t / 1000));

            content.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::DarkGray)),
                Span::styled(&model.name, Style::default().fg(Color::White)),
                Span::styled(tokens_info, Style::default().fg(Color::DarkGray)),
            ]));
        }

        if provider.models.len() > 8 {
            content.push(Line::from(Span::styled(
                format!("  ... and {} more", provider.models.len() - 8),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Configuration hints
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "─── Configuration ───",
        Style::default().fg(Color::DarkGray),
    )));
    content.push(Line::from(""));
    // Find matching credential schema for this provider
    let schema = state.credential_schemas.iter().find(|s| s.provider_id == provider.id);
    render_auth_hints(&provider.id, &provider.auth_type, &mut content, schema);

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[e]dit config  [t]est connection",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn auth_type_label(auth_type: &str) -> &str {
    match auth_type {
        "aws_credentials" => "AWS Credentials",
        "oauth" => "OAuth (Claude Code)",
        "api_key" => "API Key",
        "none" => "None required",
        _ => auth_type,
    }
}

fn render_auth_hints(
    _provider_id: &str,
    auth_type: &str,
    content: &mut Vec<Line>,
    schema: Option<&crate::tui::state::CredentialSchemaInfo>,
) {
    // If we have a schema, show env var hints from the default auth method's fields
    if let Some(schema) = schema {
        if let Some(method) = schema.auth_methods.first() {
            // Show env var export hints for fields that have env_var set
            let env_fields: Vec<_> = method.fields.iter()
                .filter(|f| f.env_var.is_some())
                .collect();

            if !env_fields.is_empty() {
                for field in &env_fields {
                    if let Some(env_var) = &field.env_var {
                        let detected = std::env::var(env_var).is_ok();
                        let indicator = if detected { "✓" } else { " " };
                        let color = if detected { Color::Green } else { Color::DarkGray };
                        let value_hint = field.default.as_deref().unwrap_or("\"...\"");

                        content.push(Line::from(vec![
                            Span::styled(format!("  {indicator} export "), Style::default().fg(color)),
                            Span::styled(env_var.clone(), Style::default().fg(Color::Yellow)),
                            Span::styled(format!("={value_hint}"), Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                }
            }

            // Show the method hint
            if let Some(hint) = &method.hint {
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    hint.clone(),
                    Style::default().fg(Color::Gray),
                )));
            }

            // Show auth method count
            let method_count = schema.auth_methods.len();
            if method_count > 1 {
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    format!("{method_count} auth methods available — press [e] to configure"),
                    Style::default().fg(Color::DarkGray),
                )));
            }

            return;
        }
    }

    // Fallback: static hints for when schema is not available
    match auth_type {
        "aws_credentials" => {
            content.push(Line::from("Configure via AWS credential chain:"));
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled("AWS_ACCESS_KEY_ID", Style::default().fg(Color::Yellow)),
                Span::styled("=\"...\"", Style::default().fg(Color::DarkGray)),
            ]));
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled("AWS_SECRET_ACCESS_KEY", Style::default().fg(Color::Yellow)),
                Span::styled("=\"...\"", Style::default().fg(Color::DarkGray)),
            ]));
        }
        "oauth" => {
            content.push(Line::from("Uses Claude Code OAuth credentials."));
            content.push(Line::from(Span::styled(
                "  Run: claude (to authenticate)",
                Style::default().fg(Color::Yellow),
            )));
        }
        "api_key" => {
            content.push(Line::from(Span::styled(
                "  Set API key via environment or press [e] to configure.",
                Style::default().fg(Color::Gray),
            )));
        }
        "none" => {
            content.push(Line::from(Span::styled(
                "No configuration needed.",
                Style::default().fg(Color::Green),
            )));
        }
        _ => {
            content.push(Line::from(Span::styled(
                "Press [e] to configure.",
                Style::default().fg(Color::Gray),
            )));
        }
    }
}
