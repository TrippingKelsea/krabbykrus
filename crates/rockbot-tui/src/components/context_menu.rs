//! Context menu overlay component

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph},
    Frame,
};

use crate::effects::palette;
use crate::state::ContextMenuState;

/// Render the context menu as a floating panel
pub fn render_context_menu(frame: &mut Frame, _area: Rect, menu: &ContextMenuState) {
    // Compute menu dimensions
    let max_label = menu.items.iter().map(|i| i.label.len()).max().unwrap_or(10);
    let width = (max_label + 8) as u16; // "[k] Label" + padding
    let height = menu.items.len() as u16 + 2; // items + top/bottom border

    let (px, py) = menu.position;
    let frame_area = frame.area();

    // Clamp to screen
    let x = px.min(frame_area.width.saturating_sub(width));
    let y = py.min(frame_area.height.saturating_sub(height));

    let menu_rect = Rect {
        x,
        y,
        width: width.min(frame_area.width.saturating_sub(x)),
        height: height.min(frame_area.height.saturating_sub(y)),
    };

    frame.render_widget(Clear, menu_rect);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::ACTIVE_PRIMARY))
        .title("Actions");

    let inner = block.inner(menu_rect);
    frame.render_widget(block, menu_rect);

    // Render items
    for (i, item) in menu.items.iter().enumerate() {
        if i >= inner.height as usize {
            break;
        }

        let is_selected = i == menu.selected;

        let style = if is_selected {
            Style::default()
                .bg(palette::ACTIVE_PRIMARY)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let key_style = if is_selected {
            Style::default()
                .bg(palette::ACTIVE_PRIMARY)
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };

        let line = Line::from(vec![
            Span::styled(format!(" [{}] ", item.key), key_style),
            Span::styled(&item.label, style),
        ]);

        let row = Rect {
            x: inner.x,
            y: inner.y + i as u16,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(line).style(style), row);
    }
}
