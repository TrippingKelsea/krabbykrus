//! Overlay renderers for vault, settings, models, and cron.
//!
//! Each overlay is a centered modal that delegates to existing component renderers.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Tabs},
    Frame,
};

use super::centered_rect;
use crate::app::CredentialsTab;
use crate::effects::EffectState;
use crate::state::AppState;

/// Render the vault/credentials overlay (Alt+V) — 90%x90% centered.
pub fn render_vault_overlay(
    frame: &mut Frame,
    full: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let area = centered_rect(90, 90, full);
    frame.render_widget(Clear, area);

    let tab = CredentialsTab::from_index(state.credentials_tab);
    let titles: Vec<Line<'_>> = CredentialsTab::all()
        .iter()
        .map(|t| {
            if *t == tab {
                Line::from(Span::styled(
                    t.label(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(
                    t.label(),
                    Style::default().fg(Color::DarkGray),
                ))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Vault ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Tab bar + body
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(inner);

    let tabs = Tabs::new(titles)
        .select(tab.index())
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("│");
    frame.render_widget(tabs, chunks[0]);

    // Vault not ready guard
    if !state.vault.initialized {
        super::credentials::render_vault_hint(
            frame,
            chunks[1],
            "Vault not initialized",
            "i: initialize",
        );
        return;
    }
    if state.vault.locked {
        super::credentials::render_vault_hint(frame, chunks[1], "Vault locked", "u: unlock");
        return;
    }

    match state.credentials_tab {
        0 => super::credentials::render_endpoints_list(frame, chunks[1], state),
        1 => super::credentials::render_providers_list(frame, chunks[1], state),
        2 => super::credentials::render_permissions_list(frame, chunks[1], state),
        3 => super::credentials::render_audit_list(frame, chunks[1]),
        _ => {}
    }
}

/// Render the settings overlay (Alt+S) — 80%x85% centered.
pub fn render_settings_overlay(
    frame: &mut Frame,
    full: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    let area = centered_rect(80, 85, full);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Settings ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    super::settings::render_settings_detail(frame, inner, state);
}

/// Render the models overlay (Alt+M) — 80%x85% centered.
pub fn render_models_overlay(
    frame: &mut Frame,
    full: Rect,
    state: &AppState,
    provider_index: usize,
    _scroll: usize,
    _effect_state: &EffectState,
) {
    let area = centered_rect(80, 85, full);
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Models ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.providers.is_empty() {
        super::models::render_no_providers(frame, inner);
        return;
    }

    // Dynamic tab bar from actual providers
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(inner);

    let titles: Vec<Line<'_>> = state
        .providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i == provider_index {
                Line::from(Span::styled(
                    &p.name,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(&p.name, Style::default().fg(Color::DarkGray)))
            }
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(provider_index)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("│");
    frame.render_widget(tabs, chunks[0]);

    // Render selected provider details — temporarily set selected_provider
    // We can't mutate state, so we render manually with the provider at index
    let idx = provider_index.min(state.providers.len().saturating_sub(1));
    render_provider_detail_at(frame, chunks[1], state, idx);
}

/// Render provider details for a specific index (used by models overlay).
fn render_provider_detail_at(frame: &mut Frame, area: Rect, state: &AppState, idx: usize) {
    use crate::effects::palette;
    use ratatui::widgets::Wrap;

    let Some(provider) = state.providers.get(idx) else {
        let paragraph = Paragraph::new("No provider selected");
        frame.render_widget(paragraph, area);
        return;
    };

    let status_color = if provider.available {
        palette::CONFIGURED
    } else {
        palette::UNCONFIGURED
    };
    let status_text = if provider.available {
        "Available"
    } else {
        "Not Available"
    };

    let mut content = vec![
        Line::from(Span::styled(
            &provider.name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("Provider ID: {}", provider.id),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]),
    ];

    // Capabilities
    content.push(Line::from(""));
    let cap_items = [
        ("Streaming", provider.supports_streaming),
        ("Tool Use", provider.supports_tools),
        ("Vision", provider.supports_vision),
    ];
    for (name, supported) in cap_items {
        let (icon, color) = if supported {
            ("\u{2713}", Color::Green)
        } else {
            ("\u{2717}", Color::DarkGray)
        };
        content.push(Line::from(vec![
            Span::styled(format!("  {icon} "), Style::default().fg(color)),
            Span::raw(name),
        ]));
    }

    // Models summary
    if !provider.models.is_empty() {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            format!(
                "{} models available — Enter to browse",
                provider.models.len()
            ),
            Style::default().fg(Color::DarkGray),
        )));
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[Enter] model list  [e] configure",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Render the cron jobs overlay (Alt+C) — 85%x85% centered.
pub fn render_cron_overlay(
    frame: &mut Frame,
    full: Rect,
    state: &AppState,
    _scroll: usize,
    _effect_state: &EffectState,
) {
    let area = centered_rect(85, 85, full);
    frame.render_widget(Clear, area);

    let filter_labels = ["All", "Active", "Disabled"];
    let titles: Vec<Line<'_>> = filter_labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            if i == state.selected_cron_card {
                Line::from(Span::styled(
                    *label,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(*label, Style::default().fg(Color::DarkGray)))
            }
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Cron Jobs ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Fill(1)])
        .split(inner);

    let tabs = Tabs::new(titles)
        .select(state.selected_cron_card)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("│");
    frame.render_widget(tabs, chunks[0]);

    super::cron::render_cron_detail(frame, chunks[1], state);
}
