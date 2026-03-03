//! Modal dialog components

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::state::{AddCredentialField, AddCredentialState};
use super::centered_rect;

/// Endpoint types for the add credential modal
pub const ENDPOINT_TYPES: &[(&str, &str)] = &[
    ("home_assistant", "Home Assistant"),
    ("generic_rest", "Generic REST API"),
    ("generic_oauth2", "OAuth2 Service"),
    ("api_key_service", "API Key Service"),
    ("basic_auth_service", "Basic Auth Service"),
    ("bearer_token", "Bearer Token"),
];

/// Number of endpoint types (for modulo in selection)
pub const ENDPOINT_TYPE_COUNT: usize = 6;

/// Render the password input modal
pub fn render_password_modal(
    frame: &mut Frame,
    area: Rect,
    prompt: &str,
    masked: bool,
    input: &str,
) {
    let modal_area = centered_rect(50, 20, area);
    frame.render_widget(Clear, modal_area);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Vault");
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2), // Prompt
            Constraint::Length(3), // Input
            Constraint::Length(1), // Help
        ])
        .split(inner);

    let prompt_para = Paragraph::new(prompt)
        .style(Style::default().fg(Color::White));
    frame.render_widget(prompt_para, chunks[0]);
    
    let display_value = if masked {
        "*".repeat(input.len())
    } else {
        input.to_string()
    };
    
    let input_para = Paragraph::new(format!("{}█", display_value))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(input_para, chunks[1]);
    
    let help = Paragraph::new("Enter to submit, Esc to cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[2]);
}

/// Render the confirmation modal
pub fn render_confirm_modal(frame: &mut Frame, area: Rect, message: &str) {
    let modal_area = centered_rect(40, 15, area);
    frame.render_widget(Clear, modal_area);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title("Confirm");
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    let text = vec![
        Line::from(""),
        Line::from(message.to_string()),
        Line::from(""),
        Line::from(Span::styled("[y]es  [n]o", Style::default().fg(Color::DarkGray))),
    ];
    
    let para = Paragraph::new(text)
        .alignment(Alignment::Center);
    frame.render_widget(para, inner);
}

/// Render the add credential modal
pub fn render_add_credential_modal(frame: &mut Frame, area: Rect, state: &AddCredentialState) {
    let modal_area = centered_rect(60, 70, area);
    frame.render_widget(Clear, modal_area);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Add Service Endpoint");
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Name
            Constraint::Length(3), // Type
            Constraint::Length(3), // URL
            Constraint::Length(3), // Secret
            Constraint::Length(3), // Expiration
            Constraint::Length(2), // Help
            Constraint::Min(0),    // Spacer
        ])
        .split(inner);

    // Helper to render input field
    fn render_field(
        frame: &mut Frame,
        area: Rect,
        label: &str,
        value: &str,
        active: bool,
        masked: bool,
    ) {
        let style = if active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        
        let display_value = if masked && !value.is_empty() {
            "*".repeat(value.len())
        } else {
            value.to_string()
        };
        
        let cursor = if active { "█" } else { "" };
        
        let text = format!("{}: {}{}", label, display_value, cursor);
        let paragraph = Paragraph::new(text)
            .style(style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(paragraph, area);
    }

    render_field(
        frame,
        chunks[0],
        "Endpoint Name",
        &state.name,
        state.field == AddCredentialField::Name,
        false,
    );
    
    // Service Type selector
    let type_style = if state.field == AddCredentialField::EndpointType {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    let type_name = ENDPOINT_TYPES
        .get(state.endpoint_type)
        .map(|(_, n)| *n)
        .unwrap_or("Unknown");
    let type_text = format!("Service Type: ◀ {} ▶", type_name);
    let type_para = Paragraph::new(type_text)
        .style(type_style)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(type_para, chunks[1]);
    
    render_field(
        frame,
        chunks[2],
        "Base URL",
        &state.url,
        state.field == AddCredentialField::Url,
        false,
    );
    
    render_field(
        frame,
        chunks[3],
        "Token/Secret",
        &state.secret,
        state.field == AddCredentialField::Secret,
        true,
    );
    
    render_field(
        frame,
        chunks[4],
        "Expires (opt)",
        &state.expiration,
        state.field == AddCredentialField::Expiration,
        false,
    );
    
    let help = Paragraph::new("Tab/↑↓: Fields | ←→: Select type | Enter: Next/Submit | Esc: Cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[5]);
}
