//! Credentials/Vault management component
//!
//! Content fills the full area (tab cards are in the top slot bar).
//! Each tab shows a vertical list navigated with Up/Down. Enter on
//! a list item opens a read-only view modal.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};

use crate::effects::{palette, EffectState};
use crate::state::AppState;

/// Render the credentials page — content fills the full area (tabs are in top slot bar)
pub fn render_credentials(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    selected_tab: usize,
    _effect_state: &EffectState,
) {
    // Vault not ready — show compact hint (errors shown in status strip)
    if !state.vault.initialized {
        render_vault_hint(frame, area, "Vault not initialized", "i: initialize");
        return;
    }
    if state.vault.locked {
        render_vault_hint(frame, area, "Vault locked", "u: unlock");
        return;
    }

    match selected_tab {
        0 => render_endpoints_list(frame, area, state),
        1 => render_providers_list(frame, area, state),
        2 => render_permissions_list(frame, area, state),
        3 => render_audit_list(frame, area),
        _ => {}
    }
}

/// Render a compact vault status hint centered in the content area
pub(crate) fn render_vault_hint(frame: &mut Frame, area: Rect, message: &str, action: &str) {
    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            message,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("[ {action} ]"),
            Style::default().fg(Color::Green),
        )),
    ];

    let paragraph = Paragraph::new(content).alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
}

/// Render the Endpoints tab as a selectable vertical list
pub(crate) fn render_endpoints_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Endpoints (Enter:View  a:Add  d:Delete)");

    if state.endpoints.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No endpoints configured",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'a' to add a credential endpoint",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    let items: Vec<ListItem> = state
        .endpoints
        .iter()
        .map(|ep| {
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
                Span::styled(
                    format!("  {}", ep.endpoint_type),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("  {url_short}"),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let highlight_style = Style::default()
        .bg(palette::ACTIVE_PRIMARY)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let list = List::new(items)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.endpoints.is_empty() {
        list_state.select(Some(
            state
                .selected_endpoint
                .min(state.endpoints.len().saturating_sub(1)),
        ));
    }

    frame.render_stateful_widget(list, body, &mut list_state);

    // Scrollbar for endpoint list
    if state.endpoints.len() > body.height as usize {
        let mut sb_state =
            ScrollbarState::new(state.endpoints.len()).position(state.selected_endpoint);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            body,
            &mut sb_state,
        );
    }
}

/// Render the Providers tab as a selectable vertical list
pub(crate) fn render_providers_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Providers (Enter:View  e:Configure)");

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
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    let items: Vec<ListItem> = state
        .credential_schemas
        .iter()
        .map(|schema| {
            let configured = state.endpoints.iter().any(|e| {
                e.id.to_lowercase().contains(&schema.provider_id)
                    || e.name.to_lowercase().contains(&schema.provider_id)
            });
            let (indicator, ind_color) = if configured {
                ("●", Color::Green)
            } else {
                ("○", Color::Yellow)
            };

            let cat_icon = match schema.category.as_str() {
                "model" => "LLM ",
                "communication" => "MSG ",
                "tool" => "TL  ",
                _ => "",
            };

            ListItem::new(Line::from(vec![
                Span::raw(cat_icon),
                Span::styled(format!("{indicator} "), Style::default().fg(ind_color)),
                Span::styled(
                    schema.provider_name.as_str(),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!(" ({})", schema.provider_id),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let highlight_style = Style::default()
        .bg(palette::ACTIVE_PRIMARY)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let list = List::new(items)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.credential_schemas.is_empty() {
        list_state.select(Some(
            state
                .selected_provider_index
                .min(state.credential_schemas.len().saturating_sub(1)),
        ));
    }

    frame.render_stateful_widget(list, body, &mut list_state);
}

pub(crate) fn render_permissions_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Permissions (Enter:View  p:Add Rule)");

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
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    let mut items: Vec<ListItem> = state
        .permissions
        .iter()
        .enumerate()
        .map(|(i, rule)| {
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
        })
        .collect();

    // Implicit deny-all rule (not editable)
    items.push(ListItem::new(Line::from(vec![
        Span::styled(
            format!("#{:<2} ", state.permissions.len() + 1),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled("[DENY] ", Style::default().fg(Color::Red)),
        Span::styled(
            "* (implicit)",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        Span::styled(" → ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "All Sources",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
    ])));

    let highlight_style = Style::default()
        .bg(palette::ACTIVE_PRIMARY)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let list = List::new(items)
        .highlight_style(highlight_style)
        .highlight_symbol("▶ ");

    let mut list_state = ListState::default();
    if !state.permissions.is_empty() {
        list_state.select(Some(
            state
                .selected_permission
                .min(state.permissions.len().saturating_sub(1)),
        ));
    }

    frame.render_stateful_widget(list, body, &mut list_state);
}

pub(crate) fn render_audit_list(frame: &mut Frame, area: Rect) {
    let body = super::render_detail_header(frame, area, "Audit Log");

    let content = vec![
        Line::from(""),
        Line::from("Audit log tracks all credential access."),
        Line::from(""),
        Line::from(Span::styled(
            "Press 'v' to verify integrity",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, body);
}
