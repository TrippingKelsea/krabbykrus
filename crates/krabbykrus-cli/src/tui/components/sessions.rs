//! Sessions component - view and interact with sessions

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;
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
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Session Details");

    if let Some(session) = state.sessions.get(state.selected_session) {
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
        let content = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Select a session",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(content)
            .block(block)
            .alignment(Alignment::Center);
        frame.render_widget(paragraph, area);
    }
}
