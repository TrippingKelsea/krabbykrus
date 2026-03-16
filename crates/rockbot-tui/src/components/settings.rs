//! Settings component - detail panel (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::effects::EffectState;
use crate::state::AppState;

/// Render the settings page — detail fills the full area (cards are in top slot bar)
pub fn render_settings(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    render_settings_detail(frame, area, state);
}

fn render_settings_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.selected_settings_card {
        0 => render_general(frame, area, state),
        1 => render_paths(frame, area, state),
        2 => render_about(frame, area),
        _ => {}
    }
}

fn render_general(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "General");

    let gateway_status = if state.gateway.connected {
        Span::styled("● Running", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ Stopped", Style::default().fg(Color::Red))
    };

    let content = vec![
        Line::from(vec![
            Span::styled("Gateway Status: ", Style::default().fg(Color::Cyan)),
            gateway_status,
        ]),
        Line::from(vec![
            Span::styled("Gateway Version: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.gateway.version.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Client Version: ", Style::default().fg(Color::Cyan)),
            Span::raw(env!("CARGO_PKG_VERSION")),
        ]),
        Line::from(vec![
            Span::styled("Active Sessions: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.sessions.len())),
        ]),
        Line::from(vec![
            Span::styled("Configured Agents: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.agents.len())),
        ]),
        Line::from(vec![
            Span::styled("LLM Providers: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.providers.len())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "[s]tart/[S]top gateway  [r]estart",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, body);
}

fn render_paths(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Paths");

    let content = vec![
        Line::from(vec![
            Span::styled("Config: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.config_path.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled("Vault: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Agent directories:",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(Span::styled(
            "  ~/.config/rockbot/agents/{agent_id}/",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  Each agent has SOUL.md and SYSTEM-PROMPT.md",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, body);
}

fn render_about(frame: &mut Frame, area: Rect) {
    let body = super::render_detail_header(frame, area, "About");

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "RockBot",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("A Rust-native AI agent framework"),
        Line::from(""),
        Line::from("Self-hosted multi-channel AI gateway with"),
        Line::from("pluggable LLM providers, credential management,"),
        Line::from("and a TUI + Web interface."),
        Line::from(""),
        Line::from(Span::styled(
            "https://github.com/TrippingKelsea/rockbot",
            Style::default().fg(Color::Blue),
        )),
    ];

    let paragraph = Paragraph::new(content).alignment(Alignment::Center);
    frame.render_widget(paragraph, body);
}
