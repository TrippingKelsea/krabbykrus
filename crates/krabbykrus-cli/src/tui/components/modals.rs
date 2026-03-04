//! Modal dialog components

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::tui::state::{AddCredentialState, get_fields_for_endpoint_type};
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

/// Get display name for endpoint type by index
fn get_endpoint_type_name(idx: usize) -> &'static str {
    match idx {
        0 => "Home Assistant",
        1 => "Generic REST API",
        2 => "OAuth2 Service",
        3 => "API Key Service",
        4 => "Basic Auth Service",
        5 => "Bearer Token",
        _ => "Unknown",
    }
}

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

/// Render the add credential modal with dynamic fields
pub fn render_add_credential_modal(frame: &mut Frame, area: Rect, state: &AddCredentialState) {
    let fields = get_fields_for_endpoint_type(state.endpoint_type);
    
    // Calculate modal height based on number of fields
    // Name (3) + Type (3) + dynamic fields (3 each) + help (2) + layout margins (2) + block borders (2)
    let content_height = 3 + 3 + (fields.len() * 3) + 2 + 4;
    let modal_height = (content_height as u16).min(area.height.saturating_sub(4)).max(20);
    let modal_percent_y = ((modal_height as f32 / area.height as f32) * 100.0) as u16;
    
    let modal_area = centered_rect(65, modal_percent_y.max(50), area);
    frame.render_widget(Clear, modal_area);
    
    let title_type_name = get_endpoint_type_name(state.endpoint_type);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!("Add {} Endpoint", title_type_name));
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    // Build constraints dynamically
    let mut constraints = vec![
        Constraint::Length(3), // Name
        Constraint::Length(3), // Type selector
    ];
    for _ in &fields {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(2)); // Help
    constraints.push(Constraint::Min(0));    // Spacer
    
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(constraints)
        .split(inner);

    // Render Name field
    render_input_field(
        frame,
        chunks[0],
        "Endpoint Name",
        &state.name,
        "",
        state.is_name_field(),
        false,
        true,
    );
    
    // Render Type selector - use render_input_field with arrows in the value
    let selector_type_name = get_endpoint_type_name(state.endpoint_type);
    let type_value = format!("◀ {} ▶", selector_type_name);
    render_input_field(
        frame,
        chunks[1],
        "Service Type",
        &type_value,
        "",
        state.is_type_field(),
        false,
        false, // not required (it always has a value)
    );
    
    // Render dynamic fields
    for (i, field) in fields.iter().enumerate() {
        let value = state.field_values.get(i).map(|s| s.as_str()).unwrap_or("");
        let is_active = state.dynamic_field_index() == Some(i);
        
        // Add required indicator
        let label = if field.required {
            format!("{} *", field.label)
        } else {
            field.label.to_string()
        };
        
        render_input_field(
            frame,
            chunks[2 + i],
            &label,
            value,
            field.placeholder,
            is_active,
            field.masked,
            field.required,
        );
    }
    
    // Help text
    let help_idx = 2 + fields.len();
    if help_idx < chunks.len() {
        let help_text = if state.is_type_field() {
            "←→: Change type | Tab/↑↓: Navigate | Enter: Next | Esc: Cancel"
        } else if state.is_last_field() {
            "Tab/↑↓: Navigate | Enter: Submit | Esc: Cancel"
        } else {
            "Tab/↑↓: Navigate | Enter: Next | Esc: Cancel"
        };
        
        let help = Paragraph::new(help_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(help, chunks[help_idx]);
    }
}

/// Render a single input field
fn render_input_field(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: &str,
    placeholder: &str,
    active: bool,
    masked: bool,
    required: bool,
) {
    let border_style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    
    let label_style = if active {
        Style::default().fg(Color::Yellow)
    } else if required && value.is_empty() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::White)
    };
    
    let display_value = if masked && !value.is_empty() {
        "*".repeat(value.len())
    } else if value.is_empty() && !active {
        placeholder.to_string()
    } else {
        value.to_string()
    };
    
    let value_style = if value.is_empty() && !active {
        Style::default().fg(Color::DarkGray) // Placeholder style
    } else if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };
    
    let cursor = if active { "█" } else { "" };
    
    let text = Line::from(vec![
        Span::styled(format!("{}: ", label), label_style),
        Span::styled(display_value, value_style),
        Span::styled(cursor, Style::default().fg(Color::Yellow)),
    ]);
    
    let paragraph = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_style(border_style));
    
    frame.render_widget(paragraph, area);
}
