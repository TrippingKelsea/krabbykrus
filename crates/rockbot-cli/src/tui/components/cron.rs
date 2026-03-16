//! Cron jobs component - card strip + job list/detail

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table, Wrap},
    Frame,
};

use super::render_spinner;
use crate::tui::effects::{self, palette, EffectState};
use crate::tui::state::AppState;

const CARD_WIDTH: u16 = 16;

/// Render cron jobs page — summary cards in cards_area, job list in detail_area
pub fn render_cron_jobs(
    frame: &mut Frame,
    cards_area: Rect,
    detail_area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    render_cron_cards(frame, cards_area, state, effect_state);
    render_cron_detail(frame, detail_area, state);
}

fn render_cron_cards(frame: &mut Frame, area: Rect, state: &AppState, effect_state: &EffectState) {
    let cards = [("All Jobs", 0usize), ("Active", 1), ("Disabled", 2)];

    let mut constraints: Vec<Constraint> = cards
        .iter()
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Min(0));

    let card_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    let elapsed = effect_state.elapsed_secs();

    let total = state.cron_jobs.len();
    let active = state.cron_jobs.iter().filter(|j| j.enabled).count();
    let disabled = total - active;

    for &(label, idx) in &cards {
        let is_selected = idx == state.selected_cron_card;

        let border_style = if is_selected {
            effects::active_border_style(elapsed)
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
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let (count, count_color) = match idx {
            0 => (total, Color::Cyan),
            1 => (active, Color::Green),
            2 => (disabled, Color::DarkGray),
            _ => (0, Color::DarkGray),
        };

        let lines = vec![
            Line::from(Span::styled(label, label_style)),
            Line::from(Span::styled(
                format!("{count}"),
                Style::default().fg(count_color),
            )),
            Line::from(Span::styled("jobs", Style::default().fg(Color::DarkGray))),
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

fn render_cron_detail(frame: &mut Frame, area: Rect, state: &AppState) {
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
            Constraint::Min(0),
            Constraint::Length(footer_lines.len() as u16 + 1),
        ])
        .split(body);

    let table = Table::new(rows, widths).header(header);
    frame.render_widget(table, detail_chunks[0]);

    let footer = Paragraph::new(footer_lines).wrap(Wrap { trim: false });
    frame.render_widget(footer, detail_chunks[1]);
}
