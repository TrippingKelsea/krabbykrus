//! Sidebar navigation component — compact scrollable menu

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::effects::{self, EffectState};
use crate::state::{AppState, MenuItem};

/// Render the sidebar navigation as a compact scrollable menu.
/// Fits within the same height as the card strip (typically 5 rows).
pub fn render_sidebar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let all_items = MenuItem::all();
    let total = all_items.len();
    let selected = state.menu_index;

    // Border takes 0 top, 0 bottom, 0 left, 1 right = usable height is area.height
    // We use Borders::RIGHT as the visual separator
    let usable = area.height as usize;

    // Determine the visible window: keep selected item centered
    let (start, visible_count) = if total <= usable {
        (0, total)
    } else {
        let half = usable / 2;
        let start = if selected < half {
            0
        } else if selected + half >= total {
            total - usable
        } else {
            selected - half
        };
        (start, usable)
    };
    let end = (start + visible_count).min(total);

    let can_scroll_up = start > 0;
    let can_scroll_down = end < total;

    // Build scroll indicator for right border
    let border_style = effects::inactive_border_style();

    // Build the right-border title to show scroll arrows
    let scroll_hint = match (can_scroll_up, can_scroll_down) {
        (true, true) => "▲▼",
        (true, false) => "▲ ",
        (false, true) => " ▼",
        (false, false) => "",
    };

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(border_style)
        .title_bottom(
            Line::from(Span::styled(
                scroll_hint,
                Style::default().fg(Color::DarkGray),
            ))
            .right_aligned(),
        );

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Render visible menu items
    for (vi, idx) in (start..end).enumerate() {
        let item = &all_items[idx];
        let is_selected = idx == selected;

        let style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };

        let num = idx + 1;
        let prefix = if is_selected { "▶" } else { " " };
        let text = format!("{prefix}{num} {} {}", item.icon(), item.title());

        if vi < inner.height as usize {
            let row = Rect {
                x: inner.x,
                y: inner.y + vi as u16,
                width: inner.width,
                height: 1,
            };
            let line = Paragraph::new(text).style(style);
            frame.render_widget(line, row);
        }
    }
}
