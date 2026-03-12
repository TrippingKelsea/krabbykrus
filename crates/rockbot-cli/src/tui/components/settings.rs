//! Settings component - horizontal card strip + detail panel

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;

const CARD_WIDTH: u16 = 16;
const CARD_HEIGHT: u16 = 5;

/// Render the settings page — card strip on top, detail below
pub fn render_settings(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(CARD_HEIGHT), Constraint::Min(0)])
        .split(area);

    render_settings_cards(frame, chunks[0], state, effect_state);
    render_settings_detail(frame, chunks[1], state);
}

fn render_settings_cards(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let cards = [
        ("General", 0usize),
        ("Paths", 1),
        ("About", 2),
    ];

    let mut constraints: Vec<Constraint> = cards.iter()
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Min(0));

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    for &(label, idx) in &cards {
        let is_selected = idx == state.selected_settings_card;

        let border_style = if is_selected {
            effects::active_border_style(elapsed)
        } else {
            Style::default().fg(palette::INACTIVE_BORDER)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(card_chunks[idx]);
        frame.render_widget(block, card_chunks[idx]);

        if inner.height < 3 || inner.width < 2 {
            continue;
        }

        let label_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let lines = match idx {
            0 => vec![
                Line::from(Span::styled(label, label_style)),
                Line::from(Span::styled("Gateway", Style::default().fg(Color::Cyan))),
                Line::from(Span::styled("Controls", Style::default().fg(Color::DarkGray))),
            ],
            1 => vec![
                Line::from(Span::styled(label, label_style)),
                Line::from(Span::styled("Config", Style::default().fg(Color::Cyan))),
                Line::from(Span::styled("Locations", Style::default().fg(Color::DarkGray))),
            ],
            2 => vec![
                Line::from(Span::styled(label, label_style)),
                Line::from(Span::styled("RockBot", Style::default().fg(Color::Cyan))),
                Line::from(Span::styled("Info", Style::default().fg(Color::DarkGray))),
            ],
            _ => vec![],
        };

        let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
        let render_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.min(3),
        };
        frame.render_widget(paragraph, render_area);
    }
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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("General");

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

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_paths(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Paths");

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

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_about(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("About");

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "RockBot",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from("A Rust-native AI agent framework"),
        Line::from(""),
        Line::from("Self-hosted multi-channel AI gateway with"),
        Line::from("pluggable LLM providers, credential management,"),
        Line::from("and a TUI + Web interface."),
        Line::from(""),
        Line::from(Span::styled("https://github.com/TrippingKelsea/rockbot", Style::default().fg(Color::Blue))),
    ];

    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}
