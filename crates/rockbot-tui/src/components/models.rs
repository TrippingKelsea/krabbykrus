//! Models/Providers component - detail panel (card bar is in top slot bar)
//!
//! Shows LLM provider configuration status dynamically loaded from the gateway.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::effects::{palette, EffectState};
use crate::state::AppState;

/// Render the models page — detail fills the full area (cards are in top slot bar)
pub fn render_models(frame: &mut Frame, area: Rect, state: &AppState, _effect_state: &EffectState) {
    if state.providers.is_empty() {
        render_no_providers(frame, area);
        return;
    }

    render_provider_details(frame, area, state);
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
