//! Sessions component - view and interact with sessions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::{AppState, ChatRole, InputMode};
use super::render_spinner;

/// Render the sessions page
pub fn render_sessions(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    render_session_list(frame, chunks[0], state, effect_state);
    render_session_details(frame, chunks[1], state);
}

fn render_session_list(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("Sessions");

    if state.sessions_loading {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading...", state.tick_count);
        return;
    }

    let items: Vec<ListItem> = if state.sessions.is_empty() {
        vec![
            ListItem::new(Span::styled(
                "No active sessions",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        state.sessions.iter().map(|session| {
            let model_hint = session.model.as_ref()
                .and_then(|m| m.split('/').last())
                .map(|m| {
                    // Shorten model IDs for display
                    let short = if m.len() > 25 { &m[..25] } else { m };
                    format!(" [{short}]")
                })
                .unwrap_or_default();

            let msg_count = state.session_chats
                .get(&session.key)
                .map_or(session.message_count, |c| c.messages.len());

            ListItem::new(Line::from(vec![
                Span::styled(&session.agent_id, Style::default().fg(Color::White)),
                Span::styled(model_hint, Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!(" ({msg_count})"),
                    Style::default().fg(Color::Cyan),
                ),
            ]))
        }).collect()
    };

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
    if !state.sessions.is_empty() {
        list_state.select(Some(state.selected_session));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_session_details(frame: &mut Frame, area: Rect, state: &AppState) {
    let is_chat_mode = matches!(state.input_mode, InputMode::ChatInput);

    // Build title from selected session
    let chat_title = if let Some(session) = state.sessions.get(state.selected_session) {
        let model_part = session.model.as_ref()
            .and_then(|m| m.split('/').last())
            .unwrap_or("default");
        format!("{} — {model_part}", session.agent_id)
    } else {
        "Chat".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(chat_title);

    let messages = state.chat_messages();
    let chat_loading = state.chat_loading();
    let chat_scroll = state.chat_scroll();

    if !messages.is_empty() || is_chat_mode || chat_loading {
        // Chat view — messages + input
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);

        render_chat_messages(frame, chunks[0], messages, chat_loading, chat_scroll);
        render_chat_input(frame, chunks[1], state, is_chat_mode);
    } else if state.sessions.is_empty() {
        // No sessions at all
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);

        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Press 'n' to create a new session",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Sessions let you pick a model (ad-hoc) or agent",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "and keep conversation history",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content).alignment(Alignment::Center);
        frame.render_widget(paragraph, chunks[0]);
        render_chat_input(frame, chunks[1], state, false);
    } else if let Some(err) = &state.sessions_error {
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("Error: {err}"),
                Style::default().fg(Color::Red),
            )),
        ];
        let paragraph = Paragraph::new(content)
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
    } else {
        // Session selected but no messages yet — show empty chat with hint
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);

        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "No messages yet. Press 'c' to start chatting.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content).alignment(Alignment::Center);
        frame.render_widget(paragraph, chunks[0]);
        render_chat_input(frame, chunks[1], state, false);
    }
}

fn render_chat_messages(
    frame: &mut Frame,
    area: Rect,
    messages: &[crate::tui::state::ChatMessage],
    loading: bool,
    scroll: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Messages ({})", messages.len()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if messages.is_empty() && !loading {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No messages yet. Type a message and press Enter.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(empty, inner);
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    for msg in messages {
        let (prefix, style) = match msg.role {
            ChatRole::User => ("You: ", Style::default().fg(Color::Cyan)),
            ChatRole::Assistant => ("AI: ", Style::default().fg(Color::Green)),
            ChatRole::System => ("sys: ", Style::default().fg(Color::Yellow)),
        };

        let timestamp = msg.timestamp.as_ref()
            .map(|t| format!("[{t}] "))
            .unwrap_or_default();

        let first_line = Line::from(vec![
            Span::styled(timestamp, Style::default().fg(Color::DarkGray)),
            Span::styled(prefix.to_string(), style.add_modifier(Modifier::BOLD)),
        ]);
        lines.push(first_line);

        for line in msg.content.lines() {
            lines.push(Line::from(Span::styled(format!("  {line}"), style)));
        }
        lines.push(Line::from(""));
    }

    if loading {
        lines.push(Line::from(vec![
            Span::styled("AI: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("thinking...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    frame.render_widget(paragraph, inner);
}

fn render_chat_input(frame: &mut Frame, area: Rect, state: &AppState, is_active: bool) {
    let border_style = if is_active {
        Style::default().fg(palette::ACTIVE_PRIMARY)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_active {
        "Type message (Enter to send, Esc to cancel)"
    } else {
        "Press 'c' to chat"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let input_text = if is_active {
        format!("{}_", &state.input_buffer)
    } else {
        state.input_buffer.clone()
    };

    let paragraph = Paragraph::new(input_text)
        .block(block)
        .style(Style::default().fg(Color::White));

    frame.render_widget(paragraph, area);
}
