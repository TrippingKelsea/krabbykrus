//! Agents management component

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::{AgentStatus, AppState};
use super::render_spinner;

/// Render the agents page
pub fn render_agents(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_agent_list(frame, chunks[0], state, effect_state);
    render_agent_details(frame, chunks[1], state);
}

fn render_agent_list(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    // Use animated border when content pane is focused
    let border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
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
                "Press [a] to create an agent",
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

            let prefix = if agent.parent_id.is_some() {
                Span::styled("  └ ", Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            };

            ListItem::new(Line::from(vec![
                status_indicator,
                prefix,
                Span::raw(&agent.id),
            ]))
        }).collect()
    };

    // Use active highlight only when content is focused
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
        
        let mut content = vec![
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
        ];

        if let Some(ref parent) = agent.parent_id {
            content.push(Line::from(vec![
                Span::styled("Parent: ", Style::default().fg(Color::Cyan)),
                Span::styled(parent.as_str(), Style::default().fg(Color::Yellow)),
                Span::styled(" (subagent)", Style::default().fg(Color::DarkGray)),
            ]));
        }

        // Show subagents of this agent
        let subagents: Vec<&str> = state.agents.iter()
            .filter(|a| a.parent_id.as_deref() == Some(&agent.id))
            .map(|a| a.id.as_str())
            .collect();
        if !subagents.is_empty() {
            content.push(Line::from(vec![
                Span::styled("Subagents: ", Style::default().fg(Color::Cyan)),
                Span::raw(subagents.join(", ")),
            ]));
        }

        if let Some(ref ws) = agent.workspace {
            content.push(Line::from(vec![
                Span::styled("Workspace: ", Style::default().fg(Color::Cyan)),
                Span::raw(ws.as_str()),
            ]));
        }

        content.push(Line::from(vec![
            Span::styled("Max Calls: ", Style::default().fg(Color::Cyan)),
            Span::raw(agent.max_tool_calls.map(|n| n.to_string()).unwrap_or("-".to_string())),
        ]));

        content.push(Line::from(vec![
            Span::styled("Sessions: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", agent.session_count)),
        ]));

        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "[a]dd  [e]dit  [d]isable  [r]eload",
            Style::default().fg(Color::DarkGray),
        )));
        
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
