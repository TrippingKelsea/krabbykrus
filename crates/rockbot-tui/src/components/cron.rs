//! Cron jobs component - detail panel (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Row, Table, Wrap},
    Frame,
};

use super::render_spinner;
use crate::effects::EffectState;
use crate::state::AppState;

/// Render cron jobs page — detail fills the full area (cards are in top slot bar)
pub fn render_cron_jobs(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    _effect_state: &EffectState,
) {
    render_cron_detail(frame, area, state);
}

pub(crate) fn render_cron_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Cron Jobs");

    if state.cron_loading {
        render_spinner(frame, body, "Loading cron jobs...", state.tick_count);
        return;
    }

    if state.cron_jobs.is_empty() {
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No cron jobs configured",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Use the API to create cron jobs:",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(Span::styled(
                "  POST /api/cron/jobs",
                Style::default().fg(Color::Cyan),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    // Filter based on selected card
    let jobs: Vec<_> = match state.selected_cron_card {
        1 => state.cron_jobs.iter().filter(|j| j.enabled).collect(),
        2 => state.cron_jobs.iter().filter(|j| !j.enabled).collect(),
        _ => state.cron_jobs.iter().collect(),
    };

    if jobs.is_empty() {
        let label = if state.selected_cron_card == 1 {
            "active"
        } else {
            "disabled"
        };
        let content = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("No {label} cron jobs"),
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center);
        frame.render_widget(content, body);
        return;
    }

    let header = Row::new(vec!["Name", "Agent", "Schedule", "Status", "Last Run"])
        .style(Style::default().fg(Color::Cyan))
        .bottom_margin(1);

    let rows: Vec<Row> = jobs
        .iter()
        .enumerate()
        .map(|(i, job)| {
            let status_style = if job.enabled {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let status_label = if job.enabled { "Active" } else { "Disabled" };
            let last_status = job.last_status.as_deref().unwrap_or("-");
            let last_run = job.last_run.as_deref().unwrap_or("Never");

            let row_style = if i == state.selected_cron_job {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            Row::new(vec![
                job.name.clone(),
                job.agent_id.clone().unwrap_or_else(|| "-".to_string()),
                job.schedule.clone(),
                format!("{status_label} ({last_status})"),
                last_run.to_string(),
            ])
            .style(status_style)
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(22),
        Constraint::Percentage(18),
        Constraint::Percentage(25),
        Constraint::Percentage(18),
        Constraint::Percentage(17),
    ];

    let mut footer_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "[e]nable/disable  [d]elete  [t]rigger now  [r]efresh",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    if let Some(job) = jobs.get(state.selected_cron_job) {
        if let Some(ref next) = job.next_run {
            footer_lines.push(Line::from(vec![
                Span::styled("Next run: ", Style::default().fg(Color::Cyan)),
                Span::raw(next.as_str()),
            ]));
        }
    }

    // Split body area into table + footer
    let detail_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(footer_lines.len() as u16 + 1),
        ])
        .split(body);

    let table = Table::new(rows, widths).header(header);
    frame.render_widget(table, detail_chunks[0]);

    let footer = Paragraph::new(footer_lines).wrap(Wrap { trim: false });
    frame.render_widget(footer, detail_chunks[1]);
}
