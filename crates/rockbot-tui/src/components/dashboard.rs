//! Dashboard component - detail panel (card bar is now in the top slot bar)

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Row, Table, Wrap},
    Frame,
};

use super::render_spinner;
use crate::effects::EffectState;
use crate::state::{AgentStatus, AppState};

/// Client (TUI) version — set at compile time from Cargo.toml
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render the dashboard page — detail panel fills the full area
pub fn render_dashboard(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    render_detail_panel(frame, area, state);
}

/// Render the detail panel based on which dashboard card is selected
fn render_detail_panel(frame: &mut Frame, area: Rect, state: &AppState) {
    let active_card = state.slot_bar.active_slot.saturating_sub(1);
    match active_card {
        0 => render_gateway_detail(frame, area, state),
        1 => render_client_detail(frame, area, state),
        2 => render_agents_detail(frame, area, state),
        3 => render_vault_detail(frame, area, state),
        _ => render_gateway_detail(frame, area, state),
    }
}

fn render_client_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    use ratatui::widgets::Sparkline;

    let body = super::render_detail_header(frame, area, "WS Connection");
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Fill(1)])
        .split(body);

    if !state.ws_latency_history.is_empty() {
        let data: Vec<u64> = state.ws_latency_history.iter().copied().collect();
        let sparkline =
            Sparkline::default()
                .data(&data)
                .style(Style::default().fg(if state.ws_connected {
                    Color::Green
                } else {
                    Color::Yellow
                }));
        let spark_block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::BOTTOM)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " RTT ms ",
                Style::default().fg(Color::DarkGray),
            ));
        let spark_inner = spark_block.inner(chunks[0]);
        frame.render_widget(spark_block, chunks[0]);
        frame.render_widget(sparkline, spark_inner);
    }

    let mut content = vec![
        Line::from(vec![
            Span::styled("WS: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                if state.ws_connected {
                    "Connected"
                } else {
                    "Disconnected"
                },
                Style::default().fg(if state.ws_connected {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
        ]),
        Line::from(vec![
            Span::styled("RTT: ", Style::default().fg(Color::Cyan)),
            Span::raw(
                state
                    .ws_last_rtt_ms
                    .map(|ms| format!("{ms} ms"))
                    .unwrap_or_else(|| "--".to_string()),
            ),
        ]),
        Line::from(vec![
            Span::styled("Server Conns: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.gateway.active_connections)),
        ]),
        Line::from(vec![
            Span::styled("Server Sessions: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.gateway.active_sessions)),
        ]),
        Line::from(vec![
            Span::styled("Reconnects: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.ws_reconnect_count)),
            Span::styled("  Disconnects: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.ws_disconnect_count)),
        ]),
    ];

    if let Some(reason) = &state.ws_last_disconnect_reason {
        content.push(Line::from(""));
        content.push(Line::from(vec![
            Span::styled("Last Disconnect: ", Style::default().fg(Color::Cyan)),
            Span::styled(reason.as_str(), Style::default().fg(Color::DarkGray)),
        ]));
    }

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(
        paragraph,
        if state.ws_latency_history.is_empty() {
            body
        } else {
            chunks[1]
        },
    );
}

fn render_gateway_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Gateway");

    if state.gateway_loading {
        render_spinner(frame, body, "Checking gateway...", state.tick_count);
        return;
    }

    let (status, color) = if state.gateway.connected {
        ("● Running", Color::Green)
    } else {
        ("○ Stopped", Color::Red)
    };

    let gw_ver = state.gateway.version.as_deref().unwrap_or("-");
    let version_match = state.gateway.version.as_deref() == Some(CLIENT_VERSION);
    let gw_ver_style = if version_match || !state.gateway.connected {
        Style::default()
    } else {
        Style::default().fg(Color::Yellow)
    };

    let mut content = vec![
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(status, Style::default().fg(color)),
        ]),
        Line::from(vec![
            Span::styled("Gateway: ", Style::default().fg(Color::Cyan)),
            Span::styled(format!("v{gw_ver}"), gw_ver_style),
            if !version_match && state.gateway.connected {
                Span::styled(" (mismatch!)", Style::default().fg(Color::Yellow))
            } else {
                Span::raw("")
            },
        ]),
        Line::from(vec![
            Span::styled("Client:  ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("v{CLIENT_VERSION}")),
        ]),
        Line::from(vec![
            Span::styled("Endpoint: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.gateway_url.as_str()),
        ]),
    ];

    if let Some(ref err) = state.gateway_error {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            format!("Error: {err}"),
            Style::default().fg(Color::Red),
        )));
    }

    // Show provider summary
    if !state.providers.is_empty() {
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "Providers:",
            Style::default().fg(Color::Cyan),
        )));
        for p in &state.providers {
            let (ind, ind_color) = if p.available {
                ("●", Color::Green)
            } else {
                ("○", Color::Yellow)
            };
            content.push(Line::from(vec![
                Span::styled(format!("  {ind} "), Style::default().fg(ind_color)),
                Span::raw(&p.name),
                Span::styled(
                    format!(" ({} models)", p.models.len()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "[s]tart  [S]top  [r]estart",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, body);
}

fn render_agents_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Agents Overview");

    if state.agents_loading {
        render_spinner(frame, body, "Loading agents...", state.tick_count);
        return;
    }

    if state.agents.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No agents configured",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Go to Agents tab (3) to add agents",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    let header = Row::new(vec!["Agent ID", "Model", "Sessions", "Status"])
        .style(Style::default().fg(Color::Cyan))
        .bottom_margin(1);

    let rows: Vec<Row> = state
        .agents
        .iter()
        .map(|agent| {
            let status_style = match agent.status {
                AgentStatus::Active => Style::default().fg(Color::Green),
                AgentStatus::Pending => Style::default().fg(Color::Yellow),
                AgentStatus::Error => Style::default().fg(Color::Red),
                AgentStatus::Disabled => Style::default().fg(Color::DarkGray),
            };

            Row::new(vec![
                agent.id.clone(),
                agent.model.clone().unwrap_or_else(|| "-".to_string()),
                format!("{}", agent.session_count),
                agent.status.label().to_string(),
            ])
            .style(status_style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(30),
        Constraint::Percentage(35),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
    ];

    let table = Table::new(rows, widths).header(header);

    frame.render_widget(table, body);
}

fn render_vault_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Vault");

    if state.vault_loading {
        render_spinner(frame, body, "Checking vault...", state.tick_count);
        return;
    }

    let mut content = vec![];

    if !state.vault.initialized {
        content.push(Line::from(Span::styled(
            "Vault not initialized",
            Style::default().fg(Color::Yellow),
        )));
        content.push(Line::from(""));
        content.push(Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]));
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "Press 'i' to initialize",
            Style::default().fg(Color::Green),
        )));
    } else {
        let (lock_text, lock_color) = if state.vault.locked {
            ("Locked", Color::Yellow)
        } else {
            ("Unlocked", Color::Green)
        };

        content.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::Cyan)),
            Span::styled(lock_text, Style::default().fg(lock_color)),
        ]));
        content.push(Line::from(vec![
            Span::styled("Endpoints: ", Style::default().fg(Color::Cyan)),
            Span::raw(format!("{}", state.vault.endpoint_count)),
        ]));
        content.push(Line::from(vec![
            Span::styled("Path: ", Style::default().fg(Color::Cyan)),
            Span::raw(state.vault_path.display().to_string()),
        ]));
        content.push(Line::from(""));

        if state.vault.locked {
            content.push(Line::from(Span::styled(
                "Press 'u' to unlock",
                Style::default().fg(Color::Green),
            )));
        } else {
            content.push(Line::from(Span::styled(
                "Press 'l' to lock",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, body);
}
