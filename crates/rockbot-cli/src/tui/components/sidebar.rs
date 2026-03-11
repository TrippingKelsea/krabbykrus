//! Sidebar navigation component

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::{AppState, MenuItem};

/// Render the sidebar navigation
pub fn render_sidebar(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let items: Vec<ListItem> = MenuItem::all()
        .iter()
        .map(|item| {
            let content = format!("{} {}", item.icon(), item.title());
            ListItem::new(content)
        })
        .collect();

    // Use animated purple border when sidebar is focused
    let border_style = if state.sidebar_focus {
        effects::active_border_style(effect_state.elapsed_secs())
    } else {
        effects::inactive_border_style()
    };

    let highlight_style = if state.sidebar_focus {
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("🦀 RockBot"),
        )
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.menu_index));
    
    frame.render_stateful_widget(list, area, &mut list_state);
}
