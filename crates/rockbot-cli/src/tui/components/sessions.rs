//! Sessions component - horizontal card strip + full-width chat

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::{AppState, ChatRole, InputMode};
use super::render_spinner;

/// Card width: 2 border + 13 content = 15 columns
const CARD_WIDTH: u16 = 15;
/// Card height: 2 border + 3 content lines = 5 rows
const CARD_HEIGHT: u16 = 5;

/// Derive a 3-character provider code from provider ID
fn provider_short_code(provider_id: &str) -> &'static str {
    match provider_id {
        "bedrock" => "BDR",
        "anthropic" => "ANT",
        "openai" => "OAI",
        "mock" => "MOK",
        _ => "UNK",
    }
}

/// Extract provider ID and model short name from a full model string like "bedrock/anthropic.claude-sonnet-4-20250514-v1:0"
fn format_model_short(model: &str) -> String {
    let (provider_part, model_part) = model.split_once('/').unwrap_or(("", model));
    let code = provider_short_code(provider_part);
    // Shorten model name: take last segment after '.', then truncate
    let short_model = model_part
        .rsplit('.')
        .next()
        .unwrap_or(model_part);
    // Further shorten: strip common prefixes/suffixes, truncate to 8 chars
    let short = shorten_model_name(short_model);
    format!("{code}:{short}")
}

/// Shorten a model name for card display
fn shorten_model_name(name: &str) -> String {
    // Remove version suffixes like "-v1:0", "-20250514"
    let s = name.split("-v1").next().unwrap_or(name);
    // Remove date stamps (8+ digit sequences)
    let parts: Vec<&str> = s.split('-').filter(|p| {
        !(p.len() >= 8 && p.chars().all(|c| c.is_ascii_digit()))
    }).collect();
    let joined = parts.join("-");
    if joined.len() > 9 {
        joined[..9].to_string()
    } else {
        joined
    }
}

/// Render the sessions page — card strip on top, chat below
pub fn render_sessions(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(CARD_HEIGHT), Constraint::Min(0)])
        .split(area);

    render_session_cards(frame, chunks[0], state, effect_state);
    render_chat_area(frame, chunks[1], state, effect_state);
}

/// Render the horizontal session card strip
fn render_session_cards(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    if state.sessions_loading {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(effects::inactive_border_style())
            .title("Sessions");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        render_spinner(frame, inner, "Loading...", state.tick_count);
        return;
    }

    if state.sessions.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(effects::inactive_border_style())
            .title("Sessions");
        let hint = Paragraph::new(Line::from(Span::styled(
            " Press 'n' to create a new session ",
            Style::default().fg(Color::DarkGray),
        )))
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(hint, area);
        return;
    }

    // Calculate visible card range based on selected session
    let total = state.sessions.len();
    let max_visible = (area.width / CARD_WIDTH) as usize;
    let max_visible = max_visible.max(1);

    // Center the selected session in the visible range
    let half = max_visible / 2;
    let start = if state.selected_session <= half {
        0
    } else if state.selected_session + half >= total {
        total.saturating_sub(max_visible)
    } else {
        state.selected_session - half
    };
    let end = (start + max_visible).min(total);
    let visible_count = end - start;

    // Build constraints for visible cards + optional spacer
    let mut constraints: Vec<Constraint> = (0..visible_count)
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Min(0)); // fill remaining space

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    for (vi, idx) in (start..end).enumerate() {
        let session = &state.sessions[idx];
        let is_selected = idx == state.selected_session;

        render_session_card(frame, card_chunks[vi], session, state, is_selected, elapsed);
    }

    // Fill remaining space with empty block
    if visible_count < card_chunks.len() {
        let filler = Block::default()
            .borders(Borders::NONE);
        frame.render_widget(filler, card_chunks[visible_count]);
    }
}

/// Render a single session card
fn render_session_card(
    frame: &mut Frame,
    area: Rect,
    session: &crate::tui::state::SessionInfo,
    state: &AppState,
    is_selected: bool,
    elapsed: f64,
) {
    let border_style = if is_selected {
        effects::active_border_style(elapsed)
    } else {
        Style::default().fg(palette::INACTIVE_BORDER)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 3 {
        return;
    }

    // Line 1: last 6 chars of session key
    let id_short = if session.key.len() > 6 {
        &session.key[session.key.len() - 6..]
    } else {
        &session.key
    };

    // Line 2: agent name truncated to inner width
    let max_name = inner.width as usize;
    let agent_display = if session.agent_id.starts_with("ad-hoc") {
        "ad-hoc"
    } else {
        &session.agent_id
    };
    let agent_short: String = if agent_display.len() > max_name {
        agent_display[..max_name].to_string()
    } else {
        agent_display.to_string()
    };

    // Line 3: provider:model short code
    let model_line = session.model.as_ref()
        .map_or_else(|| "no model".to_string(), |m| format_model_short(m));
    let model_display: String = if model_line.len() > max_name {
        model_line[..max_name].to_string()
    } else {
        model_line
    };

    // Message count badge
    let msg_count = state.session_chats
        .get(&session.key)
        .map_or(session.message_count, |c| c.messages.len());

    let id_style = if is_selected {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let agent_style = if is_selected {
        Style::default().fg(palette::ACTIVE_SECONDARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let model_style = if is_selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Render each line, adding msg count to ID line if space allows
    let id_text = if msg_count > 0 {
        let badge = format!("{id_short} {msg_count}");
        if badge.len() <= max_name { badge } else { id_short.to_string() }
    } else {
        id_short.to_string()
    };

    let lines = vec![
        Line::from(Span::styled(id_text, id_style)),
        Line::from(Span::styled(agent_short, agent_style)),
        Line::from(Span::styled(model_display, model_style)),
    ];

    let paragraph = Paragraph::new(lines).alignment(Alignment::Center);

    // Only render 3 lines
    let render_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.min(3),
    };
    frame.render_widget(paragraph, render_area);
}

/// Render the chat area (messages + input) — takes full width
fn render_chat_area(frame: &mut Frame, area: Rect, state: &AppState, _effect_state: &EffectState) {
    let is_chat_mode = matches!(state.input_mode, InputMode::ChatInput);

    let messages = state.chat_messages();
    let chat_loading = state.chat_loading();
    let chat_scroll = state.chat_scroll();
    let auto_scroll = state.chat_auto_scroll();

    // Calculate input height accounting for both explicit newlines and visual line wrapping
    let inner_width = area.width.saturating_sub(2).max(1) as usize; // subtract borders
    let visual_lines: usize = state.input_buffer.split('\n').map(|line| {
        let char_count = line.len().max(1); // at least 1 line per segment
        (char_count + inner_width - 1) / inner_width // ceiling division
    }).sum();
    let input_line_count = visual_lines.clamp(1, 10);
    let input_height = (input_line_count as u16) + 2; // +2 for borders

    // Split into messages area + input bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(input_height)])
        .split(area);

    if !messages.is_empty() || chat_loading {
        render_chat_messages(frame, chunks[0], state, messages, chat_loading, chat_scroll, auto_scroll);
    } else if state.sessions.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(effects::inactive_border_style());
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Press 'n' to create a new session",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    } else if let Some(err) = &state.sessions_error {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::ERROR));
        let content = Paragraph::new(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        )))
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    } else {
        // Session selected but no messages
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(effects::inactive_border_style());
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No messages yet. Press 'c' to start chatting.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    }

    render_chat_input(frame, chunks[1], state, is_chat_mode);
}

fn render_chat_messages(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    messages: &[crate::tui::state::ChatMessage],
    loading: bool,
    scroll: usize,
    auto_scroll: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER));

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

        // Render tool calls before the final content (like Claude Code)
        for tc in &msg.tool_calls {
            let status_icon = if tc.success { "+" } else { "x" };
            let status_color = if tc.success { Color::Green } else { Color::Red };
            let duration = if tc.duration_ms > 0 {
                format!(" ({:.1}s)", tc.duration_ms as f64 / 1000.0)
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("[{status_icon}]"),
                    Style::default().fg(status_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", tc.tool_name),
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                ),
                Span::styled(duration, Style::default().fg(Color::DarkGray)),
            ]));
            if !tc.result.is_empty() {
                for result_line in tc.result.lines().take(4) {
                    lines.push(Line::from(Span::styled(
                        format!("    {result_line}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                let line_count = tc.result.lines().count();
                if line_count > 4 {
                    lines.push(Line::from(Span::styled(
                        format!("    ... ({} more lines)", line_count - 4),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                    )));
                }
            }
        }

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

    // Compute actual wrapped line count by rendering to a scratch buffer.
    // Paragraph::line_count gives the exact post-wrap count so our scroll
    // offset matches what ratatui will render.
    let view_width = inner.width.max(1);
    let view_height = inner.height as usize;

    let scratch = Paragraph::new(lines.clone())
        .wrap(Wrap { trim: false });
    let total_visual_lines = scratch.line_count(view_width) as usize;

    let max_scroll = total_visual_lines.saturating_sub(view_height);

    // Store max_scroll for key handler to use when transitioning from auto_scroll
    if let Some(chat) = state.active_chat() {
        chat.max_scroll.set(max_scroll);
    }

    let effective_scroll = if auto_scroll {
        max_scroll
    } else {
        scroll.min(max_scroll)
    };

    let not_at_bottom = total_visual_lines > view_height && effective_scroll < max_scroll;

    let scroll_indicator = if not_at_bottom {
        " ↑↓:Scroll  End:Bottom"
    } else {
        ""
    };

    // Re-render block with scroll indicator if needed
    if !scroll_indicator.is_empty() {
        let block_with_hint = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::INACTIVE_BORDER))
            .title_bottom(Line::from(Span::styled(
                scroll_indicator,
                Style::default().fg(Color::DarkGray),
            )));
        frame.render_widget(block_with_hint, area);
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll as u16, 0));

    frame.render_widget(paragraph, inner);
}

fn render_chat_input(frame: &mut Frame, area: Rect, state: &AppState, is_active: bool) {
    let border_style = if is_active {
        Style::default().fg(palette::ACTIVE_PRIMARY)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_active {
        "Enter:Send │ Shift+Enter:Newline │ Esc:Cancel"
    } else {
        "Press 'c' to chat"
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let input_text = if is_active {
        format!("{}█", &state.input_buffer)
    } else {
        state.input_buffer.clone()
    };

    let paragraph = Paragraph::new(input_text)
        .block(block)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}
