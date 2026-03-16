//! Compact card-sized widgets for the slotted card bar.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Paragraph, Sparkline},
    Frame,
};

use crate::tui::state::{AgentStatus, AppState, CardWidgetId};

/// Render a compact widget inside a card's inner area (~12w x 1h).
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
    }
}

fn render_gateway_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let (text, color) = if state.gateway.connected {
        ("● Online", Color::Green)
    } else {
        ("○ Offline", Color::Red)
    };
    let p = Paragraph::new(Span::styled(text, Style::default().fg(color)));
    frame.render_widget(p, area);
}

fn render_gateway_load(frame: &mut Frame, area: Rect, state: &AppState) {
    if state.gateway_load_history.is_empty() {
        let p = Paragraph::new(Span::styled("--", Style::default().fg(Color::DarkGray)));
        frame.render_widget(p, area);
        return;
    }
    let data: Vec<u64> = state.gateway_load_history.iter().copied().collect();
    let sparkline = Sparkline::default()
        .data(&data)
        .style(Style::default().fg(Color::Cyan));
    frame.render_widget(sparkline, area);
}

fn render_gateway_network(frame: &mut Frame, area: Rect) {
    let p = Paragraph::new(Span::styled(
        "↑0 ↓0",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(p, area);
}

fn render_client_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let (text, color) = if state.gateway.connected {
        ("● Connected", Color::Green)
    } else {
        ("○ Disconnected", Color::Yellow)
    };
    let p = Paragraph::new(Span::styled(text, Style::default().fg(color)));
    frame.render_widget(p, area);
}

fn render_client_messages(frame: &mut Frame, area: Rect, state: &AppState) {
    let total: usize = state.sessions.iter().map(|s| s.message_count).sum();
    let p = Paragraph::new(Span::styled(
        format!("{total} msgs"),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(p, area);
}

fn render_client_resources(frame: &mut Frame, area: Rect) {
    let p = Paragraph::new(Span::styled(
        "mem: --",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(p, area);
}

fn render_agent_overview(frame: &mut Frame, area: Rect, state: &AppState) {
    let active = state
        .agents
        .iter()
        .filter(|a| a.status == AgentStatus::Active)
        .count();
    let total = state.agents.len();
    let p = Paragraph::new(Span::styled(
        format!("{active}/{total}"),
        Style::default().fg(Color::Green),
    ));
    frame.render_widget(p, area);
}

fn render_agent_sessions(frame: &mut Frame, area: Rect, state: &AppState) {
    let p = Paragraph::new(Span::styled(
        format!("{} active", state.sessions.len()),
        Style::default().fg(Color::Cyan),
    ));
    frame.render_widget(p, area);
}

fn render_agent_tools(frame: &mut Frame, area: Rect) {
    let p = Paragraph::new(Span::styled(
        "0 calls",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(p, area);
}
