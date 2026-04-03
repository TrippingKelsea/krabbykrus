//! Agents management component - detail panel (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::effects::EffectState;
use crate::state::{AgentStatus, AppState};

/// Render the agents page — detail fills the full area (cards are in top slot bar)
pub fn render_agents(frame: &mut Frame, area: Rect, state: &AppState, _effect_state: &EffectState) {
    render_agent_details(frame, area, state);
}

fn render_agent_details(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Agent Details");

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

        let subagents: Vec<&str> = state
            .agents
            .iter()
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
            Span::raw(
                agent
                    .max_tool_calls
                    .map_or("-".to_string(), |n| n.to_string()),
            ),
        ]));

        content.push(Line::from(vec![
            Span::styled("Sessions: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", agent.session_count)),
        ]));

        if let Some(reason) = agent.reason.as_deref() {
            content.push(Line::from(vec![
                Span::styled("Pending Reason: ", Style::default().fg(Color::Cyan)),
                Span::styled(reason, Style::default().fg(Color::Yellow)),
            ]));
        }

        if let Some(ref prompt) = agent.system_prompt {
            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                "System Prompt:",
                Style::default().fg(Color::Cyan),
            )));
            for line in prompt.lines().take(6) {
                content.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(Color::Gray),
                )));
            }
            if prompt.lines().count() > 6 {
                content.push(Line::from(Span::styled(
                    "  ...",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }

        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "[a]dd  [e]dit  [f]iles  [d]isable  [r]eload",
            Style::default().fg(Color::DarkGray),
        )));

        let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, body);
    } else if let Some(err) = &state.agents_error {
        let content = Paragraph::new(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        ))
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
    } else {
        let content = Paragraph::new(Span::styled(
            "Select an agent or press 'a' to create one",
            Style::default().fg(Color::DarkGray),
        ))
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
    }
}
