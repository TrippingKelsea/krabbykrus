//! Agents management component

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::state::{AgentStatus, AppState};
use super::render_spinner;

/// Render the agents page
pub fn render_agents(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_agent_list(frame, chunks[0], state);
    render_agent_details(frame, chunks[1], state);
}

fn render_agent_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Configured Agents");
    
    if state.agents_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading...", state.tick_count);
        return;
    }
    
    let items: Vec<ListItem> = if state.agents.is_empty() {
        vec![
            ListItem::new(Span::styled(
                "No agents configured",
                Style::default().fg(Color::DarkGray),
            )),
            ListItem::new(Span::raw("")),
            ListItem::new(Span::styled(
                "Add agents in krabbykrus.toml",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        state.agents.iter().map(|agent| {
            let status_indicator = match agent.status {
                AgentStatus::Active => Span::styled("● ", Style::default().fg(Color::Green)),
                AgentStatus::Pending => Span::styled("◐ ", Style::default().fg(Color::Yellow)),
                AgentStatus::Error => Span::styled("✗ ", Style::default().fg(Color::Red)),
                AgentStatus::Disabled => Span::styled("○ ", Style::default().fg(Color::DarkGray)),
            };
            
            ListItem::new(Line::from(vec![
                status_indicator,
                Span::raw(&agent.id),
            ]))
        }).collect()
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    if !state.agents.is_empty() {
        list_state.select(Some(state.selected_agent));
    }
    
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_agent_details(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Agent Details");

    if let Some(agent) = state.agents.get(state.selected_agent) {
        let status_text = agent.status.label();
        let status_color = match agent.status {
            AgentStatus::Active => Color::Green,
            AgentStatus::Pending => Color::Yellow,
            AgentStatus::Error => Color::Red,
            AgentStatus::Disabled => Color::DarkGray,
        };
        
        let content = vec![
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                Span::raw(&agent.id),
            ]),
            Line::from(vec![
                Span::styled("Model: ", Style::default().fg(Color::Cyan)),
                Span::raw(agent.model.as_deref().unwrap_or("-")),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]),
            Line::from(vec![
                Span::styled("Sessions: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}", agent.session_count)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "[r]eload  [e]dit  [d]isable",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        
        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, area);
    } else if let Some(err) = &state.agents_error {
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("Error: {}", err),
                Style::default().fg(Color::Red),
            )),
        ];
        let paragraph = Paragraph::new(content)
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
    } else {
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Select an agent",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content)
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
    }
}
