//! Sessions component - chat fills full area (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        Wrap,
    },
    Frame,
};

use crate::effects::{palette, EffectState};
use crate::state::{AppState, ChatRole, InputMode};

/// Rotating words shown while the model is processing
const THINKING_WORDS: &[&str] = &[
    "thinking...",
    "reasoning...",
    "pondering...",
    "considering...",
    "analyzing...",
    "reflecting...",
    "processing...",
    "evaluating...",
    "deliberating...",
    "synthesizing...",
    "contemplating...",
    "formulating...",
];

/// Pick a thinking word based on tick count, or show tool name if running a tool
fn thinking_word(tick: usize, tool_name: Option<&str>) -> String {
    if let Some(name) = tool_name {
        return format!("running {name}...");
    }
    // Change word every ~8 ticks (roughly every 2 seconds at 4 ticks/sec)
    let idx = (tick / 8) % THINKING_WORDS.len();
    THINKING_WORDS[idx].to_string()
}

/// Render the sessions page — chat fills the full area (cards are in top slot bar)
pub fn render_sessions(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    render_chat_area(frame, area, state, effect_state);
}

/// Render the chat area (messages + input) — takes full width
pub fn render_chat_area(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let is_chat_mode = matches!(state.input_mode, InputMode::ChatInput);

    let messages = state.chat_messages();
    let chat_loading = state.chat_loading();
    let chat_scroll = state.chat_scroll();
    let auto_scroll = state.chat_auto_scroll();

    // Calculate input height accounting for both explicit newlines and visual line wrapping
    // Subtract borders (2) + 1 for preemptive wrap so box grows before text hits the edge
    let inner_width = area.width.saturating_sub(3).max(1) as usize;
    let visual_lines: usize = state
        .input_buffer
        .split('\n')
        .map(|line| {
            // +1 for the cursor character that is rendered inline
            let char_count = (line.len() + 1).max(1);
            (char_count + inner_width - 1) / inner_width // ceiling division
        })
        .sum();
    let input_line_count = visual_lines.clamp(1, 10);
    let input_height = (input_line_count as u16) + 2; // +2 for borders

    // Split into messages area + input bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(input_height)])
        .split(area);

    if !messages.is_empty() || chat_loading {
        render_chat_messages(
            frame,
            chunks[0],
            state,
            messages,
            chat_loading,
            chat_scroll,
            auto_scroll,
        );
    } else if state.sessions.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Press 'n' to create a new session",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    } else if let Some(err) = &state.sessions_error {
        let content = Paragraph::new(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        )))
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    } else {
        // Session selected but no messages
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No messages yet. Press 'c' to start chatting.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, chunks[0]);
    }

    render_chat_input(frame, chunks[1], state, is_chat_mode);
}

pub fn render_chat_messages(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    messages: &[crate::state::ChatMessage],
    loading: bool,
    scroll: usize,
    auto_scroll: bool,
) {
    let inner = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };

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

        let timestamp = msg
            .timestamp
            .as_ref()
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
            let expand_hint = if !tc.result.is_empty() {
                if tc.expanded {
                    " [-]"
                } else {
                    " [+]"
                }
            } else {
                ""
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("[{status_icon}]"),
                    Style::default()
                        .fg(status_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", tc.tool_name),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(duration, Style::default().fg(Color::DarkGray)),
                Span::styled(expand_hint, Style::default().fg(Color::DarkGray)),
            ]));
            if tc.expanded && !tc.result.is_empty() {
                for result_line in tc.result.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("    {result_line}"),
                        Style::default().fg(Color::DarkGray),
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
        let thinking = state.active_chat().map(|c| &c.thinking);
        let thinking_label = thinking_word(
            state.tick_count,
            thinking.and_then(|t| t.tool_name.as_deref()),
        );
        let mut indicator_spans = vec![
            Span::styled(
                "AI: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                thinking_label,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ];

        // Show token stats if we have any
        if let Some(ts) = thinking {
            if ts.cumulative_total > 0 {
                let tps = ts.tokens_per_second();
                let tps_str = if tps > 0.5 {
                    format!("  [{} tok | {:.1} tok/s]", ts.cumulative_total, tps)
                } else {
                    format!("  [{} tok]", ts.cumulative_total)
                };
                indicator_spans.push(Span::styled(tps_str, Style::default().fg(Color::DarkGray)));
            }
        }

        lines.push(Line::from(indicator_spans));
    }

    // Compute actual wrapped line count by rendering to a scratch buffer.
    // Paragraph::line_count gives the exact post-wrap count so our scroll
    // offset matches what ratatui will render.
    let view_width = inner.width.max(1);
    let view_height = inner.height as usize;

    let scratch = Paragraph::new(lines.clone()).wrap(Wrap { trim: false });
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
            .borders(Borders::NONE)
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

    // Vertical scrollbar
    if total_visual_lines > view_height {
        let sb_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height,
        };
        let mut sb_state = ScrollbarState::new(max_scroll).position(effective_scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            sb_area,
            &mut sb_state,
        );
    }
}

pub fn render_chat_input(frame: &mut Frame, area: Rect, state: &AppState, is_active: bool) {
    let border_style = if is_active {
        Style::default().fg(palette::ACTIVE_PRIMARY)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_active {
        "Enter:Send │ Shift+Enter / Ctrl+J:Newline │ PgUp/Dn:Scroll │ Ctrl+R:Retry │ Esc:Back"
    } else {
        "Press 'c' to chat │ 'd' to archive"
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(title);

    if is_active {
        // Build text with visible cursor at the correct position
        let cursor_pos = state.input_cursor.min(state.input_buffer.len());
        let before = &state.input_buffer[..cursor_pos];
        let after = &state.input_buffer[cursor_pos..];
        // Insert block cursor character and render as plain text so wrapping works
        let input_text = format!("{before}█{after}");
        let paragraph = Paragraph::new(input_text)
            .block(block)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    } else {
        let paragraph = Paragraph::new(state.input_buffer.as_str())
            .block(block)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }
}
