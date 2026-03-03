//! Sidebar navigation component

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::tui::state::{AppState, MenuItem};

/// Render the sidebar navigation
pub fn render_sidebar(frame: &mut Frame, area: Rect, state: &AppState) {
    let items: Vec<ListItem> = MenuItem::all()
        .iter()
        .map(|item| {
            let content = format!("{} {}", item.icon(), item.title());
            ListItem::new(content)
        })
        .collect();

    let border_style = if state.sidebar_focus {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("🦀 Krabbykrus"),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.menu_index));
    
    frame.render_stateful_widget(list, area, &mut list_state);
}
