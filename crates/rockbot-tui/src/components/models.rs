//! Models/Providers component - horizontal card strip + detail panel
//!
//! Shows LLM provider configuration status dynamically loaded from the gateway.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    Frame,
};

use crate::effects::{self, palette, EffectState};
use crate::state::AppState;

/// Card width for provider cards
const CARD_WIDTH: u16 = 16;

/// Derive a 3-character provider code
fn provider_short_code(id: &str) -> &'static str {
    match id {
        "bedrock" => "BDR",
        "anthropic" => "ANT",
        "openai" => "OAI",
        "mock" => "MOK",
        _ => "UNK",
    }
}

/// Render the models page — cards in cards_area, details in detail_area
pub fn render_models(
    frame: &mut Frame,
    cards_area: Rect,
    detail_area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    if state.providers.is_empty() {
        render_no_providers(frame, detail_area);
        return;
    }

    render_provider_cards(frame, cards_area, state, effect_state);
    render_provider_details(frame, detail_area, state);
}

fn render_no_providers(frame: &mut Frame, area: Rect) {
    let body = super::render_detail_header(frame, area, "LLM Providers");

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
    ];

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, body);
}

fn render_provider_cards(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    let total = state.providers.len();
    let max_visible = (area.width / CARD_WIDTH) as usize;
    let max_visible = max_visible.max(1);

    let half = max_visible / 2;
    let start = if state.selected_provider <= half {
        0
    } else if state.selected_provider + half >= total {
        total.saturating_sub(max_visible)
    } else {
        state.selected_provider - half
    };
    let end = (start + max_visible).min(total);
    let visible_count = end - start;

    let mut constraints: Vec<Constraint> = (0..visible_count)
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Fill(1));

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .flex(Flex::Start)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    for (vi, idx) in (start..end).enumerate() {
        let provider = &state.providers[idx];
        let is_selected = idx == state.selected_provider;

        let border_style = if is_selected {
            effects::active_border_style(elapsed)
        } else {
            Style::default().fg(palette::INACTIVE_BORDER)
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style);

        let inner = block.inner(card_chunks[vi]);
        frame.render_widget(block, card_chunks[vi]);

        if inner.height < 3 || inner.width < 3 {
            continue;
        }

        let max_w = inner.width as usize;
        let code = provider_short_code(&provider.id);

        // Line 1: provider code + availability indicator
        let (indicator, ind_color) = if provider.available {
            ("●", palette::CONFIGURED)
        } else {
            ("○", palette::UNCONFIGURED)
        };

        // Line 2: provider name (truncated)
        let name: String = if provider.name.len() > max_w {
            provider.name[..max_w].to_string()
        } else {
            provider.name.clone()
        };

        // Line 3: model count
        let model_count = format!("{} models", provider.models.len());

        let name_style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let lines = vec![
            Line::from(vec![
                Span::styled(indicator, Style::default().fg(ind_color)),
                Span::styled(
                    format!(" {code}"),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(name, name_style)),
            Line::from(Span::styled(
                model_count,
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
        let render_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.min(3),
        };
        frame.render_widget(paragraph, render_area);
    }

    if visible_count < card_chunks.len() {
        super::render_card_scroll_hint(frame, card_chunks[visible_count], start > 0, end < total);
    }
}

fn render_provider_details(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Provider Details");

    let idx = state
        .selected_provider
        .min(state.providers.len().saturating_sub(1));
    let Some(provider) = state.providers.get(idx) else {
        let paragraph = Paragraph::new("No provider selected");
        frame.render_widget(paragraph, body);
        return;
    };

    let status_color = if provider.available {
        palette::CONFIGURED
    } else {
        palette::UNCONFIGURED
    };
    let status_text = if provider.available {
        "Available"
    } else {
        "Not Available"
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

        for model in provider.models.iter().take(12) {
            let tokens_info = model.max_output_tokens.map_or_else(
                || format!(" ({}k ctx)", model.context_window / 1000),
                |t| format!(" ({}k ctx, {}k out)", model.context_window / 1000, t / 1000),
            );

            content.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(Color::DarkGray)),
                Span::styled(&model.name, Style::default().fg(Color::White)),
                Span::styled(tokens_info, Style::default().fg(Color::DarkGray)),
            ]));
        }

        if provider.models.len() > 12 {
            content.push(Line::from(Span::styled(
                format!("  ... and {} more", provider.models.len() - 12),
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
    let schema = state
        .credential_schemas
        .iter()
        .find(|s| s.provider_id == provider.id);
    render_auth_hints(&provider.auth_type, &mut content, schema);

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[e]dit config  [t]est connection",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, body);
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
    auth_type: &str,
    content: &mut Vec<Line>,
    schema: Option<&crate::state::CredentialSchemaInfo>,
) {
    if let Some(schema) = schema {
        if let Some(method) = schema.auth_methods.first() {
            let env_fields: Vec<_> = method
                .fields
                .iter()
                .filter(|f| f.env_var.is_some())
                .collect();

            if !env_fields.is_empty() {
                for field in &env_fields {
                    if let Some(env_var) = &field.env_var {
                        let detected = std::env::var(env_var).is_ok();
                        let indicator = if detected { "✓" } else { " " };
                        let color = if detected {
                            Color::Green
                        } else {
                            Color::DarkGray
                        };
                        let value_hint = field.default.as_deref().unwrap_or("\"...\"");

                        content.push(Line::from(vec![
                            Span::styled(
                                format!("  {indicator} export "),
                                Style::default().fg(color),
                            ),
                            Span::styled(env_var.clone(), Style::default().fg(Color::Yellow)),
                            Span::styled(
                                format!("={value_hint}"),
                                Style::default().fg(Color::DarkGray),
                            ),
                        ]));
                    }
                }
            }

            if let Some(hint) = &method.hint {
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    hint.clone(),
                    Style::default().fg(Color::Gray),
                )));
            }

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

    // Fallback hints
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
