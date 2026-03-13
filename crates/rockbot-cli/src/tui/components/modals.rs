//! Modal dialog components

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::state::{AddCredentialState, AgentInfo, CredentialSchemaInfo, CreateSessionState, EditAgentState, EditCredentialState, EditPermissionState, EditProviderState, EndpointInfo, ModelProvider, PermissionRule, SessionInfo, SessionMode, get_fields_for_endpoint_type};
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
    
    let input_para = Paragraph::new(format!("{display_value}█"))
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
        .title(format!("Add {title_type_name} Endpoint"));
    
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
    let type_value = format!("◀ {selector_type_name} ▶");
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
        let value = state.field_values.get(i).map_or("", std::string::String::as_str);
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
#[allow(clippy::too_many_arguments)]
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
        Span::styled(format!("{label}: "), label_style),
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
        .title(format!("Edit {title_type_name} Endpoint"));
    
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
        let value = state.field_values.get(i).map_or("", std::string::String::as_str);
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
        .title(format!("Session: {session_key}"));
    
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

/// Render the edit provider modal — fully schema-driven.
///
/// This renders the same form whether opened from Credentials→Providers or Models→Edit.
/// The form fields come entirely from the provider's credential schema.
pub fn render_edit_provider_modal(
    frame: &mut Frame,
    area: Rect,
    state: &EditProviderState,
) {
    // Calculate modal height based on field count
    let field_count = state.current_auth_method().map_or(0, |m| m.fields.len());
    let modal_percent = (40 + field_count as u16 * 5).min(80);
    let modal_area = centered_rect(65, modal_percent, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(" Configure {} ", state.provider_name));

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let mut lines = vec![
        Line::from(Span::styled(
            &state.provider_name,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        )),
        Line::from(""),
    ];

    // Auth type selector (field 0) — show label from schema auth method
    let auth_type_focused = state.field_index == 0;
    let auth_method_count = state.schema.as_ref().map_or(1, |s| s.auth_methods.len());
    let auth_label = state.current_auth_method()
        .map_or_else(|| state.auth_type.label().to_string(), |m| m.label.clone());
    let auth_display = if auth_method_count > 1 {
        format!("◀ {auth_label} ▶")
    } else {
        auth_label
    };

    lines.push(Line::from(vec![
        Span::styled(
            if auth_type_focused { "▶ " } else { "  " },
            Style::default().fg(Color::Yellow)
        ),
        Span::styled("Auth Type: ", Style::default().fg(Color::Cyan)),
        Span::styled(
            auth_display,
            if auth_type_focused {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            }
        ),
    ]));

    if auth_type_focused && auth_method_count > 1 {
        lines.push(Line::from(Span::styled(
            "    (←/→ to change)",
            Style::default().fg(Color::DarkGray)
        )));
    }
    lines.push(Line::from(""));

    // Dynamic fields from the current auth method's schema
    if let Some(method) = state.current_auth_method() {
        // Show hint if present
        if method.fields.is_empty() {
            if let Some(hint) = &method.hint {
                lines.push(Line::from(Span::styled(
                    format!("  {hint}"),
                    Style::default().fg(Color::Gray)
                )));
                lines.push(Line::from(""));
            }
        }

        // Render each field from the schema
        for (i, field) in method.fields.iter().enumerate() {
            let field_focused = state.field_index == i + 1;
            let value = state.field_values
                .get(i)
                .map_or("", |(_, v)| v.as_str());

            // Show env var detection for fields with env_var set
            let env_detected = field.env_var.as_ref().and_then(|var| {
                std::env::var(var).ok().map(|_| var.clone())
            });

            // Build label with required indicator
            let label_text = if field.required {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            };

            // Choose display value: entered text > default > placeholder
            let display_value = if field.secret && !value.is_empty() && !field_focused {
                "*".repeat(value.len().min(30))
            } else if value.is_empty() && !field_focused {
                field.default.as_deref()
                    .or(field.placeholder.as_deref())
                    .unwrap_or("")
                    .to_string()
            } else {
                value.to_string()
            };

            let value_style = if field_focused {
                Style::default().fg(Color::Yellow)
            } else if !value.is_empty() {
                if field.secret { Style::default().fg(Color::Green) } else { Style::default().fg(Color::White) }
            } else if field.default.is_some() {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray) // placeholder
            };

            lines.push(Line::from(vec![
                Span::styled(
                    if field_focused { "▶ " } else { "  " },
                    Style::default().fg(Color::Yellow)
                ),
                Span::styled(format!("{label_text}: "), Style::default().fg(Color::Cyan)),
                Span::styled(display_value, value_style),
                if field_focused {
                    Span::styled("█", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]));

            // Show env var status below the field
            if let Some(env_var) = &env_detected {
                lines.push(Line::from(Span::styled(
                    format!("    ✓ Found in ${env_var}"),
                    Style::default().fg(Color::Green)
                )));
            } else if let Some(env_var) = &field.env_var {
                if field_focused {
                    lines.push(Line::from(Span::styled(
                        format!("    env: ${env_var}"),
                        Style::default().fg(Color::DarkGray)
                    )));
                }
            }
        }

        // Show hint below fields (if fields exist and hint is present)
        if !method.fields.is_empty() {
            if let Some(hint) = &method.hint {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  {hint}"),
                    Style::default().fg(Color::DarkGray)
                )));
            }
        }

        // Show docs URL
        if let Some(url) = &method.docs_url {
            lines.push(Line::from(Span::styled(
                format!("  Docs: {url}"),
                Style::default().fg(Color::DarkGray)
            )));
        }
    } else {
        // No schema — legacy fallback message
        lines.push(Line::from(Span::styled(
            "  No configuration schema available for this provider.",
            Style::default().fg(Color::Gray)
        )));
    }

    // Footer help
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab/↑↓: Navigate │ ←→: Auth Type │ Enter: Save │ Esc: Cancel",
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
    // System prompt height accounting for both explicit newlines and visual wrapping
    // Modal is 65% width, minus borders/margin, estimate inner width
    let modal_inner_width = ((area.width as f32) * 0.65) as usize;
    let field_inner_width = modal_inner_width.saturating_sub(4).max(1); // borders + margin
    let prompt_line_count = {
        state.system_prompt.split('\n').map(|line| {
            let char_count = line.len().max(1);
            (char_count + field_inner_width - 1) / field_inner_width
        }).sum::<usize>().clamp(1, 10)
    };
    let prompt_height = (prompt_line_count as u16) + 2;

    // Modal needs to be taller to accommodate the growing prompt
    let base_percent = 70u16;
    let extra = prompt_height.saturating_sub(3); // 3 is the default single-line height
    let modal_percent = (base_percent + extra * 2).min(90);

    let modal_area = centered_rect(65, modal_percent, area);
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
        Constraint::Length(3),             // Agent ID
        Constraint::Length(3),             // Model
        Constraint::Length(3),             // Parent Agent
        Constraint::Length(3),             // Workspace
        Constraint::Length(3),             // Max Tool Calls
        Constraint::Length(prompt_height), // System Prompt (grows up to 12)
        Constraint::Length(2),             // Help/subagent info
        Constraint::Min(0),               // Spacer
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

    // Field 1: Model (picker or text input)
    if state.available_models.is_empty() {
        render_input_field(
            frame, chunks[1], "Model", &state.model,
            "e.g., anthropic/claude-sonnet-4-20250514", state.field_index == 1, false, false,
        );
    } else {
        let is_active = state.field_index == 1;
        let display = if let Some(idx) = state.selected_model_index {
            let model = &state.available_models[idx];
            format!("◀ {} ▶", model.label)
        } else if state.model.is_empty() {
            "◀ (none selected) ▶".to_string()
        } else {
            format!("◀ {} (custom) ▶", state.model)
        };
        let hint = format!("{}/{} models available — ←→ to cycle",
            state.selected_model_index.map_or(0, |i| i + 1),
            state.available_models.len()
        );
        let border_style = if is_active {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title("Model");
        let text_style = if is_active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };
        let content = if is_active {
            vec![
                Line::from(Span::styled(&display, text_style)),
                Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            ]
        } else {
            vec![Line::from(Span::styled(&display, text_style))]
        };
        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, chunks[1]);
    }

    // Field 2: Parent Agent (subagent)
    let parent_hint = if !state.parent_id.is_empty() {
        let parent_exists = agents.iter().any(|a| a.id == state.parent_id);
        if parent_exists { "(valid parent)" } else { "(parent not found!)" }
    } else {
        "empty = top-level agent"
    };
    let parent_label = format!("Parent Agent {parent_hint}");
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

    // Field 5: System Prompt (multi-line textarea)
    {
        let prompt_active = state.field_index == 5;
        let border_style = if prompt_active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = "System Prompt";
        let prompt_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(
                label,
                if prompt_active {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::White)
                },
            ));

        let display_text = if state.system_prompt.is_empty() && !prompt_active {
            "optional override".to_string()
        } else if prompt_active {
            format!("{}█", &state.system_prompt)
        } else {
            state.system_prompt.clone()
        };

        let text_style = if state.system_prompt.is_empty() && !prompt_active {
            Style::default().fg(Color::DarkGray)
        } else if prompt_active {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        let paragraph = Paragraph::new(display_text)
            .block(prompt_block)
            .style(text_style)
            .wrap(ratatui::widgets::Wrap { trim: false });
        frame.render_widget(paragraph, chunks[5]);
    }

    // Subagent info / help line
    let subagents: Vec<&str> = agents.iter()
        .filter(|a| a.parent_id.as_deref() == Some(&state.id))
        .map(|a| a.id.as_str())
        .collect();

    let help_text = if !subagents.is_empty() {
        format!("Subagents: {} | Tab:Nav | Ctrl+S:Save | Esc:Cancel", subagents.join(", "))
    } else {
        "Tab/Up/Down:Navigate | Ctrl+S:Save | Esc:Cancel".to_string()
    };

    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(help, chunks[6]);
}

/// Render the create session modal
pub fn render_create_session_modal(
    frame: &mut Frame,
    area: Rect,
    state: &CreateSessionState,
) {
    let modal_area = centered_rect(55, 40, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Create Session");

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Mode selector
            Constraint::Length(4), // Model/Agent picker
            Constraint::Length(2), // Help text
            Constraint::Min(0),   // Spacer
        ])
        .split(inner);

    // Field 0: Mode selector
    let mode_active = state.field_index == 0;
    let mode_border = if mode_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mode_text = match state.mode {
        SessionMode::AdHoc => "◀ Ad-Hoc (no agent) ▶",
        SessionMode::AgentBound => "◀ Agent-Bound ▶",
    };
    let mode_block = Block::default()
        .borders(Borders::ALL)
        .border_style(mode_border)
        .title("Session Type");
    let mode_style = if mode_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let mode_para = Paragraph::new(Span::styled(mode_text, mode_style)).block(mode_block);
    frame.render_widget(mode_para, chunks[0]);

    // Field 1: Model or Agent picker
    let picker_active = state.field_index == 1;
    let picker_border = if picker_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let picker_style = if picker_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };

    match state.mode {
        SessionMode::AdHoc => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(picker_border)
                .title("Model");

            let (display, hint) = if state.available_models.is_empty() {
                ("(no models available)".to_string(), "Start gateway and configure providers".to_string())
            } else {
                let model = &state.available_models[state.selected_model_index];
                (
                    format!("◀ {} ▶", model.label),
                    format!("{}/{} — ←→ to cycle", state.selected_model_index + 1, state.available_models.len()),
                )
            };

            let content = vec![
                Line::from(Span::styled(display, picker_style)),
                Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            ];
            let para = Paragraph::new(content).block(block);
            frame.render_widget(para, chunks[1]);
        }
        SessionMode::AgentBound => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(picker_border)
                .title("Agent");

            let (display, hint) = if state.available_agents.is_empty() {
                ("(no agents available)".to_string(), "Create an agent first".to_string())
            } else {
                let (_, ref name) = state.available_agents[state.selected_agent_index];
                (
                    format!("◀ {} ▶", name),
                    format!("{}/{} — ←→ to cycle", state.selected_agent_index + 1, state.available_agents.len()),
                )
            };

            let content = vec![
                Line::from(Span::styled(display, picker_style)),
                Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
            ];
            let para = Paragraph::new(content).block(block);
            frame.render_widget(para, chunks[1]);
        }
    }

    // Help
    let help_lines = vec![
        Line::from(Span::styled(
            "←→:Cycle │ Tab/↑↓:Navigate │ Enter:Create │ Esc:Cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let help = Paragraph::new(help_lines);
    frame.render_widget(help, chunks[2]);
}

/// Render a read-only view of an endpoint
pub fn render_view_endpoint_modal(
    frame: &mut Frame,
    area: Rect,
    endpoint_index: usize,
    endpoints: &[EndpointInfo],
) {
    let modal_area = centered_rect(60, 45, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Endpoint Details");

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let content = if let Some(ep) = endpoints.get(endpoint_index) {
        let (cred_text, cred_color) = if ep.has_credential {
            ("Stored", Color::Green)
        } else {
            ("Missing", Color::Red)
        };

        vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                Span::raw(&ep.id),
            ]),
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::Cyan)),
                Span::styled(&ep.name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("Type: ", Style::default().fg(Color::Cyan)),
                Span::raw(&ep.endpoint_type),
            ]),
            Line::from(vec![
                Span::styled("URL: ", Style::default().fg(Color::Cyan)),
                Span::raw(&ep.base_url),
            ]),
            Line::from(vec![
                Span::styled("Credential: ", Style::default().fg(Color::Cyan)),
                Span::styled(cred_text, Style::default().fg(cred_color)),
            ]),
            Line::from(vec![
                Span::styled("Expires: ", Style::default().fg(Color::Cyan)),
                Span::raw(ep.expiration.as_deref().unwrap_or("Never")),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "e:Edit │ Esc:Close",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled("Endpoint not found", Style::default().fg(Color::Red))),
            Line::from(""),
            Line::from(Span::styled("Esc:Close", Style::default().fg(Color::DarkGray))),
        ]
    };

    let paragraph = Paragraph::new(content).alignment(Alignment::Left);

    let inner_margin = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner);

    frame.render_widget(paragraph, inner_margin[0]);
}

/// Render a read-only view of a provider schema
pub fn render_view_provider_modal(
    frame: &mut Frame,
    area: Rect,
    provider_index: usize,
    schemas: &[CredentialSchemaInfo],
    endpoints: &[EndpointInfo],
) {
    let modal_area = centered_rect(60, 50, area);
    frame.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("Provider Details");

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let content = if let Some(schema) = schemas.get(provider_index) {
        let configured = endpoints.iter().any(|e| {
            e.id.to_lowercase().contains(&schema.provider_id)
                || e.name.to_lowercase().contains(&schema.provider_id)
        });
        let (status_text, status_color) = if configured {
            ("Configured", Color::Green)
        } else {
            ("Not configured", Color::Yellow)
        };

        let cat_label = match schema.category.as_str() {
            "model" => "Model Provider (LLM)",
            "communication" => "Communication Channel",
            "tool" => "Tool Integration",
            _ => &schema.category,
        };

        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("Provider: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &schema.provider_name,
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::Cyan)),
                Span::raw(&schema.provider_id),
            ]),
            Line::from(vec![
                Span::styled("Category: ", Style::default().fg(Color::Cyan)),
                Span::raw(cat_label),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]),
        ];

        // Show auth methods
        if !schema.auth_methods.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Auth Methods: ", Style::default().fg(Color::Cyan)),
                Span::raw(format!("{}", schema.auth_methods.len())),
            ]));
            for method in &schema.auth_methods {
                let field_count = method.fields.len();
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(&method.label, Style::default().fg(Color::White)),
                    Span::styled(format!(" ({field_count} fields)"), Style::default().fg(Color::DarkGray)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "e:Configure │ Esc:Close",
            Style::default().fg(Color::DarkGray),
        )));

        lines
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled("Provider not found", Style::default().fg(Color::Red))),
            Line::from(""),
            Line::from(Span::styled("Esc:Close", Style::default().fg(Color::DarkGray))),
        ]
    };

    let paragraph = Paragraph::new(content).alignment(Alignment::Left);

    let provider_inner = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner);

    frame.render_widget(paragraph, provider_inner[0]);
}

/// Render a scrollable model list modal for a provider
pub fn render_view_model_list_modal(
    frame: &mut Frame,
    area: Rect,
    provider_index: usize,
    scroll: usize,
    providers: &[ModelProvider],
) {
    let modal_area = centered_rect(70, 70, area);
    frame.render_widget(Clear, modal_area);

    let provider = providers.get(provider_index);
    let title = provider.map_or_else(
        || "Models".to_string(),
        |p| format!("{} — {} models", p.name, p.models.len()),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let Some(provider) = provider else {
        let msg = Paragraph::new("Provider not found")
            .style(Style::default().fg(Color::Red));
        frame.render_widget(msg, inner);
        return;
    };

    if provider.models.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("No models available", Style::default().fg(Color::DarkGray))),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(msg, inner);
        return;
    }

    let usable_height = inner.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    // Header
    lines.push(Line::from(Span::styled(
        "  Model ID",
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, model) in provider.models.iter().enumerate() {
        let is_highlighted = i == scroll;

        let tokens_info = model.max_output_tokens.map_or_else(
            || format!("{}k ctx", model.context_window / 1000),
            |t| format!("{}k ctx, {}k out", model.context_window / 1000, t / 1000),
        );

        let prefix = if is_highlighted { "▶ " } else { "  " };
        let name_style = if is_highlighted {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let prefix_style = if is_highlighted {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, prefix_style),
            Span::styled(&model.name, name_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(&model.id, Style::default().fg(Color::DarkGray)),
            Span::styled(format!("  ({tokens_info})"), Style::default().fg(Color::Cyan)),
        ]));

        if !model.description.is_empty() {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(&model.description, Style::default().fg(Color::Gray)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Footer
    lines.push(Line::from(Span::styled(
        format!("↑↓:Scroll ({}/{}) │ Esc:Close", scroll + 1, provider.models.len()),
        Style::default().fg(Color::DarkGray),
    )));

    // Calculate scroll offset to keep the highlighted model visible
    let mut target_line = 2; // skip header
    for i in 0..scroll {
        target_line += 3;
        if !provider.models[i].description.is_empty() {
            target_line += 1;
        }
    }
    let line_scroll = if target_line + 4 > usable_height {
        target_line.saturating_sub(2)
    } else {
        0
    };

    let paragraph = Paragraph::new(lines)
        .scroll((line_scroll as u16, 0));

    let model_list_inner = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(0)])
        .split(inner);

    frame.render_widget(paragraph, model_list_inner[0]);
}

/// Render the permission editor modal
pub fn render_edit_permission_modal(
    frame: &mut Frame,
    area: Rect,
    state: &EditPermissionState,
) {
    let modal_area = centered_rect(55, 45, area);
    frame.render_widget(Clear, modal_area);

    let title = if state.is_edit {
        "Edit Permission".to_string()
    } else {
        "Add Permission".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Endpoint selector
            Constraint::Length(3), // Source selector
            Constraint::Length(3), // Access level selector
            Constraint::Length(2), // Help
            Constraint::Min(0),   // Spacer
        ])
        .split(inner);

    // Field 0: Endpoint (credential)
    let ep_active = state.field_index == 0;
    let ep_border = if ep_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let ep_label = state.selected_endpoint_name();
    let ep_display = format!("◀ {ep_label} ▶");
    let ep_block = Block::default()
        .borders(Borders::ALL)
        .border_style(ep_border)
        .title("Credential");
    let ep_style = if ep_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let ep_hint = format!("{}/{} — ←→ to cycle", state.selected_endpoint + 1, state.endpoints.len());
    let ep_content = if ep_active {
        vec![
            Line::from(Span::styled(ep_display, ep_style)),
            Line::from(Span::styled(ep_hint, Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![Line::from(Span::styled(ep_display, ep_style))]
    };
    let ep_para = Paragraph::new(ep_content).block(ep_block);
    frame.render_widget(ep_para, chunks[0]);

    // Field 1: Source
    let source_active = state.field_index == 1;
    let source_border = if source_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let source_label = state.sources.get(state.selected_source)
        .map_or_else(|| "(none)".to_string(), crate::tui::state::PermissionSource::label);
    let source_display = format!("◀ {source_label} ▶");
    let source_block = Block::default()
        .borders(Borders::ALL)
        .border_style(source_border)
        .title("Source");
    let source_style = if source_active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::Gray)
    };
    let source_hint = format!("{}/{} — ←→ to cycle", state.selected_source + 1, state.sources.len());
    let source_content = if source_active {
        vec![
            Line::from(Span::styled(source_display, source_style)),
            Line::from(Span::styled(source_hint, Style::default().fg(Color::DarkGray))),
        ]
    } else {
        vec![Line::from(Span::styled(source_display, source_style))]
    };
    let source_para = Paragraph::new(source_content).block(source_block);
    frame.render_widget(source_para, chunks[1]);

    // Field 2: Access Level
    let access_active = state.field_index == 2;
    let access_border = if access_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let access_color = state.access.color();
    let access_display = format!("◀ {} ▶", state.access.label());
    let access_block = Block::default()
        .borders(Borders::ALL)
        .border_style(access_border)
        .title("Access Level");
    let access_style = if access_active {
        Style::default().fg(access_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(access_color)
    };
    let access_para = Paragraph::new(Span::styled(access_display, access_style)).block(access_block);
    frame.render_widget(access_para, chunks[2]);

    // Help
    let help = Paragraph::new(Span::styled(
        "↑↓:Field │ ←→:Cycle │ Enter/Ctrl+S:Save │ Esc:Cancel",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(help, chunks[3]);
}

/// Render read-only view of a permission rule
pub fn render_view_permission_modal(
    frame: &mut Frame,
    area: Rect,
    permission_index: usize,
    permissions: &[PermissionRule],
) {
    let modal_area = centered_rect(55, 40, area);
    frame.render_widget(Clear, modal_area);

    let Some(rule) = permissions.get(permission_index) else {
        return;
    };

    let title = format!("Permission Rule #{}", permission_index + 1);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    let access_color = rule.access.color();

    let content = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Credential:  ", Style::default().fg(Color::Cyan)),
            Span::styled(&rule.endpoint_name, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Endpoint ID: ", Style::default().fg(Color::Cyan)),
            Span::styled(&rule.endpoint_id, Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Source:      ", Style::default().fg(Color::Cyan)),
            Span::styled(rule.source.label(), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Access:      ", Style::default().fg(Color::Cyan)),
            Span::styled(rule.access.label(), Style::default().fg(access_color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("  Priority:    ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("#{} of {}", permission_index + 1, permissions.len()),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Rules evaluate top-to-bottom. First match wins.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  Implicit DENY ALL follows all explicit rules.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  e:Edit │ +/-:Reorder │ d:Delete │ Esc:Close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, inner);
}
