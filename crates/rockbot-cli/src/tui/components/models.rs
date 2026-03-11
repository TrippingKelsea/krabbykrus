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
        .title("LLM Providers");

    let items: Vec<ListItem> = state
        .providers
        .iter()
        .map(|provider| {
            let (indicator, indicator_style) = if provider.available {
                ("● ", Style::default().fg(palette::CONFIGURED))
            } else {
                ("○ ", Style::default().fg(palette::UNCONFIGURED))
            };

            ListItem::new(Line::from(vec![
                Span::styled(indicator, indicator_style),
                Span::raw(&provider.name),
            ]))
        })
        .collect();

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

    let mut list_state = ListState::default();
    let idx = state.selected_provider.min(state.providers.len().saturating_sub(1));
    list_state.select(Some(idx));

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
            Span::styled(format!("  {} ", icon), Style::default().fg(color)),
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
                .max_output_tokens
                .map(|t| format!(" ({}k ctx, {}k out)", model.context_window / 1000, t / 1000))
                .unwrap_or_else(|| format!(" ({}k ctx)", model.context_window / 1000));

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
    render_auth_hints(&provider.id, &provider.auth_type, &mut content);

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[t]est connection",
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

fn render_auth_hints(provider_id: &str, auth_type: &str, content: &mut Vec<Line>) {
    match auth_type {
        "aws_credentials" => {
            content.push(Line::from("Configure via AWS credential chain:"));
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "AWS_ACCESS_KEY_ID",
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("=\"...\"", Style::default().fg(Color::DarkGray)),
            ]));
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "AWS_SECRET_ACCESS_KEY",
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled("=\"...\"", Style::default().fg(Color::DarkGray)),
            ]));
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled("AWS_REGION", Style::default().fg(Color::Yellow)),
                Span::styled("=\"us-east-1\"", Style::default().fg(Color::DarkGray)),
            ]));
            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                "Also supports: IAM roles, ~/.aws/credentials",
                Style::default().fg(Color::Gray),
            )));
        }
        "oauth" => {
            content.push(Line::from("Uses Claude Code OAuth credentials:"));
            content.push(Line::from(vec![
                Span::styled("  1. Install: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    "npm i -g @anthropic-ai/claude-code",
                    Style::default().fg(Color::Yellow),
                ),
            ]));
            content.push(Line::from(vec![
                Span::styled("  2. Run: ", Style::default().fg(Color::Gray)),
                Span::styled("claude", Style::default().fg(Color::Yellow)),
                Span::styled(" (to authenticate)", Style::default().fg(Color::Gray)),
            ]));
        }
        "api_key" => {
            let env_var = match provider_id {
                "openai" => "OPENAI_API_KEY",
                "google" => "GOOGLE_API_KEY",
                _ => "API_KEY",
            };
            content.push(Line::from(vec![
                Span::styled("  export ", Style::default().fg(Color::DarkGray)),
                Span::styled(env_var, Style::default().fg(Color::Yellow)),
                Span::styled("=\"your-key\"", Style::default().fg(Color::DarkGray)),
            ]));
        }
        "none" => {
            content.push(Line::from(Span::styled(
                "No configuration needed.",
                Style::default().fg(Color::Green),
            )));
        }
        _ => {
            content.push(Line::from(Span::styled(
                "See provider documentation for setup.",
                Style::default().fg(Color::Gray),
            )));
        }
    }
}
