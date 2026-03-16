//! Slotted card bar component — mode selector + dynamic info cards.

use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::effects::{self, EffectState};
use crate::state::{AppState, SlotKind};

const CARD_WIDTH: u16 = 14;
const CARD_HEIGHT: u16 = 3;

/// Total height of the slot bar.
pub fn slot_bar_height() -> u16 {
    CARD_HEIGHT
}

/// Render the slotted card bar.
pub fn render_slot_bar(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    let bar = &state.slot_bar;
    let slot_count = bar.slots.len();

    if slot_count == 0 || area.height < CARD_HEIGHT {
        return;
    }

    let max_visible = (area.width / (CARD_WIDTH + 1)) as usize;
    let scroll_offset = bar
        .active_slot
        .saturating_sub(max_visible.saturating_sub(1));
    let visible_end = (scroll_offset + max_visible).min(slot_count);
    let visible_range = scroll_offset..visible_end;
    let visible_count = visible_range.len();

    let mut constraints: Vec<Constraint> = (0..visible_count)
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Fill(1));

    let cols = Layout::horizontal(&constraints)
        .flex(Flex::Start)
        .spacing(1)
        .split(area);

    for (i, slot_idx) in visible_range.clone().enumerate() {
        let slot = &bar.slots[slot_idx];
        let is_selected = slot_idx == bar.active_slot;

        let border_style = if is_selected {
            effects::active_border_style(effect_state.elapsed_secs())
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if is_selected {
                BorderType::Rounded
            } else {
                BorderType::Plain
            })
            .border_style(border_style);

        let inner = block.inner(cols[i]);
        frame.render_widget(block, cols[i]);

        match slot.kind {
            SlotKind::ModeSelector => {
                let label = format!("{}▲▼", slot.label);
                let max_w = CARD_WIDTH.saturating_sub(2) as usize;
                let display = if label.chars().count() > max_w {
                    let t: String = label.chars().take(max_w.saturating_sub(1)).collect();
                    format!("{t}…")
                } else {
                    label
                };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let p = Paragraph::new(Line::from(Span::styled(display, style)))
                    .alignment(Alignment::Center);
                frame.render_widget(p, inner);
            }
            SlotKind::InfoCard => {
                if let Some(view) = slot.views.get(slot.active_view) {
                    super::card_widgets::render_card_widget(&view.widget, frame, inner, state);
                } else {
                    let style = if is_selected {
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    let label = format!("{} {}", slot.icon, slot.label);
                    let max_w = CARD_WIDTH.saturating_sub(2) as usize;
                    let display = if label.chars().count() > max_w {
                        let t: String = label.chars().take(max_w.saturating_sub(1)).collect();
                        format!("{t}…")
                    } else {
                        label
                    };
                    let p = Paragraph::new(Line::from(Span::styled(display, style)))
                        .alignment(Alignment::Center);
                    frame.render_widget(p, inner);
                }

                // View count indicator (^v) if multiple views
                if slot.views.len() > 1 && inner.width > 2 && inner.height > 0 {
                    let indicator =
                        Paragraph::new(Span::styled("▲▼", Style::default().fg(Color::DarkGray)))
                            .alignment(Alignment::Right);
                    let ind_area = Rect {
                        x: inner.x + inner.width.saturating_sub(2),
                        y: inner.y,
                        width: 2,
                        height: 1,
                    };
                    frame.render_widget(indicator, ind_area);
                }
            }
        }
    }

    // Scroll indicators
    let can_left = scroll_offset > 0;
    let can_right = visible_end < slot_count;
    if can_left || can_right {
        super::render_card_scroll_hint(frame, cols[visible_count], can_left, can_right);
    }
}
