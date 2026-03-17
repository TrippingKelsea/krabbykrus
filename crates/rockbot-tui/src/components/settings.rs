//! Settings component - detail panel (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::effects::{palette, EffectState};
use crate::state::AppState;
use rockbot_core::{AnimationStyle, ColorTheme};

/// Render the settings page — detail fills the full area (cards are in top slot bar)
pub fn render_settings(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    render_settings_detail(frame, area, state);
}

pub(crate) fn render_settings_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.selected_settings_card {
        0 => render_general(frame, area, state),
        1 => render_paths(frame, area, state),
        2 => render_about(frame, area),
        3 => render_theme(frame, area, state),
        _ => {}
    }
}

pub(crate) fn render_general(frame: &mut Frame, area: Rect, state: &AppState) {
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

pub(crate) fn render_paths(frame: &mut Frame, area: Rect, state: &AppState) {
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

pub(crate) fn render_about(frame: &mut Frame, area: Rect) {
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

pub(crate) fn render_theme(frame: &mut Frame, area: Rect, state: &AppState) {
    use ratatui::layout::{Constraint, Direction, Layout};

    let body = super::render_detail_header(frame, area, "Theme");

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Color Theme label
            Constraint::Length(1), // Color Theme picker
            Constraint::Length(1), // spacer
            Constraint::Length(1), // Animation Style label
            Constraint::Length(1), // Animation Style picker
            Constraint::Length(1), // spacer
            Constraint::Length(1), // hint
            Constraint::Fill(1),   // fill
        ])
        .split(body);

    // Color Theme label
    let label_style = if state.selected_settings_field == 0 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled("Color Theme", label_style)),
        rows[0],
    );

    // Color Theme picker
    let theme_spans: Vec<Span<'_>> = ColorTheme::all()
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let color = palette::theme_primary(t);
            let style = if *t == state.tui_config.color_theme {
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(color)
            };
            let mut spans = vec![Span::styled(format!(" {} ", t.label()), style)];
            if i < ColorTheme::all().len() - 1 {
                spans.push(Span::raw(" "));
            }
            spans
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(theme_spans)), rows[1]);

    // Animation Style label
    let label_style = if state.selected_settings_field == 1 {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_widget(
        Paragraph::new(Span::styled("Animation Style", label_style)),
        rows[3],
    );

    // Animation Style picker
    let anim_spans: Vec<Span<'_>> = AnimationStyle::all()
        .iter()
        .enumerate()
        .flat_map(|(i, a)| {
            let style = if *a == state.tui_config.animation_style {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans = vec![Span::styled(format!(" {} ", a.label()), style)];
            if i < AnimationStyle::all().len() - 1 {
                spans.push(Span::raw(" "));
            }
            spans
        })
        .collect();
    frame.render_widget(Paragraph::new(Line::from(anim_spans)), rows[4]);

    // Hint
    frame.render_widget(
        Paragraph::new(Span::styled(
            "↑↓:Field  [:Prev  ]:Next",
            Style::default().fg(Color::DarkGray),
        )),
        rows[6],
    );
}
