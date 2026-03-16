//! Card chain navigation component — horizontal card strip with breadcrumbs.

use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::tui::effects::{self, EffectState};
use crate::tui::state::{AppState, CardChain};

/// Width of each card in the horizontal strip (in columns).
const CARD_WIDTH: u16 = 14;
/// Height of each card (border + label + badge).
const CARD_HEIGHT: u16 = 3;

/// Render the card chain: breadcrumbs (if depth > 1) + horizontal card strip.
///
/// Returns the height consumed so the caller can lay out remaining space.
pub fn render_card_chain(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    let chain = &state.card_chain;
    let depth = chain.levels.len();

    if depth > 1 {
        // Split: 1 row for breadcrumbs + remaining for cards
        let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(CARD_HEIGHT)])
            .split(area);
        render_breadcrumbs(frame, rows[0], chain);
        render_card_strip(frame, rows[1], state, effect_state);
    } else {
        render_card_strip(frame, area, state, effect_state);
    }
}

/// Total height needed to render the card chain area.
pub fn card_chain_height(chain: &CardChain) -> u16 {
    if chain.levels.len() > 1 {
        1 + CARD_HEIGHT // breadcrumbs + cards
    } else {
        CARD_HEIGHT
    }
}

/// Render breadcrumb trail: "RockBot > Agents > main-agent"
fn render_breadcrumbs(frame: &mut Frame, area: Rect, chain: &CardChain) {
    let crumbs = chain.breadcrumbs();
    let mut spans = Vec::new();
    for (i, crumb) in crumbs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                " > ",
                Style::default().fg(Color::DarkGray),
            ));
        }
        let style = if i == crumbs.len() - 1 {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(*crumb, style));
    }
    let line = Line::from(spans);
    let p = Paragraph::new(line);
    frame.render_widget(p, Rect { x: area.x + 1, width: area.width.saturating_sub(1), ..area });
}

/// Render horizontal card strip for the active level.
fn render_card_strip(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    effect_state: &EffectState,
) {
    let chain = &state.card_chain;
    let level = chain.active();
    let card_count = level.cards.len();

    if card_count == 0 || area.height < CARD_HEIGHT {
        return;
    }

    // Build constraints: one per visible card + filler
    let max_visible = (area.width / (CARD_WIDTH + 1)) as usize; // +1 for gap
    let scroll_offset = level.selected.saturating_sub(max_visible.saturating_sub(1));
    let visible_end = (scroll_offset + max_visible).min(card_count);
    let visible_range = scroll_offset..visible_end;
    let visible_count = visible_range.len();

    let mut constraints: Vec<Constraint> = (0..visible_count)
        .map(|_| Constraint::Length(CARD_WIDTH))
        .collect();
    constraints.push(Constraint::Fill(1)); // filler

    let cols = Layout::horizontal(&constraints)
        .flex(Flex::Start)
        .spacing(1)
        .split(area);

    for (i, card_idx) in visible_range.clone().enumerate() {
        let card = &level.cards[card_idx];
        let is_selected = card_idx == level.selected;

        let border_style = if is_selected && state.card_chain_focused {
            effects::active_border_style(effect_state.elapsed_secs())
        } else if is_selected {
            Style::default().fg(Color::Cyan)
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

        // Card content: icon + label (truncated to fit)
        let label = format!("{} {}", card.icon, card.label);
        let max_label_width = CARD_WIDTH.saturating_sub(2) as usize; // minus borders
        let display_label = if label.chars().count() > max_label_width {
            let truncated: String = label.chars().take(max_label_width.saturating_sub(1)).collect();
            format!("{truncated}…")
        } else {
            label
        };

        let label_style = if is_selected {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };

        let inner = block.inner(cols[i]);
        frame.render_widget(block, cols[i]);

        // Render label centered in the inner area
        let label_line = Line::from(Span::styled(display_label, label_style));
        let p = Paragraph::new(label_line).alignment(Alignment::Center);
        frame.render_widget(p, inner);
    }

    // Scroll indicators in the filler area
    let can_left = scroll_offset > 0;
    let can_right = visible_end < card_count;
    if can_left || can_right {
        super::render_card_scroll_hint(frame, cols[visible_count], can_left, can_right);
    }
}
