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
    // Use animated border when content pane is focused
    let border_style = if !state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title("Active Sessions");
    
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
            let channel_indicator = session.channel.as_ref()
                .map(|c| format!("[{}] ", c))
                .unwrap_or_default();
            
            ListItem::new(Line::from(vec![
                Span::styled(channel_indicator, Style::default().fg(Color::Cyan)),
                Span::raw(&session.agent_id),
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
    if !state.sessions.is_empty() {
        list_state.select(Some(state.selected_session));
    }
    
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_session_details(frame: &mut Frame, area: Rect, state: &AppState) {
    // Check if we're in chat mode
    let is_chat_mode = matches!(state.input_mode, InputMode::ChatInput);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .title(if is_chat_mode { "Chat" } else { "Session Details" });

    if is_chat_mode || !state.chat_messages.is_empty() {
        // Chat mode - show messages and input
        let inner = block.inner(area);
        frame.render_widget(block, area);
        
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);
        
        render_chat_messages(frame, chunks[0], state);
        render_chat_input(frame, chunks[1], state, is_chat_mode);
    } else if let Some(session) = state.sessions.get(state.selected_session) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(0)])
            .split(block.inner(area));
        
        frame.render_widget(block, area);
        
        // Session info
        let info = vec![
            Line::from(vec![
                Span::styled("Key: ", Style::default().fg(Color::Cyan)),
                Span::raw(&session.key),
            ]),
            Line::from(vec![
                Span::styled("Agent: ", Style::default().fg(Color::Cyan)),
                Span::raw(&session.agent_id),
            ]),
            Line::from(vec![
                Span::styled("Channel: ", Style::default().fg(Color::Cyan)),
                Span::raw(session.channel.as_deref().unwrap_or("-")),
            ]),
            Line::from(vec![
                Span::styled("Started: ", Style::default().fg(Color::Cyan)),
                Span::raw(session.started_at.as_deref().unwrap_or("-")),
            ]),
            Line::from(vec![
                Span::styled("Messages: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}", session.message_count)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "[c]hat  [k]ill  [v]iew history",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        
        let info_para = Paragraph::new(info);
        frame.render_widget(info_para, chunks[0]);
        
        // Chat preview area
        let chat_block = Block::default()
            .borders(Borders::ALL)
            .title("Recent Messages");
        
        let chat_content = Paragraph::new(vec![
            Line::from(Span::styled(
                "Press 'c' to open chat",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(chat_block)
        .alignment(Alignment::Center);
        
        frame.render_widget(chat_content, chunks[1]);
    } else if let Some(err) = &state.sessions_error {
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
        // No session selected - show chat interface with hint
        let inner = block.inner(area);
        frame.render_widget(block, area);
        
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(inner);
        
        if state.chat_messages.is_empty() {
            let content = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Press 'c' to start chatting",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Make sure you have an Anthropic API key",
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(
                    "configured in Credentials → Providers",
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            let paragraph = Paragraph::new(content).alignment(Alignment::Center);
            frame.render_widget(paragraph, chunks[0]);
        } else {
            render_chat_messages(frame, chunks[0], state);
        }
        render_chat_input(frame, chunks[1], state, is_chat_mode);
    }
}

fn render_chat_messages(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Messages ({})", state.chat_messages.len()));
    
    let inner = block.inner(area);
    frame.render_widget(block, area);
    
    if state.chat_messages.is_empty() {
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
    
    // Build message lines
    let mut lines: Vec<Line> = Vec::new();
    
    for msg in &state.chat_messages {
        let (prefix, style) = match msg.role {
            ChatRole::User => ("You: ", Style::default().fg(Color::Cyan)),
            ChatRole::Assistant => ("AI: ", Style::default().fg(Color::Green)),
            ChatRole::System => ("⚠ ", Style::default().fg(Color::Yellow)),
        };
        
        // Add timestamp if available
        let timestamp = msg.timestamp.as_ref()
            .map(|t| format!("[{}] ", t))
            .unwrap_or_default();
        
        // First line with prefix
        let first_line = Line::from(vec![
            Span::styled(timestamp, Style::default().fg(Color::DarkGray)),
            Span::styled(prefix.to_string(), style.add_modifier(Modifier::BOLD)),
        ]);
        lines.push(first_line);
        
        // Wrap message content
        for line in msg.content.lines() {
            lines.push(Line::from(Span::styled(format!("  {}", line), style)));
        }
        lines.push(Line::from("")); // Spacing between messages
    }
    
    // Show loading indicator
    if state.chat_loading {
        lines.push(Line::from(vec![
            Span::styled("AI: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("thinking...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        ]));
    }
    
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((state.chat_scroll as u16, 0));
    
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
        format!("{}_", &state.input_buffer) // Show cursor
    } else {
        state.input_buffer.clone()
    };
    
    let paragraph = Paragraph::new(input_text)
        .block(block)
        .style(Style::default().fg(Color::White));
    
    frame.render_widget(paragraph, area);
}
