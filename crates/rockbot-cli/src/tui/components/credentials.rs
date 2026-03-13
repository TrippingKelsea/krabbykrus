//! Credentials/Vault management component
//!
//! Tab cards on top (Endpoints, Providers, Permissions, Audit), navigated with
//! Left/Right. Each tab shows a vertical list navigated with Up/Down. Enter on
//! a list item opens a read-only view modal.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;

/// Card dimensions for tab strip
const CARD_WIDTH: u16 = 18;
const CARD_HEIGHT: u16 = 5;

/// Tab labels and sub-headings
const TABS: &[(&str, &str, &str)] = &[
    ("Endpoints", "Stored", "Credentials"),
    ("Providers", "Available", "Schemas"),
    ("Permissions", "Access", "Rules"),
    ("Audit", "Activity", "Log"),
];

/// Render the credentials page — tab card strip on top, list content below
pub fn render_credentials(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    selected_tab: usize,
    effect_state: &EffectState,
) {
    // Vault not ready — show init/unlock screens
    if !state.vault.initialized {
        render_vault_init(frame, area, state);
        return;
    }
    if state.vault.locked {
        render_vault_locked(frame, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(CARD_HEIGHT), Constraint::Min(0)])
        .split(area);

    render_tab_cards(frame, chunks[0], state, selected_tab, effect_state);

    match selected_tab {
        0 => render_endpoints_list(frame, chunks[1], state),
        1 => render_providers_list(frame, chunks[1], state),
        2 => render_permissions_list(frame, chunks[1], state),
        3 => render_audit_list(frame, chunks[1]),
        _ => {}
    }
}

fn render_tab_cards(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    selected_tab: usize,
    effect_state: &EffectState,
) {
    let mut constraints: Vec<Constraint> = TABS.iter()
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Min(0));

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    for (idx, &(label, sub1, sub2)) in TABS.iter().enumerate() {
        let is_selected = idx == selected_tab;

        let border_style = if is_selected && !state.sidebar_focus {
            effects::active_border_style(elapsed)
        } else if is_selected {
            Style::default().fg(palette::ACTIVE_PRIMARY)
        } else {
            Style::default().fg(palette::INACTIVE_BORDER)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(card_chunks[idx]);
        frame.render_widget(block, card_chunks[idx]);

        if inner.height < 3 || inner.width < 2 {
            continue;
        }

        let label_style = if is_selected {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        // Count for each tab
        let count_text = match idx {
            0 => format!("{}", state.endpoints.len()),
            1 => format!("{}", state.credential_schemas.len()),
            _ => String::new(),
        };

        let lines = vec![
            Line::from(Span::styled(label, label_style)),
            Line::from(Span::styled(sub1, Style::default().fg(Color::Cyan))),
            Line::from(Span::styled(
                if count_text.is_empty() { sub2.to_string() } else { format!("{count_text} {sub2}") },
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines).alignment(Alignment::Center);
        let render_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: inner.height.min(3),
        };
        frame.render_widget(paragraph, render_area);
    }
}

fn render_vault_init(frame: &mut Frame, area: Rect, state: &AppState) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Vault not initialized",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Vault Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]),
        Line::from(""),
        Line::from("The credential vault needs to be initialized before"),
        Line::from("you can store API keys and secrets."),
        Line::from(""),
        Line::from(Span::styled(
            "Press 'i' to initialize with password",
            Style::default().fg(Color::Green),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Initialize Vault");

    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

fn render_vault_locked(frame: &mut Frame, area: Rect) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled("Vault Locked", Style::default().fg(Color::Yellow))),
        Line::from(""),
        Line::from("Enter your password to unlock the vault."),
        Line::from(""),
        Line::from(Span::styled("Press 'u' to unlock", Style::default().fg(Color::Green))),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Unlock Vault");

    let paragraph = Paragraph::new(content)
        .block(block)
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}

/// Render the Endpoints tab as a selectable vertical list
fn render_endpoints_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Endpoints (Enter:View  a:Add  d:Delete)");

    if state.endpoints.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("No endpoints configured", Style::default().fg(Color::DarkGray))),
            Line::from(""),
            Line::from(Span::styled("Press 'a' to add a credential endpoint", Style::default().fg(Color::DarkGray))),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, area);
        return;
    }

    let items: Vec<ListItem> = state.endpoints.iter().map(|ep| {
        let (icon, icon_color) = if ep.has_credential {
            ("●", Color::Green)
        } else {
            ("○", Color::Yellow)
        };

        let url_short = if ep.base_url.is_empty() {
            ep.id.chars().take(20).collect::<String>()
        } else {
            ep.base_url.replace("https://", "").replace("http://", "")
        };

        ListItem::new(Line::from(vec![
            Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
            Span::styled(&ep.name, Style::default().fg(Color::White)),
            Span::styled(format!("  {}", ep.endpoint_type), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  {url_short}"), Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();

    let highlight_style = if !state.sidebar_focus {
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
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.endpoints.is_empty() {
        list_state.select(Some(state.selected_endpoint.min(state.endpoints.len().saturating_sub(1))));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Render the Providers tab as a selectable vertical list
fn render_providers_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Providers (Enter:View  e:Configure)");

    if state.credential_schemas.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Start the gateway to see providers",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Run: rockbot gateway start",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, area);
        return;
    }

    let items: Vec<ListItem> = state.credential_schemas.iter().map(|schema| {
        let configured = state.endpoints.iter().any(|e| {
            e.id.to_lowercase().contains(&schema.provider_id) ||
            e.name.to_lowercase().contains(&schema.provider_id)
        });
        let (indicator, ind_color) = if configured { ("●", Color::Green) } else { ("○", Color::Yellow) };

        let cat_icon = match schema.category.as_str() {
            "model" => "LLM ",
            "communication" => "MSG ",
            "tool" => "TL  ",
            _ => "",
        };

        ListItem::new(Line::from(vec![
            Span::raw(cat_icon),
            Span::styled(format!("{indicator} "), Style::default().fg(ind_color)),
            Span::styled(schema.provider_name.as_str(), Style::default().fg(Color::White)),
            Span::styled(format!(" ({})", schema.provider_id), Style::default().fg(Color::DarkGray)),
        ]))
    }).collect();

    let highlight_style = if !state.sidebar_focus {
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
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.credential_schemas.is_empty() {
        list_state.select(Some(state.selected_provider_index.min(state.credential_schemas.len().saturating_sub(1))));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_permissions_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Permissions (Enter:View  p:Add Rule)");

    if state.permissions.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No permission rules configured",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from("Default permissions are created automatically"),
            Line::from("when a credential is added."),
            Line::from(""),
            Line::from(Span::styled(
                "Implicit DENY ALL is active for uncovered credentials.",
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'p' to add a permission rule",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(block)
        .alignment(Alignment::Center);
        frame.render_widget(content, area);
        return;
    }

    let mut items: Vec<ListItem> = state.permissions.iter().enumerate().map(|(i, rule)| {
        let access_color = rule.access.color();

        ListItem::new(Line::from(vec![
            Span::styled(
                format!("#{:<2} ", i + 1),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("[{}] ", rule.access.short_label()),
                Style::default().fg(access_color),
            ),
            Span::styled(&rule.endpoint_name, Style::default().fg(Color::White)),
            Span::styled(" → ", Style::default().fg(Color::DarkGray)),
            Span::styled(rule.source.label(), Style::default().fg(Color::Cyan)),
        ]))
    }).collect();

    // Implicit deny-all rule (not editable)
    items.push(ListItem::new(Line::from(vec![
        Span::styled(
            format!("#{:<2} ", state.permissions.len() + 1),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("[DENY] ", Style::default().fg(Color::Red)),
        Span::styled("* (implicit)", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        Span::styled(" → ", Style::default().fg(Color::DarkGray)),
        Span::styled("All Sources", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
    ])));

    let highlight_style = if !state.sidebar_focus {
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
        .block(block)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.permissions.is_empty() {
        list_state.select(Some(state.selected_permission.min(state.permissions.len().saturating_sub(1))));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_audit_list(frame: &mut Frame, area: Rect) {
    let content = vec![
        Line::from(""),
        Line::from("Audit log tracks all credential access."),
        Line::from(""),
        Line::from(Span::styled(
            "Press 'v' to verify integrity",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE_BORDER))
        .title("Audit Log");

    let paragraph = Paragraph::new(content).block(block);
    frame.render_widget(paragraph, area);
}
