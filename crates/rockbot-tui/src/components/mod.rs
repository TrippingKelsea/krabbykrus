//! UI Components for the TUI
//!
//! Each component handles rendering a specific section of the UI.
//! Components are stateless renderers that read from AppState.

pub mod agents;
pub mod card_chain;
pub mod card_widgets;
pub mod context_menu;
pub mod credentials;
pub mod cron;
pub mod dashboard;
pub mod modals;
pub mod models;
pub mod sessions;
pub mod settings;
pub mod sidebar;

pub use agents::render_agents;
pub use card_chain::render_slot_bar;
pub use credentials::render_credentials;
pub use cron::render_cron_jobs;
pub use dashboard::render_dashboard;
pub use modals::{
    render_add_credential_modal, render_confirm_modal, render_create_session_modal,
    render_edit_agent_modal, render_edit_context_file_modal, render_edit_credential_modal,
    render_edit_permission_modal, render_edit_provider_modal, render_password_modal,
    render_view_context_files_modal, render_view_endpoint_modal, render_view_model_list_modal,
    render_view_permission_modal, render_view_provider_modal, render_view_session_modal,
};
pub use models::render_models;
pub use sessions::{render_chat_area, render_chat_input, render_chat_messages, render_sessions};
pub use settings::render_settings;
pub use sidebar::render_sidebar;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Wrap},
    Frame,
};

/// Render a loading spinner
pub fn render_spinner(frame: &mut Frame, area: Rect, message: &str, tick: usize) {
    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = spinner_chars[tick % spinner_chars.len()];

    let text = format!("{spinner} {message}");
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render an error message block
pub fn render_error(frame: &mut Frame, area: Rect, error: &str) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled("Error", Style::default().fg(Color::Red)));

    let paragraph = Paragraph::new(error)
        .style(Style::default().fg(Color::Red))
        .wrap(Wrap { trim: true })
        .block(block);

    frame.render_widget(paragraph, area);
}

/// Render status bar at bottom of screen
pub fn render_status_bar(
    frame: &mut Frame,
    area: Rect,
    message: Option<&(String, bool)>,
    help_text: &str,
) {
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

    let status = Paragraph::new(format!(" {text}")).style(style);

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
    format!("{key}:{action} ")
}

/// Split a detail area into a title row + body, and render the title.
/// Returns the body `Rect` for the caller to render content into.
pub fn render_detail_header(frame: &mut Frame, area: Rect, title: &str) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(area);

    let header = Paragraph::new(Line::from(vec![Span::styled(
        format!(" {title}"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    frame.render_widget(header, chunks[0]);

    // Body area with 1-char left padding
    Rect {
        x: chunks[1].x + 1,
        y: chunks[1].y,
        width: chunks[1].width.saturating_sub(1),
        height: chunks[1].height,
    }
}

/// Render horizontal scroll indicators below a card strip.
/// `area` is the filler rect after the last visible card.
/// `can_left`/`can_right` indicate whether more items exist off-screen.
pub fn render_card_scroll_hint(frame: &mut Frame, area: Rect, can_left: bool, can_right: bool) {
    if !can_left && !can_right {
        return;
    }
    let hint = match (can_left, can_right) {
        (true, true) => "◀ ▶",
        (true, false) => "◀",
        (false, true) => "▶",
        _ => "",
    };
    if area.width < hint.len() as u16 || area.height == 0 {
        return;
    }
    let row = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let p = Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray)))
        .alignment(Alignment::Right);
    frame.render_widget(p, row);
}
