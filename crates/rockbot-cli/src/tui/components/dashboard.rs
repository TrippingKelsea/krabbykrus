//! Dashboard component - horizontal card strip + detail panel

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::{AgentStatus, AppState};
use super::render_spinner;

const CARD_WIDTH: u16 = 16;
const CARD_HEIGHT: u16 = 5;

/// Render the dashboard page — card strip on top, detail below
pub fn render_dashboard(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(CARD_HEIGHT), Constraint::Min(0)])
        .split(area);

    render_status_cards(frame, chunks[0], state, effect_state);
    render_detail_panel(frame, chunks[1], state);
}

fn render_status_cards(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let cards = [
        ("Gateway", 0usize),
        ("Agents", 1),
        ("Sessions", 2),
        ("Vault", 3),
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
        let is_selected = idx == state.selected_dashboard_card;

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

        let lines = match idx {
            0 => gateway_card_lines(state),
            1 => agents_card_lines(state),
            2 => sessions_card_lines(state),
            3 => vault_card_lines(state),
            _ => vec![],
        };

        // Add label as first line
        let label_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let mut all_lines = vec![Line::from(Span::styled(label, label_style))];
        all_lines.extend(lines);

        let paragraph = Paragraph::new(all_lines).alignment(Alignment::Center);
        let render_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.min(3),
        };
        frame.render_widget(paragraph, render_area);
    }
}

fn gateway_card_lines(state: &AppState) -> Vec<Line<'static>> {
    if state.gateway_loading {
        return vec![Line::from(Span::styled("...", Style::default().fg(Color::DarkGray)))];
    }
    let (status, color) = if state.gateway.connected {
        ("● Online", Color::Green)
    } else {
        ("○ Offline", Color::Red)
    };
    vec![
        Line::from(Span::styled(status, Style::default().fg(color))),
        Line::from(Span::styled(
            state.gateway.version.clone().unwrap_or_else(|| "-".to_string()),
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

fn agents_card_lines(state: &AppState) -> Vec<Line<'static>> {
    if state.agents_loading {
        return vec![Line::from(Span::styled("...", Style::default().fg(Color::DarkGray)))];
    }
    let active = state.agents.iter().filter(|a| a.status == AgentStatus::Active).count();
    let pending = state.agents.iter().filter(|a| a.status == AgentStatus::Pending).count();
    let mut lines = vec![
        Line::from(Span::styled(format!("{active} active"), Style::default().fg(Color::Green))),
    ];
    if pending > 0 {
        lines.push(Line::from(Span::styled(format!("+{pending} pend"), Style::default().fg(Color::Yellow))));
    } else {
        lines.push(Line::from(Span::styled(format!("{} total", state.agents.len()), Style::default().fg(Color::DarkGray))));
    }
    lines
}

fn sessions_card_lines(state: &AppState) -> Vec<Line<'static>> {
    if state.sessions_loading {
        return vec![Line::from(Span::styled("...", Style::default().fg(Color::DarkGray)))];
    }
    let total_msgs: usize = state.sessions.iter().map(|s| s.message_count).sum();
    vec![
        Line::from(Span::styled(format!("{} active", state.sessions.len()), Style::default().fg(Color::Cyan))),
        Line::from(Span::styled(format!("{total_msgs} msgs"), Style::default().fg(Color::DarkGray))),
    ]
}

fn vault_card_lines(state: &AppState) -> Vec<Line<'static>> {
    if state.vault_loading {
        return vec![Line::from(Span::styled("...", Style::default().fg(Color::DarkGray)))];
    }
    if !state.vault.initialized {
        return vec![
            Line::from(Span::styled("Not Init", Style::default().fg(Color::Yellow))),
            Line::from(Span::styled("'i' to init", Style::default().fg(Color::DarkGray))),
        ];
    }
    let lock = if state.vault.locked { "Locked" } else { "Unlocked" };
    let lock_color = if state.vault.locked { Color::Yellow } else { Color::Green };
    vec![
        Line::from(Span::styled(lock, Style::default().fg(lock_color))),
        Line::from(Span::styled(format!("{} endpts", state.vault.endpoint_count), Style::default().fg(Color::DarkGray))),
    ]
}

/// Render the detail panel based on which dashboard card is selected
fn render_detail_panel(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.selected_dashboard_card {
        0 => render_gateway_detail(frame, area, state),
        1 => render_agents_detail(frame, area, state),
        2 => render_sessions_detail(frame, area, state),
        3 => render_vault_detail(frame, area, state),
        _ => {}
    }
}

fn render_gateway_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Gateway");

    if state.gateway_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Checking gateway...", state.tick_count);
        return;
    }

    let (status, color) = if state.gateway.connected {
        ("● Running", Color::Green)
    } else {
        ("○ Stopped", Color::Red)
    };

    let mut content = vec![
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(status, Style::default().fg(color)),
        ]),
        Line::from(vec![
            Span::styled("Version: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.gateway.version.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Endpoint: ", Style::default().fg(Color::Cyan)),
            Span::raw("http://127.0.0.1:18080"),
        ]),
    ];

    if let Some(ref err) = state.gateway_error {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(format!("Error: {err}"), Style::default().fg(Color::Red))));
    }

    // Show provider summary
    if !state.providers.is_empty() {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled("Providers:", Style::default().fg(Color::Cyan))));
        for p in &state.providers {
            let (ind, ind_color) = if p.available { ("●", Color::Green) } else { ("○", Color::Yellow) };
            content.push(Line::from(vec![
                Span::styled(format!("  {ind} "), Style::default().fg(ind_color)),
                Span::raw(&p.name),
                Span::styled(format!(" ({} models)", p.models.len()), Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[s]tart  [S]top  [r]estart",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(content)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_agents_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Agents Overview");

    if state.agents_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading agents...", state.tick_count);
        return;
    }

    if state.agents.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("No agents configured", Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled("Go to Agents tab (3) to add agents", Style::default().fg(Color::DarkGray))),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, area);
        return;
    }

    let header = Row::new(vec!["Agent ID", "Model", "Sessions", "Status"])
        .style(Style::default().fg(Color::Cyan))
        .bottom_margin(1);

    let rows: Vec<Row> = state.agents.iter().map(|agent| {
        let status_style = match agent.status {
            AgentStatus::Active => Style::default().fg(Color::Green),
            AgentStatus::Pending => Style::default().fg(Color::Yellow),
            AgentStatus::Error => Style::default().fg(Color::Red),
            AgentStatus::Disabled => Style::default().fg(Color::DarkGray),
        };

        Row::new(vec![
            agent.id.clone(),
            agent.model.clone().unwrap_or_else(|| "-".to_string()),
            format!("{}", agent.session_count),
            agent.status.label().to_string(),
        ])
        .style(status_style)
    }).collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block);

    frame.render_widget(table, area);
}

fn render_sessions_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Sessions Overview");

    if state.sessions_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading sessions...", state.tick_count);
        return;
    }

    if state.sessions.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("No active sessions", Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled("Go to Sessions tab (4) to create one", Style::default().fg(Color::DarkGray))),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, area);
        return;
    }

    let mut content = vec![
        Line::from(vec![
            Span::styled("Active: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.sessions.len())),
        ]),
        Line::from(vec![
            Span::styled("Total Messages: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.sessions.iter().map(|s| s.message_count).sum::<usize>())),
        ]),
        Line::from(""),
    ];

    for session in state.sessions.iter().take(10) {
        let model_hint = session.model.as_ref()
            .and_then(|m| m.split('/').last())
            .unwrap_or("-");
        content.push(Line::from(vec![
            Span::styled(&session.agent_id, Style::default().fg(Color::White)),
            Span::styled(format!("  {model_hint}"), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("  ({} msgs)", session.message_count), Style::default().fg(Color::Cyan)),
        ]));
    }

    if state.sessions.len() > 10 {
        content.push(Line::from(Span::styled(
            format!("  ... and {} more", state.sessions.len() - 10),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}

fn render_vault_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Vault");

    if state.vault_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Checking vault...", state.tick_count);
        return;
    }

    let mut content = vec![];

    if !state.vault.initialized {
        content.push(Line::from(Span::styled("Vault not initialized", Style::default().fg(Color::Yellow))));
        content.push(Line::from(""));
        content.push(Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]));
        content.push(Line::from(""));
        content.push(Line::from(Span::styled("Press 'i' to initialize", Style::default().fg(Color::Green))));
    } else {
        let (lock_text, lock_color) = if state.vault.locked {
            ("Locked", Color::Yellow)
        } else {
            ("Unlocked", Color::Green)
        };

        content.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(lock_text, Style::default().fg(lock_color)),
        ]));
        content.push(Line::from(vec![
            Span::styled("Endpoints: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.vault.endpoint_count)),
        ]));
        content.push(Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]));
        content.push(Line::from(""));

        if state.vault.locked {
            content.push(Line::from(Span::styled("Press 'u' to unlock", Style::default().fg(Color::Green))));
        } else {
            content.push(Line::from(Span::styled("Press 'l' to lock", Style::default().fg(Color::DarkGray))));
        }
    }

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}
