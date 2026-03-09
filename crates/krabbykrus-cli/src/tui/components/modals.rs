//! Modal dialog components

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use crate::tui::state::{AddCredentialState, AgentInfo, EditAgentState, EditCredentialState, EditProviderState, ProviderAuthType, SessionInfo, get_fields_for_endpoint_type};
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

/// Render the edit credential modal (similar to add but pre-populated)
pub fn render_edit_credential_modal(frame: &mut Frame, area: Rect, state: &EditCredentialState) {
    let fields = get_fields_for_endpoint_type(state.endpoint_type);
    
    // Calculate modal height based on number of fields
    // Name (3) + dynamic fields (3 each) + help (2) + layout margins (2) + block borders (2)
    let content_height = 3 + (fields.len() * 3) + 2 + 4;
    let modal_height = (content_height as u16).min(area.height.saturating_sub(4)).max(20);
    let modal_percent_y = ((modal_height as f32 / area.height as f32) * 100.0) as u16;
    
    let modal_area = centered_rect(65, modal_percent_y.max(50), area);
    frame.render_widget(Clear, modal_area);
    
    let title_type_name = get_endpoint_type_name(state.endpoint_type);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(format!("Edit {} Endpoint", title_type_name));
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    // Build constraints dynamically (no type selector in edit mode)
    let mut constraints = vec![
        Constraint::Length(3), // Name
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
    
    // Render dynamic fields
    for (i, field) in fields.iter().enumerate() {
        let value = state.field_values.get(i).map(|s| s.as_str()).unwrap_or("");
        let is_active = state.dynamic_field_index() == Some(i);
        
        // Add required indicator and modified indicator for secrets
        let label = if field.masked && state.secret_modified {
            format!("{} * (modified)", field.label)
        } else if field.required {
            format!("{} *", field.label)
        } else {
            field.label.to_string()
        };
        
        render_input_field(
            frame,
            chunks[1 + i],
            &label,
            value,
            field.placeholder,
            is_active,
            field.masked,
            field.required,
        );
    }
    
    // Help text
    let help_idx = 1 + fields.len();
    if help_idx < chunks.len() {
        let help_text = if state.is_last_field() {
            "Tab/↑↓: Navigate | Enter: Save | Esc: Cancel"
        } else {
            "Tab/↑↓: Navigate | Enter: Next | Esc: Cancel"
        };
        
        let help = Paragraph::new(help_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(help, chunks[help_idx]);
    }
}

/// Render the view session modal
pub fn render_view_session_modal(
    frame: &mut Frame, 
    area: Rect, 
    session_key: &str,
    sessions: &[SessionInfo],
) {
    let modal_area = centered_rect(60, 50, area);
    frame.render_widget(Clear, modal_area);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!("Session: {}", session_key));
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    // Find session info
    let session = sessions.iter().find(|s| s.key == session_key);
    
    let content = if let Some(session) = session {
        vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Session Key: ", Style::default().fg(Color::Gray)),
                Span::styled(&session.key, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("Agent: ", Style::default().fg(Color::Gray)),
                Span::styled(&session.agent_id, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Channel: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    session.channel.as_deref().unwrap_or("unknown"),
                    Style::default().fg(Color::Yellow)
                ),
            ]),
            Line::from(vec![
                Span::styled("Started: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    session.started_at.as_deref().unwrap_or("unknown"),
                    Style::default().fg(Color::White)
                ),
            ]),
            Line::from(vec![
                Span::styled("Messages: ", Style::default().fg(Color::Gray)),
                Span::styled(
                    session.message_count.to_string(),
                    Style::default().fg(Color::Green)
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Press Esc or Enter to close",
                Style::default().fg(Color::DarkGray)
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "Session not found",
                Style::default().fg(Color::Red)
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press Esc or Enter to close",
                Style::default().fg(Color::DarkGray)
            )),
        ]
    };
    
    let paragraph = Paragraph::new(content)
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::NONE));
    
    let inner_margin = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner);
    
    frame.render_widget(paragraph, inner_margin[0]);
}

/// Render the edit provider modal
pub fn render_edit_provider_modal(
    frame: &mut Frame,
    area: Rect,
    state: &EditProviderState,
) {
    let modal_area = centered_rect(60, 50, area);
    frame.render_widget(Clear, modal_area);
    
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" Configure {} ", state.provider_name));
    
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);
    
    // Build form content
    let mut lines = vec![
        Line::from(Span::styled(
            &state.provider_name,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        )),
        Line::from(""),
    ];
    
    // Auth type selector (field 0)
    let auth_type_focused = state.field_index == 0;
    let auth_options = ProviderAuthType::all_for_provider(state.provider_index);
    let auth_type_display = if auth_options.len() > 1 {
        format!("◀ {} ▶", state.auth_type.label())
    } else {
        state.auth_type.label().to_string()
    };
    
    lines.push(Line::from(vec![
        Span::styled(
            if auth_type_focused { "▶ " } else { "  " },
            Style::default().fg(Color::Yellow)
        ),
        Span::styled("Auth Type: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            auth_type_display,
            if auth_type_focused {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            }
        ),
    ]));
    
    if auth_type_focused && auth_options.len() > 1 {
        lines.push(Line::from(Span::styled(
            "    (←/→ to change)",
            Style::default().fg(Color::DarkGray)
        )));
    }
    lines.push(Line::from(""));
    
    // Dynamic fields based on auth type
    match state.auth_type {
        ProviderAuthType::ApiKey => {
            // API Key field (field 1)
            let api_key_focused = state.field_index == 1;
            let display_text = if api_key_focused && state.api_key.is_empty() {
                "type your key...".to_string()
            } else if state.api_key.is_empty() {
                "".to_string()
            } else {
                "*".repeat(state.api_key.len().min(30))
            };
            let key_style = if api_key_focused {
                Style::default().fg(Color::Yellow)
            } else if state.api_key.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Green)
            };
            
            lines.push(Line::from(vec![
                Span::styled(
                    if api_key_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled("API Key: ", Style::default().fg(Color::Cyan)),
                Span::styled(display_text, key_style),
                if api_key_focused { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
            ]));
            lines.push(Line::from(""));
            
            // Base URL field (field 2)
            let base_url_focused = state.field_index == 2;
            lines.push(Line::from(vec![
                Span::styled(
                    if base_url_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled("Base URL: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &state.base_url,
                    if base_url_focused {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                if base_url_focused { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
            ]));
        }
        ProviderAuthType::SessionKey => {
            // Session key info
            lines.push(Line::from(Span::styled(
                "  Uses Claude Code credentials (~/.claude/.credentials.json)",
                Style::default().fg(Color::Gray)
            )));
            lines.push(Line::from(""));
            
            // Check if credentials exist
            let has_creds = if let Some(home) = dirs::home_dir() {
                home.join(".claude").join(".credentials.json").exists()
            } else {
                false
            };
            
            if has_creds {
                lines.push(Line::from(Span::styled(
                    "  ✓ Claude Code credentials found",
                    Style::default().fg(Color::Green)
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  ✗ Run 'claude' to authenticate",
                    Style::default().fg(Color::Red)
                )));
            }
            lines.push(Line::from(""));
            
            // Base URL field (field 1)
            let base_url_focused = state.field_index == 1;
            lines.push(Line::from(vec![
                Span::styled(
                    if base_url_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled("Base URL: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &state.base_url,
                    if base_url_focused {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                if base_url_focused { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
            ]));
        }
        ProviderAuthType::None => {
            lines.push(Line::from(Span::styled(
                "  No authentication required (local service)",
                Style::default().fg(Color::Green)
            )));
            lines.push(Line::from(""));
            
            // Base URL field (field 1)
            let base_url_focused = state.field_index == 1;
            lines.push(Line::from(vec![
                Span::styled(
                    if base_url_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled("Base URL: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &state.base_url,
                    if base_url_focused {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                if base_url_focused { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
            ]));
        }
        ProviderAuthType::AwsCredentials => {
            lines.push(Line::from(Span::styled(
                "  Uses AWS credentials from environment/config",
                Style::default().fg(Color::Gray)
            )));
            lines.push(Line::from(""));
            
            // Check AWS credentials
            let has_aws = std::env::var("AWS_ACCESS_KEY_ID").is_ok() 
                && std::env::var("AWS_SECRET_ACCESS_KEY").is_ok();
            
            if has_aws {
                lines.push(Line::from(Span::styled(
                    "  ✓ AWS credentials found in environment",
                    Style::default().fg(Color::Green)
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "  ✗ Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY",
                    Style::default().fg(Color::Red)
                )));
            }
            lines.push(Line::from(""));
            
            // AWS Region field (field 1)
            let region_focused = state.field_index == 1;
            lines.push(Line::from(vec![
                Span::styled(
                    if region_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled("AWS Region: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &state.aws_region,
                    if region_focused {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::White)
                    }
                ),
                if region_focused { Span::styled("█", Style::default().fg(Color::Yellow)) } else { Span::raw("") },
            ]));
        }
    }
    
    // Footer help
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab/↑↓: Navigate │ Enter: Save │ Esc: Cancel",
        Style::default().fg(Color::DarkGray)
    )));
    
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::NONE));

    let inner_margin = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner);

    frame.render_widget(paragraph, inner_margin[0]);
}

/// Render the add/edit agent modal
pub fn render_edit_agent_modal(
    frame: &mut Frame,
    area: Rect,
    state: &EditAgentState,
    agents: &[AgentInfo],
) {
    let modal_area = centered_rect(65, 70, area);
    frame.render_widget(Clear, modal_area);

    let title = if state.is_edit {
        format!("Edit Agent: {}", state.id)
    } else {
        "Create Agent".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let constraints = vec![
        Constraint::Length(3), // Agent ID
        Constraint::Length(3), // Model
        Constraint::Length(3), // Parent Agent
        Constraint::Length(3), // Workspace
        Constraint::Length(3), // Max Tool Calls
        Constraint::Length(3), // System Prompt
        Constraint::Length(2), // Help/subagent info
        Constraint::Min(0),   // Spacer
    ];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(constraints)
        .split(inner);

    // Field 0: Agent ID
    let id_active = state.field_index == 0;
    let id_label = if state.is_edit { "Agent ID (read-only)" } else { "Agent ID" };
    render_input_field(
        frame, chunks[0], id_label, &state.id,
        "e.g., my-agent", id_active, false, true,
    );

    // Field 1: Model
    render_input_field(
        frame, chunks[1], "Model", &state.model,
        "e.g., anthropic/claude-sonnet-4-20250514", state.field_index == 1, false, false,
    );

    // Field 2: Parent Agent (subagent)
    let parent_hint = if !state.parent_id.is_empty() {
        let parent_exists = agents.iter().any(|a| a.id == state.parent_id);
        if parent_exists { "(valid parent)" } else { "(parent not found!)" }
    } else {
        "empty = top-level agent"
    };
    let parent_label = format!("Parent Agent {}", parent_hint);
    render_input_field(
        frame, chunks[2], &parent_label, &state.parent_id,
        "leave empty for top-level", state.field_index == 2, false, false,
    );

    // Field 3: Workspace
    render_input_field(
        frame, chunks[3], "Workspace", &state.workspace,
        "uses default if empty", state.field_index == 3, false, false,
    );

    // Field 4: Max Tool Calls
    render_input_field(
        frame, chunks[4], "Max Tool Calls", &state.max_tool_calls,
        "10", state.field_index == 4, false, false,
    );

    // Field 5: System Prompt
    render_input_field(
        frame, chunks[5], "System Prompt", &state.system_prompt,
        "optional override", state.field_index == 5, false, false,
    );

    // Subagent info / help line
    let subagents: Vec<&str> = agents.iter()
        .filter(|a| a.parent_id.as_deref() == Some(&state.id))
        .map(|a| a.id.as_str())
        .collect();

    let help_text = if !subagents.is_empty() {
        format!("Subagents: {} | Tab:Nav | Enter:Save | Esc:Cancel", subagents.join(", "))
    } else {
        "Tab/Up/Down:Navigate | Enter:Save | Esc:Cancel".to_string()
    };

    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[6]);
}
