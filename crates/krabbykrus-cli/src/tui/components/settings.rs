//! Settings component

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::effects::{self, EffectState};
use crate::tui::state::AppState;

/// Render the settings page
pub fn render_settings(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(10), // General
            Constraint::Length(8),  // Paths
            Constraint::Min(0),     // About
        ])
        .split(area);

    render_general(frame, chunks[0], state, effect_state);
    render_paths(frame, chunks[1], state);
    render_about(frame, chunks[2]);
}

fn render_general(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    // Use animated border when content pane is focused
    let border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
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
            Span::styled("Version: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.gateway.version.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Active Sessions: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.sessions.len())),
        ]),
        Line::from(vec![
            Span::styled("Configured Agents: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.agents.len())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "[s]tart/[S]top gateway  [r]estart",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("General");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_paths(frame: &mut Frame, area: Rect, state: &AppState) {
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
            "[o]pen config  [e]dit",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Paths");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_about(frame: &mut Frame, area: Rect) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "🦀 Krabbykrus",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("A Rust-native AI agent framework"),
        Line::from(""),
        Line::from(Span::styled("https://github.com/TrippingKelsea/krabbykrus", Style::default().fg(Color::Blue))),
        Line::from(""),
        Line::from(Span::styled(
            "Press ? for keyboard shortcuts",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title("About");
    
    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}
