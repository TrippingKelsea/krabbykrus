//! Agents management component - horizontal card strip + detail panel

use ratatui::{
    layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    Frame,
};

use super::render_spinner;
use crate::effects::{self, palette, EffectState};
use crate::state::{AgentStatus, AppState};

/// Card width for agent cards
const CARD_WIDTH: u16 = 16;

/// Render the agents page — cards in cards_area, details in detail_area
pub fn render_agents(
    frame: &mut Frame,
    cards_area: Rect,
    detail_area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    render_agent_cards(frame, cards_area, state, effect_state);
    render_agent_details(frame, detail_area, state);
}

fn render_agent_cards(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    if state.agents_loading {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(effects::inactive_border_style())
            .title("Agents");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading...", state.tick_count);
        return;
    }

    if state.agents.is_empty() {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(effects::inactive_border_style())
            .title("Agents");
        let hint = Paragraph::new(Line::from(Span::styled(
            " Press 'a' to create an agent ",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(hint, area);
        return;
    }

    let total = state.agents.len();
    let max_visible = (area.width / CARD_WIDTH) as usize;
    let max_visible = max_visible.max(1);

    let half = max_visible / 2;
    let start = if state.selected_agent <= half {
        0
    } else if state.selected_agent + half >= total {
        total.saturating_sub(max_visible)
    } else {
        state.selected_agent - half
    };
    let end = (start + max_visible).min(total);
    let visible_count = end - start;

    let mut constraints: Vec<Constraint> = (0..visible_count)
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Fill(1));

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .flex(Flex::Start)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    for (vi, idx) in (start..end).enumerate() {
        let agent = &state.agents[idx];
        let is_selected = idx == state.selected_agent;

        let border_style = if is_selected {
            effects::active_border_style(elapsed)
        } else {
            Style::default().fg(palette::INACTIVE_BORDER)
        };

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style);

        let inner = block.inner(card_chunks[vi]);
        frame.render_widget(block, card_chunks[vi]);

        if inner.height < 3 || inner.width < 3 {
            continue;
        }

        let max_w = inner.width as usize;

        // Line 1: status indicator + agent id
        let status_char = match agent.status {
            AgentStatus::Active => ("●", Color::Green),
            AgentStatus::Pending => ("◐", Color::Yellow),
            AgentStatus::Error => ("✗", Color::Red),
            AgentStatus::Disabled => ("○", Color::DarkGray),
        };
        let id_trunc: String = if agent.id.len() > max_w.saturating_sub(2) {
            agent.id[..max_w.saturating_sub(2)].to_string()
        } else {
            agent.id.clone()
        };

        // Line 2: model short
        let model_short: String = agent
            .model
            .as_ref()
            .map(|m| {
                let s = m.split('/').last().unwrap_or(m);
                if s.len() > max_w {
                    s[..max_w].to_string()
                } else {
                    s.to_string()
                }
            })
            .unwrap_or_else(|| "no model".to_string());

        // Line 3: session count
        let sessions_text = format!("{} sess", agent.session_count);

        let id_style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let model_style = if is_selected {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let lines = vec![
            Line::from(vec![
                Span::styled(status_char.0, Style::default().fg(status_char.1)),
                Span::styled(format!(" {id_trunc}"), id_style),
            ]),
            Line::from(Span::styled(model_short, model_style)),
            Line::from(Span::styled(
                sessions_text,
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
        let render_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.min(3),
        };
        frame.render_widget(paragraph, render_area);
    }

    // Fill remaining space with scroll hint
    if visible_count < card_chunks.len() {
        super::render_card_scroll_hint(frame, card_chunks[visible_count], start > 0, end < total);
    }
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
