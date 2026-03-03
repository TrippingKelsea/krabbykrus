//! Dashboard component - main overview

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame,
};

use crate::tui::state::{AgentStatus, AppState};
use super::render_spinner;

/// Render the dashboard page
pub fn render_dashboard(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(7),  // Status cards
            Constraint::Min(0),     // Agents table
        ])
        .split(area);

    render_header(frame, chunks[0], state);
    render_status_cards(frame, chunks[1], state);
    render_agents_table(frame, chunks[2], state);
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let status_indicator = if state.gateway.connected {
        Span::styled(" ● Online ", Style::default().fg(Color::Green))
    } else {
        Span::styled(" ○ Offline ", Style::default().fg(Color::Red))
    };
    
    let title = Line::from(vec![
        Span::styled("🚀 Krabbykrus Dashboard", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        status_indicator,
    ]);
    
    let header = Paragraph::new(title)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
    
    frame.render_widget(header, area);
}

fn render_status_cards(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    // Gateway card
    let gateway_content = if state.gateway_loading {
        vec![
            Line::from(""),
            Line::from(Span::styled("Loading...", Style::default().fg(Color::DarkGray))),
        ]
    } else if let Some(err) = &state.gateway_error {
        vec![
            Line::from(""),
            Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))),
        ]
    } else {
        let (status_text, status_color) = if state.gateway.connected {
            ("● Running", Color::Green)
        } else {
            ("○ Stopped", Color::Red)
        };
        vec![
            Line::from(Span::styled(status_text, Style::default().fg(status_color))),
            Line::from(""),
            Line::from(Span::styled(
                state.gateway.version.as_deref().unwrap_or("-"),
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };
    
    let gateway_card = Paragraph::new(gateway_content)
        .block(Block::default().borders(Borders::ALL).title("Gateway"))
        .alignment(Alignment::Center);
    frame.render_widget(gateway_card, chunks[0]);

    // Agents card
    let active_count = state.agents.iter().filter(|a| a.status == AgentStatus::Active).count();
    let pending_count = state.agents.iter().filter(|a| a.status == AgentStatus::Pending).count();
    
    let agents_content = if state.agents_loading {
        vec![
            Line::from(""),
            Line::from(Span::styled("Loading...", Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("{}", active_count),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled("active", Style::default().fg(Color::DarkGray))),
            if pending_count > 0 {
                Line::from(Span::styled(
                    format!("+{} pending", pending_count),
                    Style::default().fg(Color::Yellow),
                ))
            } else {
                Line::from("")
            },
        ]
    };
    
    let agents_card = Paragraph::new(agents_content)
        .block(Block::default().borders(Borders::ALL).title("Agents"))
        .alignment(Alignment::Center);
    frame.render_widget(agents_card, chunks[1]);

    // Sessions card
    let sessions_content = if state.sessions_loading {
        vec![
            Line::from(""),
            Line::from(Span::styled("Loading...", Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                format!("{}", state.sessions.len()),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled("active", Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled(
                format!("{} total msgs", state.sessions.iter().map(|s| s.message_count).sum::<usize>()),
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };
    
    let sessions_card = Paragraph::new(sessions_content)
        .block(Block::default().borders(Borders::ALL).title("Sessions"))
        .alignment(Alignment::Center);
    frame.render_widget(sessions_card, chunks[2]);

    // Vault card
    let vault_content = if state.vault_loading {
        vec![
            Line::from(""),
            Line::from(Span::styled("Loading...", Style::default().fg(Color::DarkGray))),
        ]
    } else if !state.vault.initialized {
        vec![
            Line::from(Span::styled("Not Init", Style::default().fg(Color::Yellow))),
            Line::from(""),
            Line::from(Span::styled("Press 'c' to configure", Style::default().fg(Color::DarkGray))),
        ]
    } else {
        let lock_status = if state.vault.locked { "🔒 Locked" } else { "🔓 Unlocked" };
        vec![
            Line::from(lock_status),
            Line::from(""),
            Line::from(Span::styled(
                format!("{} endpoints", state.vault.endpoint_count),
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };
    
    let vault_card = Paragraph::new(vault_content)
        .block(Block::default().borders(Borders::ALL).title("Vault"))
        .alignment(Alignment::Center);
    frame.render_widget(vault_card, chunks[3]);
}

fn render_agents_table(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agents");
    
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
            Line::from(Span::styled("Add agents in krabbykrus.toml", Style::default().fg(Color::DarkGray))),
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
        .block(block)
        .row_highlight_style(Style::default().bg(Color::DarkGray));
    
    frame.render_widget(table, area);
}
