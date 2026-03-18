//! Settings component - detail panel (card bar is in top slot bar)

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::effects::palette;
use crate::state::{AppState, FontRole, ThemeToken};
use rockbot_config::{AnimationStyle, ColorTheme, RgbaColor, TuiThemeConfig};

pub const SETTINGS_SECTION_LABELS: [&str; 5] = ["General", "Paths", "About", "Theme", "Fonts"];
pub const FONT_FAMILY_OPTIONS: [&str; 7] = [
    "terminal-default",
    "Iosevka",
    "JetBrains Mono",
    "Fira Code",
    "IBM Plex Mono",
    "SF Mono",
    "Monaspace Neon",
];

pub struct ThemeEditorLayout {
    pub controls: Rect,
    pub tokens: Rect,
    pub wheel: Rect,
    pub value_slider: Rect,
    pub alpha_slider: Rect,
    pub preview: Rect,
    pub hint: Rect,
}

pub struct TypographyLayout {
    pub roles: Rect,
    pub families: Rect,
    pub size: Rect,
    pub preview: Rect,
    pub hint: Rect,
}

pub fn render_settings(frame: &mut Frame, area: Rect, state: &AppState) {
    render_settings_detail(frame, area, state);
}

pub(crate) fn render_settings_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    match state.selected_settings_card {
        0 => render_general(frame, area, state),
        1 => render_paths(frame, area, state),
        2 => render_about(frame, area, state),
        3 => render_theme(frame, area, state),
        4 => render_typography(frame, area, state),
        _ => {}
    }
}

pub fn preset_cells(area: Rect) -> Vec<Rect> {
    equal_cells(area, ColorTheme::all().len())
}

pub fn animation_cells(area: Rect) -> Vec<Rect> {
    equal_cells(area, AnimationStyle::all().len())
}

pub fn family_cells(area: Rect) -> Vec<Rect> {
    equal_cells(area, FONT_FAMILY_OPTIONS.len())
}

pub fn theme_editor_layout(area: Rect) -> ThemeEditorLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Fill(1),
            Constraint::Length(2),
        ])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Min(30),
            Constraint::Length(30),
        ])
        .split(rows[1]);

    let picker = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(body[1]);

    ThemeEditorLayout {
        controls: rows[0],
        tokens: body[0],
        wheel: picker[0],
        value_slider: picker[1],
        alpha_slider: picker[2],
        preview: body[2],
        hint: rows[2],
    }
}

pub fn typography_layout(area: Rect) -> TypographyLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(2)])
        .split(area);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Fill(1)])
        .split(rows[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .split(body[1]);

    TypographyLayout {
        roles: body[0],
        families: right[0],
        size: right[1],
        preview: right[2],
        hint: rows[1],
    }
}

pub fn rgba_to_hsv(color: RgbaColor) -> (f32, f32, f32) {
    let r = f32::from(color.r) / 255.0;
    let g = f32::from(color.g) / 255.0;
    let b = f32::from(color.b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (max - r).abs() <= f32::EPSILON {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };

    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };

    (hue / 360.0, saturation.clamp(0.0, 1.0), max.clamp(0.0, 1.0))
}

pub fn hsv_to_rgba(hue: f32, saturation: f32, value: f32, alpha: u8) -> RgbaColor {
    let h = hue.rem_euclid(1.0) * 6.0;
    let s = saturation.clamp(0.0, 1.0);
    let v = value.clamp(0.0, 1.0);
    let c = v * s;
    let x = c * (1.0 - ((h.rem_euclid(2.0)) - 1.0).abs());
    let m = v - c;

    let (r1, g1, b1) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    RgbaColor {
        r: ((r1 + m) * 255.0).round() as u8,
        g: ((g1 + m) * 255.0).round() as u8,
        b: ((b1 + m) * 255.0).round() as u8,
        a: alpha,
    }
}

fn equal_cells(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, count as u32); count])
        .split(area)
        .iter()
        .copied()
        .collect()
}

fn render_general(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "General");
    let label = palette::accent_primary(&state.tui_config);
    let secondary = palette::text_secondary(&state.tui_config);

    let gateway_status = if state.gateway.connected {
        Span::styled("● Running", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ Stopped", Style::default().fg(Color::Red))
    };

    let content = vec![
        Line::from(vec![
            Span::styled("Gateway Status: ", Style::default().fg(label)),
            gateway_status,
        ]),
        Line::from(vec![
            Span::styled("Gateway Version: ", Style::default().fg(label)),
            Span::styled(
                state.gateway.version.as_deref().unwrap_or("-"),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Client Version: ", Style::default().fg(label)),
            Span::styled(
                env!("CARGO_PKG_VERSION"),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Active Sessions: ", Style::default().fg(label)),
            Span::styled(
                format!("{}", state.sessions.len()),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Configured Agents: ", Style::default().fg(label)),
            Span::styled(
                format!("{}", state.agents.len()),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("LLM Providers: ", Style::default().fg(label)),
            Span::styled(
                format!("{}", state.providers.len()),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "[s]tart/[S]top gateway  [r]estart",
            Style::default().fg(secondary),
        )),
    ];

    frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), body);
}

fn render_paths(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Paths");
    let label = palette::accent_primary(&state.tui_config);
    let secondary = palette::text_secondary(&state.tui_config);

    let content = vec![
        Line::from(vec![
            Span::styled("Config: ", Style::default().fg(label)),
            Span::styled(
                state.config_path.display().to_string(),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Vault: ", Style::default().fg(label)),
            Span::styled(
                state.vault_path.display().to_string(),
                Style::default().fg(palette::text_primary(&state.tui_config)),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Agent directories:",
            Style::default().fg(label),
        )),
        Line::from(Span::styled(
            "  ~/.config/rockbot/agents/{agent_id}/",
            Style::default().fg(secondary),
        )),
        Line::from(Span::styled(
            "  Each agent has SOUL.md and SYSTEM-PROMPT.md",
            Style::default().fg(secondary),
        )),
    ];

    frame.render_widget(Paragraph::new(content).wrap(Wrap { trim: false }), body);
}

fn render_about(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "About");

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "RockBot",
            Style::default()
                .fg(palette::accent_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "A Rust-native AI agent framework",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Self-hosted multi-channel AI gateway with",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(Span::styled(
            "pluggable LLM providers, credential management,",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(Span::styled(
            "and a TUI + Web interface.",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "https://github.com/TrippingKelsea/rockbot",
            Style::default().fg(palette::accent_secondary(&state.tui_config)),
        )),
    ];

    frame.render_widget(Paragraph::new(content).alignment(Alignment::Center), body);
}

fn render_theme(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Theme");
    let layout = theme_editor_layout(body);
    let theme = state.tui_config.resolved_theme();

    let control_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(3)])
        .split(layout.controls);

    render_choice_row(
        frame,
        control_rows[0],
        "Preset",
        &ColorTheme::all()
            .iter()
            .map(|theme| theme.label().to_string())
            .collect::<Vec<_>>(),
        state.selected_settings_field == 0,
        ColorTheme::all()
            .iter()
            .position(|item| *item == state.tui_config.color_theme)
            .unwrap_or(0),
        &preset_cells(control_rows[0]),
        |idx| palette::theme_primary(&ColorTheme::all()[idx]),
    );

    render_choice_row(
        frame,
        control_rows[1],
        "Motion",
        &AnimationStyle::all()
            .iter()
            .map(|style| style.label().to_string())
            .collect::<Vec<_>>(),
        state.selected_settings_field == 1,
        AnimationStyle::all()
            .iter()
            .position(|item| *item == state.tui_config.animation_style)
            .unwrap_or(0),
        &animation_cells(control_rows[1]),
        |_| palette::accent_secondary(&state.tui_config),
    );

    render_theme_token_list(frame, layout.tokens, state, &theme);
    render_color_wheel(frame, layout.wheel, state);
    render_slider(
        frame,
        layout.value_slider,
        "Value",
        state.settings_color_value,
        state.selected_settings_field == 5,
        |t| {
            hsv_to_rgba(
                state.settings_color_hue,
                state.settings_color_saturation,
                t,
                255,
            )
        },
    );
    render_slider(
        frame,
        layout.alpha_slider,
        "Alpha",
        f32::from(state.settings_color_alpha) / 255.0,
        state.selected_settings_field == 6,
        |t| {
            let mut color = hsv_to_rgba(
                state.settings_color_hue,
                state.settings_color_saturation,
                state.settings_color_value,
                (t * 255.0).round() as u8,
            );
            color.a = (t * 255.0).round() as u8;
            color
        },
    );
    render_theme_preview(frame, layout.preview, state, &theme);

    let status = state
        .settings_save_feedback
        .as_ref()
        .map_or((
            "Autosave on change",
            palette::text_secondary(&state.tui_config),
        ), |(msg, is_error)| {
            (
                msg.as_str(),
                if *is_error { Color::Red } else { Color::Green },
            )
        });

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "↑↓ field  [[]/[]] adjust  mouse: select token + drag wheel/sliders",
                Style::default().fg(palette::text_secondary(&state.tui_config)),
            )),
            Line::from(Span::styled(status.0, Style::default().fg(status.1))),
        ]),
        layout.hint,
    );
}

fn render_typography(frame: &mut Frame, area: Rect, state: &AppState) {
    let body = super::render_detail_header(frame, area, "Typography");
    let layout = typography_layout(body);
    let active_role = FontRole::all()[state.selected_font_role];

    render_font_role_list(frame, layout.roles, state);
    render_choice_row(
        frame,
        layout.families,
        "Stored Family",
        &FONT_FAMILY_OPTIONS
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>(),
        state.selected_font_field == 1,
        FONT_FAMILY_OPTIONS
            .iter()
            .position(|item| *item == active_role.family(&state.tui_config.fonts))
            .unwrap_or(0),
        &family_cells(layout.families),
        |_| palette::accent_secondary(&state.tui_config),
    );

    render_font_size_row(frame, layout.size, state, active_role);

    let preview_lines = vec![
        Line::from(Span::styled(
            format!("{} font stub", active_role.label()),
            Style::default()
                .fg(palette::text_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "family={}  size={}",
                active_role.family(&state.tui_config.fonts),
                active_role.size(&state.tui_config.fonts)
            ),
            Style::default().fg(palette::text_secondary(&state.tui_config)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "The terminal TUI persists these preferences, but the terminal emulator controls actual font rendering.",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "This is here so the Web UI and future richer clients can honor separate interface, user, AI, thinking, and tool typography.",
            Style::default().fg(palette::text_secondary(&state.tui_config)),
        )),
    ];

    let preview = Paragraph::new(preview_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(palette::border(&state.tui_config)))
                .style(Style::default().bg(palette::bg_secondary(&state.tui_config))),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(preview, layout.preview);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "↑↓ role/family/size  [[]/[]] adjust  mouse: choose family or size",
                Style::default().fg(palette::text_secondary(&state.tui_config)),
            )),
            Line::from(Span::styled(
                "Font settings autosave and are intentionally stored as semantic preferences.",
                Style::default().fg(palette::text_secondary(&state.tui_config)),
            )),
        ]),
        layout.hint,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_choice_row<F>(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    values: &[String],
    focused: bool,
    active_index: usize,
    cells: &[Rect],
    color_for_index: F,
) where
    F: Fn(usize) -> Color,
{
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);

    let label_style = if focused {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    frame.render_widget(Paragraph::new(Span::styled(label, label_style)), rows[0]);

    for (idx, cell) in cells.iter().copied().enumerate() {
        let active = idx == active_index;
        let fg = if active {
            Color::Black
        } else {
            color_for_index(idx)
        };
        let bg = if active {
            color_for_index(idx)
        } else {
            Color::Reset
        };

        let paragraph = Paragraph::new(Span::styled(
            values.get(idx).map_or("", String::as_str),
            Style::default().fg(fg).bg(bg).add_modifier(if active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if active {
                    color_for_index(idx)
                } else {
                    Color::DarkGray
                })),
        );
        frame.render_widget(paragraph, cell);
    }
}

fn render_theme_token_list(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    theme: &TuiThemeConfig,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::border(&state.tui_config)))
        .title(Span::styled(
            " Tokens ",
            Style::default()
                .fg(palette::accent_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(palette::bg_secondary(&state.tui_config)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = ThemeToken::all()
        .iter()
        .enumerate()
        .map(|(idx, token)| {
            let rgba = token.value(theme);
            let swatch = palette::rgba(rgba);
            let selected = idx == state.selected_theme_token;
            let marker = if selected { ">" } else { " " };
            Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(palette::accent_primary(&state.tui_config)),
                ),
                Span::styled("■ ", Style::default().fg(swatch)),
                Span::styled(
                    token.label(),
                    Style::default()
                        .fg(if selected {
                            palette::text_primary(&state.tui_config)
                        } else {
                            palette::text_secondary(&state.tui_config)
                        })
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_color_wheel(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(
            if state.selected_settings_field == 2
                || (3..=6).contains(&state.selected_settings_field)
            {
                palette::accent_primary(&state.tui_config)
            } else {
                palette::border(&state.tui_config)
            },
        ))
        .title(Span::styled(
            " Color Wheel ",
            Style::default()
                .fg(palette::accent_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(palette::bg_secondary(&state.tui_config)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 4 {
        return;
    }

    let mut lines = Vec::with_capacity(inner.height as usize);
    let cx = inner.width as f32 / 2.0;
    let cy = inner.height as f32 / 2.0;
    let radius = inner.width.min(inner.height) as f32 / 2.0 - 1.0;

    for y in 0..inner.height {
        let mut spans = Vec::with_capacity(inner.width as usize);
        for x in 0..inner.width {
            let dx = (x as f32 + 0.5) - cx;
            let dy = (y as f32 + 0.5) - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > radius || dist < radius * 0.28 {
                spans.push(Span::styled(
                    " ",
                    Style::default().bg(palette::bg_secondary(&state.tui_config)),
                ));
                continue;
            }

            let sat = (dist / radius).clamp(0.0, 1.0);
            let hue = ((dy.atan2(dx).to_degrees() + 360.0) % 360.0) / 360.0;
            let color = hsv_to_rgba(hue, sat, state.settings_color_value, 255);
            let selected = (hue - state.settings_color_hue).abs() < 0.03
                && (sat - state.settings_color_saturation).abs() < 0.08;
            spans.push(Span::styled(
                if selected { "•" } else { " " },
                Style::default()
                    .fg(if selected {
                        palette::text_primary(&state.tui_config)
                    } else {
                        Color::Reset
                    })
                    .bg(palette::rgba(color)),
            ));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_slider<F>(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    value: f32,
    focused: bool,
    color_at: F,
) where
    F: Fn(f32) -> RgbaColor,
{
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!(
                "{label} {:>3}%",
                (value.clamp(0.0, 1.0) * 100.0).round() as u16
            ),
            Style::default().fg(if focused { Color::White } else { Color::Gray }),
        )),
        rows[0],
    );

    let cells = equal_cells(rows[1], rows[1].width.max(1) as usize);
    for (idx, cell) in cells.iter().enumerate() {
        let ratio = if cells.len() <= 1 {
            0.0
        } else {
            idx as f32 / (cells.len() - 1) as f32
        };
        let selected = (ratio - value).abs() < (1.0 / cells.len().max(1) as f32);
        frame.render_widget(
            Paragraph::new(Span::styled(
                if selected { "│" } else { " " },
                Style::default()
                    .fg(if selected { Color::Black } else { Color::Reset })
                    .bg(palette::rgba(color_at(ratio))),
            )),
            *cell,
        );
    }
}

fn render_theme_preview(frame: &mut Frame, area: Rect, state: &AppState, theme: &TuiThemeConfig) {
    let active_token = ThemeToken::all()[state.selected_theme_token];
    let selected = active_token.value(theme);
    let selected_hex = format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        selected.r, selected.g, selected.b, selected.a
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::border(&state.tui_config)))
        .title(Span::styled(
            " Preview ",
            Style::default()
                .fg(palette::accent_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(palette::bg_secondary(&state.tui_config)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Fill(1),
        ])
        .split(inner);

    let swatch = Paragraph::new(vec![
        Line::from(Span::styled(
            "████████████████",
            Style::default().fg(palette::rgba(selected)),
        )),
        Line::from(Span::styled(
            active_token.label(),
            Style::default()
                .fg(palette::text_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            selected_hex,
            Style::default().fg(palette::text_secondary(&state.tui_config)),
        )),
    ])
    .alignment(Alignment::Center);
    frame.render_widget(swatch, rows[0]);

    let graph_preview = vec![
        Line::from(vec![
            Span::styled(
                "graph ",
                Style::default().fg(palette::rgba(theme.graph_primary)),
            ),
            Span::styled(
                "graph2 ",
                Style::default().fg(palette::rgba(theme.graph_secondary)),
            ),
            Span::styled("border", Style::default().fg(palette::rgba(theme.border))),
        ]),
        Line::from(vec![
            Span::styled(
                "user",
                Style::default().fg(palette::user_text(&state.tui_config)),
            ),
            Span::raw(" "),
            Span::styled(
                "ai",
                Style::default().fg(palette::ai_text(&state.tui_config)),
            ),
            Span::raw(" "),
            Span::styled(
                "thinking",
                Style::default().fg(palette::thinking_text(&state.tui_config)),
            ),
            Span::raw(" "),
            Span::styled(
                "tool",
                Style::default().fg(palette::tool_text(&state.tui_config)),
            ),
        ]),
        Line::from(vec![
            Span::styled("bg", Style::default().fg(palette::rgba(theme.bg_primary))),
            Span::raw(" "),
            Span::styled(
                "bg2",
                Style::default().fg(palette::rgba(theme.bg_secondary)),
            ),
            Span::raw(" "),
            Span::styled(
                format!("overlay α{}", theme.bg_overlay.a),
                Style::default().fg(palette::text_secondary(&state.tui_config)),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(graph_preview), rows[1]);

    let live = Paragraph::new(vec![
        Line::from(Span::styled(
            "Changes apply immediately and autosave to [tui], [tui.theme], and [tui.fonts].",
            Style::default().fg(palette::text_primary(&state.tui_config)),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Terminal alpha is approximate. RockBot stores the exact RGBA values and uses the overlay alpha to scale modal dimming.",
            Style::default().fg(palette::text_secondary(&state.tui_config)),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(live, rows[2]);
}

fn render_font_role_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(palette::border(&state.tui_config)))
        .title(Span::styled(
            " Roles ",
            Style::default()
                .fg(palette::accent_primary(&state.tui_config))
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(palette::bg_secondary(&state.tui_config)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = FontRole::all()
        .iter()
        .enumerate()
        .map(|(idx, role)| {
            let selected = idx == state.selected_font_role;
            let marker = if selected { ">" } else { " " };
            Line::from(vec![
                Span::styled(
                    marker,
                    Style::default().fg(palette::accent_primary(&state.tui_config)),
                ),
                Span::styled(
                    role.label(),
                    Style::default()
                        .fg(if selected {
                            palette::text_primary(&state.tui_config)
                        } else {
                            palette::text_secondary(&state.tui_config)
                        })
                        .add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_font_size_row(frame: &mut Frame, area: Rect, state: &AppState, active_role: FontRole) {
    let size = active_role.size(&state.tui_config.fonts);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if state.selected_font_field == 2 {
            palette::accent_primary(&state.tui_config)
        } else {
            palette::border(&state.tui_config)
        }))
        .style(Style::default().bg(palette::bg_secondary(&state.tui_config)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cells = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(4),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "[-]",
            Style::default().fg(palette::accent_secondary(&state.tui_config)),
        ))
        .alignment(Alignment::Center),
        cells[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("Stored Size: {size}"),
            Style::default().fg(palette::text_primary(&state.tui_config)),
        ))
        .alignment(Alignment::Center),
        cells[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "[+]",
            Style::default().fg(palette::accent_secondary(&state.tui_config)),
        ))
        .alignment(Alignment::Center),
        cells[2],
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{hsv_to_rgba, rgba_to_hsv};
    use rockbot_config::RgbaColor;

    #[test]
    fn hsv_rgb_round_trip_stays_close() {
        let original = RgbaColor {
            r: 96,
            g: 144,
            b: 225,
            a: 180,
        };
        let (h, s, v) = rgba_to_hsv(original);
        let round_trip = hsv_to_rgba(h, s, v, original.a);
        assert!((i16::from(round_trip.r) - i16::from(original.r)).abs() <= 1);
        assert!((i16::from(round_trip.g) - i16::from(original.g)).abs() <= 1);
        assert!((i16::from(round_trip.b) - i16::from(original.b)).abs() <= 1);
        assert_eq!(round_trip.a, original.a);
    }
}
