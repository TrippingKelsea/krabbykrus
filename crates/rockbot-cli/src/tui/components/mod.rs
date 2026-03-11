//! UI Components for the TUI
//!
//! Each component handles rendering a specific section of the UI.
//! Components are stateless renderers that read from AppState.

pub mod dashboard;
pub mod credentials;
pub mod agents;
pub mod sessions;
pub mod models;
pub mod settings;
pub mod sidebar;
pub mod modals;

pub use dashboard::render_dashboard;
pub use credentials::render_credentials;
pub use agents::render_agents;
pub use sessions::render_sessions;
pub use models::render_models;
pub use settings::render_settings;
pub use sidebar::render_sidebar;
pub use modals::{
    render_password_modal, render_confirm_modal, render_add_credential_modal,
    render_edit_credential_modal, render_edit_provider_modal, render_view_session_modal,
    render_edit_agent_modal,
};

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Span,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Render a loading spinner
pub fn render_spinner(frame: &mut Frame, area: Rect, message: &str, tick: usize) {
    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = spinner_chars[tick % spinner_chars.len()];
    
    let text = format!("{} {}", spinner, message);
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    
    frame.render_widget(paragraph, area);
}

/// Render an error message block
pub fn render_error(frame: &mut Frame, area: Rect, error: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled("Error", Style::default().fg(Color::Red)));
    
    let paragraph = Paragraph::new(error)
        .style(Style::default().fg(Color::Red))
        .wrap(Wrap { trim: true })
        .block(block);
    
    frame.render_widget(paragraph, area);
}

/// Render status bar at bottom of screen
pub fn render_status_bar(frame: &mut Frame, area: Rect, message: Option<&(String, bool)>, help_text: &str) {
    let (text, style) = match message {
        Some((msg, is_error)) => {
            let style = if *is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            (msg.clone(), style)
        }
        None => (help_text.to_string(), Style::default().fg(Color::DarkGray)),
    };
    
    let status = Paragraph::new(text)
        .style(style)
        .block(Block::default().borders(Borders::TOP));
    
    frame.render_widget(status, area);
}

/// Create a centered rectangle for modals
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Create styled key hint spans
pub fn key_hint(key: &str, action: &str) -> String {
    format!("{}:{} ", key, action)
}
