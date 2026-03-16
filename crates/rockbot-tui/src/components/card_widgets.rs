//! Compact card-sized widgets for the slotted card bar.
//!
//! Each widget renders into a ~12w x 3h inner area (CARD_HEIGHT=5 minus borders).
//! Layout: line 0 = label/title, line 1 = primary value, line 2 = detail.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Sparkline},
    Frame,
};

use crate::state::{AgentStatus, AppState, CardWidgetId};

/// Render a compact widget inside a card's inner area (~12w x 3h).
pub fn render_card_widget(id: &CardWidgetId, frame: &mut Frame, area: Rect, state: &AppState) {
    match id {
        CardWidgetId::GatewayStatus => render_gateway_status(frame, area, state),
        CardWidgetId::GatewayLoad => render_gateway_load(frame, area, state),
        CardWidgetId::GatewayNetwork => render_gateway_network(frame, area),
        CardWidgetId::ClientStatus => render_client_status(frame, area, state),
        CardWidgetId::ClientMessages => render_client_messages(frame, area, state),
        CardWidgetId::ClientResources => render_client_resources(frame, area),
        CardWidgetId::AgentOverview => render_agent_overview(frame, area, state),
        CardWidgetId::AgentSessions => render_agent_sessions(frame, area, state),
        CardWidgetId::AgentTools => render_agent_tools(frame, area),
        CardWidgetId::VaultStatus => render_vault_status(frame, area, state),
        CardWidgetId::CronOverview => render_cron_overview(frame, area, state),
        CardWidgetId::ModelsOverview => render_models_overview(frame, area, state),
        CardWidgetId::SettingsGeneral => render_settings_general(frame, area),
    }
}

/// Helper: split a card inner area into up to 3 rows.
fn card_rows(area: Rect) -> [Rect; 3] {
    let h = area.height;
    let row = |i: u16| Rect {
        x: area.x,
        y: area.y + i.min(h.saturating_sub(1)),
        width: area.width,
        height: if i < h { 1 } else { 0 },
    };
    [row(0), row(1), row(2)]
}

fn render_gateway_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Gateway",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let (text, color) = if state.gateway.connected {
        ("● Online", Color::Green)
    } else {
        ("○ Offline", Color::Red)
    };
    let value = Paragraph::new(Span::styled(text, Style::default().fg(color)));
    frame.render_widget(value, rows[1]);

    let version = state
        .gateway
        .version
        .as_deref()
        .unwrap_or("--");
    let detail = Paragraph::new(Span::styled(
        version,
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_gateway_load(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Load",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    if state.gateway_load_history.is_empty() {
        let p = Paragraph::new(Span::styled("--", Style::default().fg(Color::DarkGray)));
        frame.render_widget(p, rows[1]);
        return;
    }
    let data: Vec<u64> = state.gateway_load_history.iter().copied().collect();
    let sparkline = Sparkline::default()
        .data(&data)
        .style(Style::default().fg(Color::Cyan));
    // Sparkline gets 2 rows (value + detail)
    let spark_area = Rect {
        x: rows[1].x,
        y: rows[1].y,
        width: rows[1].width,
        height: 2.min(area.height.saturating_sub(1)),
    };
    frame.render_widget(sparkline, spark_area);
}

fn render_gateway_network(frame: &mut Frame, area: Rect) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Network",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let value = Paragraph::new(Span::styled(
        "\u{2191}0 \u{2193}0",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(value, rows[1]);
}

fn render_client_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Client",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let (text, color) = if state.gateway.connected {
        ("● Connected", Color::Green)
    } else {
        ("○ Disconnected", Color::Yellow)
    };
    let value = Paragraph::new(Span::styled(text, Style::default().fg(color)));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled("WS", Style::default().fg(Color::DarkGray)));
    frame.render_widget(detail, rows[2]);
}

fn render_client_messages(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Messages",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let total: usize = state.sessions.iter().map(|s| s.message_count).sum();
    let value = Paragraph::new(Span::styled(
        format!("{total}"),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled(
        format!("{} sessions", state.sessions.len()),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_client_resources(frame: &mut Frame, area: Rect) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Resources",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let value = Paragraph::new(Span::styled(
        "mem: --",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(value, rows[1]);
}

fn render_agent_overview(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Agents",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let active = state
        .agents
        .iter()
        .filter(|a| a.status == AgentStatus::Active)
        .count();
    let total = state.agents.len();
    let value = Paragraph::new(Span::styled(
        format!("{active}/{total}"),
        Style::default().fg(Color::Green),
    ));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled(
        "active",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_agent_sessions(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Sessions",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let value = Paragraph::new(Span::styled(
        format!("{}", state.sessions.len()),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled(
        "active",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_agent_tools(frame: &mut Frame, area: Rect) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Tools",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let value = Paragraph::new(Span::styled(
        "0 calls",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(value, rows[1]);
}

fn render_vault_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Vault",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let (text, color) = if state.vault.initialized {
        if state.vault.locked {
            ("Locked", Color::Yellow)
        } else {
            ("Unlocked", Color::Green)
        }
    } else {
        ("Not init", Color::Red)
    };
    let value = Paragraph::new(Span::styled(text, Style::default().fg(color)));
    frame.render_widget(value, rows[1]);

    let count = state.endpoints.len();
    let detail = Paragraph::new(Span::styled(
        format!("{count} endpoints"),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_cron_overview(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Cron",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let total = state.cron_jobs.len();
    let enabled = state.cron_jobs.iter().filter(|j| j.enabled).count();
    let value = Paragraph::new(Span::styled(
        format!("{enabled}/{total}"),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled(
        "enabled",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_models_overview(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Models",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let count = state.providers.len();
    let value = Paragraph::new(Span::styled(
        format!("{count}"),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(value, rows[1]);

    let detail = Paragraph::new(Span::styled(
        "providers",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(detail, rows[2]);
}

fn render_settings_general(frame: &mut Frame, area: Rect) {
    let rows = card_rows(area);
    let label = Paragraph::new(Span::styled(
        "Settings",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ));
    frame.render_widget(label, rows[0]);

    let value = Paragraph::new(Span::styled(
        env!("CARGO_PKG_VERSION"),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(value, rows[1]);
}
