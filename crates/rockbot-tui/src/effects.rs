//! TUI visual effects using tachyonfx
//!
//! This module provides animated effects for the TUI, including:
//! - Active pane glow/pulse effect (tachyonfx hsl_shift_fg ping-pong)
//! - Modal open/close transitions (coalesce/dissolve)
//! - Page transition fades (fade_from_fg)
//! - Background dimming for modal overlays
//! - Static fallbacks when animations are disabled

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::time::{Duration, Instant};
use tachyonfx::{fx, Effect, EffectTimer, Interpolation, Shader};

/// Effect state for animated UI elements.
///
/// Holds both the legacy sine-wave pulse (for border colors) and tachyonfx
/// managed effects for richer transitions. Not `Clone` because `Effect`
/// wraps a boxed shader.
pub struct EffectState {
    /// Start time for legacy pulse calculations
    start_time: Instant,
    /// Whether this element is currently active/focused
    pub is_active: bool,
    /// Whether animations are enabled (from TuiConfig)
    pub animations_enabled: bool,
    /// Animation style for modal transitions
    pub animation_style: rockbot_config::AnimationStyle,
    /// tachyonfx effect: modal open animation
    modal_open: Option<Effect>,
    /// tachyonfx effect: modal close animation
    modal_close: Option<Effect>,
    /// tachyonfx effect: page transition fade
    page_transition: Option<Effect>,
}

impl Default for EffectState {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            is_active: false,
            animations_enabled: true,
            animation_style: rockbot_config::AnimationStyle::default(),
            modal_open: None,
            modal_close: None,
            page_transition: None,
        }
    }
}

// Manual Debug since Effect doesn't impl Debug
impl std::fmt::Debug for EffectState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectState")
            .field("is_active", &self.is_active)
            .field("animations_enabled", &self.animations_enabled)
            .field("animation_style", &self.animation_style)
            .field("modal_open", &self.modal_open.is_some())
            .field("modal_close", &self.modal_close.is_some())
            .field("page_transition", &self.page_transition.is_some())
            .finish()
    }
}

impl EffectState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the elapsed time since effect started (for animations)
    pub fn elapsed_secs(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Reset the animation timer
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
    }

    /// Set active state
    pub fn set_active(&mut self, active: bool) {
        if self.is_active != active {
            self.reset();
        }
        self.is_active = active;
    }

    /// Set whether animations are enabled
    pub fn set_animations_enabled(&mut self, enabled: bool) {
        self.animations_enabled = enabled;
    }

    /// Trigger a modal-open animation based on current animation style
    pub fn trigger_modal_open(&mut self) {
        if !self.animations_enabled
            || matches!(self.animation_style, rockbot_config::AnimationStyle::None)
        {
            return;
        }
        self.modal_open = Some(match self.animation_style {
            rockbot_config::AnimationStyle::Coalesce => {
                fx::coalesce(EffectTimer::from_ms(600, Interpolation::CubicOut))
            }
            rockbot_config::AnimationStyle::Fade => fx::fade_from_fg(
                Color::Black,
                EffectTimer::from_ms(400, Interpolation::CubicOut),
            ),
            rockbot_config::AnimationStyle::Slide => fx::slide_in(
                tachyonfx::Motion::DownToUp,
                3,
                0,
                Color::Black,
                EffectTimer::from_ms(350, Interpolation::CubicOut),
            ),
            rockbot_config::AnimationStyle::None => return,
        });
    }

    /// Trigger a modal-close animation based on current animation style
    pub fn trigger_modal_close(&mut self) {
        if !self.animations_enabled
            || matches!(self.animation_style, rockbot_config::AnimationStyle::None)
        {
            return;
        }
        self.modal_close = Some(match self.animation_style {
            rockbot_config::AnimationStyle::Coalesce => {
                fx::dissolve(EffectTimer::from_ms(300, Interpolation::CubicIn))
            }
            rockbot_config::AnimationStyle::Fade => fx::fade_to_fg(
                Color::Black,
                EffectTimer::from_ms(250, Interpolation::CubicIn),
            ),
            rockbot_config::AnimationStyle::Slide => fx::slide_out(
                tachyonfx::Motion::UpToDown,
                3,
                0,
                Color::Black,
                EffectTimer::from_ms(300, Interpolation::CubicIn),
            ),
            rockbot_config::AnimationStyle::None => return,
        });
    }

    /// Trigger a page transition animation (fade from white)
    pub fn trigger_page_transition(&mut self) {
        if !self.animations_enabled
            || matches!(self.animation_style, rockbot_config::AnimationStyle::None)
        {
            return;
        }
        self.page_transition = Some(fx::fade_from_fg(
            Color::White,
            EffectTimer::from_ms(250, Interpolation::CubicOut),
        ));
    }

    /// Process and render the modal-open effect on the given area.
    /// Returns true if an animation is still running.
    pub fn render_modal_open(&mut self, buf: &mut Buffer, area: Rect, elapsed: Duration) -> bool {
        if let Some(ref mut effect) = self.modal_open {
            let overflow = effect.process(elapsed, buf, area);
            if overflow.is_some() {
                self.modal_open = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Process and render the modal-close effect on the given area.
    /// Returns true if an animation is still running.
    pub fn render_modal_close(&mut self, buf: &mut Buffer, area: Rect, elapsed: Duration) -> bool {
        if let Some(ref mut effect) = self.modal_close {
            let overflow = effect.process(elapsed, buf, area);
            if overflow.is_some() {
                self.modal_close = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Process and render the page transition effect on the given area.
    /// Returns true if an animation is still running.
    pub fn render_page_transition(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        elapsed: Duration,
    ) -> bool {
        if let Some(ref mut effect) = self.page_transition {
            let overflow = effect.process(elapsed, buf, area);
            if overflow.is_some() {
                self.page_transition = None;
                return false;
            }
            return true;
        }
        false
    }

    /// Dim the background buffer according to a configurable overlay alpha.
    /// Used as a backdrop behind modals.
    pub fn dim_background(buf: &mut Buffer, area: Rect, overlay_alpha: u8) {
        let keep_ratio = 1.0 - (f32::from(overlay_alpha) / 255.0).clamp(0.0, 0.85);
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    // Dim foreground
                    if let Color::Rgb(r, g, b) = cell.fg {
                        cell.set_fg(Color::Rgb(
                            (f32::from(r) * keep_ratio) as u8,
                            (f32::from(g) * keep_ratio) as u8,
                            (f32::from(b) * keep_ratio) as u8,
                        ));
                    }
                    // Dim background
                    if let Color::Rgb(r, g, b) = cell.bg {
                        cell.set_bg(Color::Rgb(
                            (f32::from(r) * keep_ratio) as u8,
                            (f32::from(g) * keep_ratio) as u8,
                            (f32::from(b) * keep_ratio) as u8,
                        ));
                    }
                }
            }
        }
    }
}

/// Color palette for the TUI
pub mod palette {
    use ratatui::style::Color;
    use rockbot_config::{ColorTheme, RgbaColor, TuiConfig};

    /// Active/focused element color (default purple theme — used by const references)
    pub const ACTIVE_PRIMARY: Color = Color::Rgb(147, 112, 219);
    pub const ACTIVE_SECONDARY: Color = Color::Rgb(186, 85, 211);
    pub const ACTIVE_GLOW: Color = Color::Rgb(218, 112, 214);

    /// Inactive element colors
    pub const INACTIVE_BORDER: Color = Color::Rgb(88, 88, 88);
    pub const INACTIVE_TEXT: Color = Color::Rgb(128, 128, 128);

    /// Status colors
    pub const SUCCESS: Color = Color::Rgb(46, 204, 113);
    pub const WARNING: Color = Color::Rgb(241, 196, 15);
    pub const ERROR: Color = Color::Rgb(231, 76, 60);
    pub const INFO: Color = Color::Rgb(52, 152, 219);

    /// Provider status colors
    pub const CONFIGURED: Color = Color::Rgb(46, 204, 113);
    pub const UNCONFIGURED: Color = Color::Rgb(241, 196, 15);
    pub const VAULT_HINT: Color = Color::Rgb(147, 112, 219);

    pub fn rgba(color: RgbaColor) -> Color {
        Color::Rgb(color.r, color.g, color.b)
    }

    fn resolved(cfg: &TuiConfig) -> rockbot_config::TuiThemeConfig {
        cfg.resolved_theme()
    }

    pub fn border(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).border)
    }

    pub fn text_primary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).text_primary)
    }

    pub fn text_secondary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).text_secondary)
    }

    pub fn accent_primary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).accent_primary)
    }

    pub fn accent_secondary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).accent_secondary)
    }

    pub fn accent_tertiary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).accent_tertiary)
    }

    pub fn graph_primary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).graph_primary)
    }

    pub fn graph_secondary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).graph_secondary)
    }

    pub fn bg_primary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).bg_primary)
    }

    pub fn bg_secondary(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).bg_secondary)
    }

    pub fn overlay_alpha(cfg: &TuiConfig) -> u8 {
        resolved(cfg).bg_overlay.a
    }

    pub fn user_text(cfg: &TuiConfig) -> Color {
        accent_primary(cfg)
    }

    pub fn ai_text(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).ai_text_color)
    }

    pub fn thinking_text(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).thinking_text_color)
    }

    pub fn tool_text(cfg: &TuiConfig) -> Color {
        rgba(resolved(cfg).tool_text_color)
    }

    /// Theme-driven primary color
    pub fn theme_primary(theme: &ColorTheme) -> Color {
        match theme {
            ColorTheme::Purple => Color::Rgb(147, 112, 219),
            ColorTheme::Blue => Color::Rgb(65, 105, 225),
            ColorTheme::Green => Color::Rgb(46, 204, 113),
            ColorTheme::Rose => Color::Rgb(219, 112, 147),
            ColorTheme::Amber => Color::Rgb(255, 191, 0),
            ColorTheme::Mono => Color::Rgb(180, 180, 180),
        }
    }

    /// Theme-driven secondary color
    pub fn theme_secondary(theme: &ColorTheme) -> Color {
        match theme {
            ColorTheme::Purple => Color::Rgb(186, 85, 211),
            ColorTheme::Blue => Color::Rgb(100, 149, 237),
            ColorTheme::Green => Color::Rgb(80, 220, 140),
            ColorTheme::Rose => Color::Rgb(255, 105, 180),
            ColorTheme::Amber => Color::Rgb(255, 165, 0),
            ColorTheme::Mono => Color::Rgb(200, 200, 200),
        }
    }

    /// Theme-driven glow color
    pub fn theme_glow(theme: &ColorTheme) -> Color {
        match theme {
            ColorTheme::Purple => Color::Rgb(218, 112, 214),
            ColorTheme::Blue => Color::Rgb(135, 206, 250),
            ColorTheme::Green => Color::Rgb(144, 238, 144),
            ColorTheme::Rose => Color::Rgb(255, 182, 193),
            ColorTheme::Amber => Color::Rgb(255, 215, 0),
            ColorTheme::Mono => Color::Rgb(220, 220, 220),
        }
    }
}

/// Calculate a pulsing value for active element borders
/// Returns a value between 0.0 and 1.0 based on sine wave
pub fn pulse_value(elapsed_secs: f64, speed: f64) -> f64 {
    let phase = elapsed_secs * speed * std::f64::consts::PI;
    (phase.sin() + 1.0) / 2.0
}

/// Get the active border color with pulse effect
pub fn active_border_color(elapsed_secs: f64) -> Color {
    let pulse = pulse_value(elapsed_secs, 0.5); // Slow pulse

    // Interpolate between primary and glow colors
    let r1 = 147u8;
    let g1 = 112u8;
    let b1 = 219u8;

    let r2 = 218u8;
    let g2 = 112u8;
    let b2 = 214u8;

    let r = (r1 as f64 + (r2 as f64 - r1 as f64) * pulse) as u8;
    let g = (g1 as f64 + (g2 as f64 - g1 as f64) * pulse) as u8;
    let b = (b1 as f64 + (b2 as f64 - b1 as f64) * pulse) as u8;

    Color::Rgb(r, g, b)
}

/// Get style for active pane border
pub fn active_border_style(elapsed_secs: f64) -> ratatui::style::Style {
    use ratatui::style::{Modifier, Style};

    Style::default()
        .fg(active_border_color(elapsed_secs))
        .add_modifier(Modifier::BOLD)
}

/// Get style for inactive pane border
pub fn inactive_border_style() -> ratatui::style::Style {
    use ratatui::style::Style;

    Style::default().fg(palette::INACTIVE_BORDER)
}

/// Tab indicator styling
pub struct TabStyle {
    pub active_fg: Color,
    pub active_bg: Color,
    pub inactive_fg: Color,
    pub inactive_bg: Color,
}

impl Default for TabStyle {
    fn default() -> Self {
        Self {
            active_fg: Color::White,
            active_bg: palette::ACTIVE_PRIMARY,
            inactive_fg: palette::INACTIVE_TEXT,
            inactive_bg: Color::Reset,
        }
    }
}

/// Format tab bar with active indicator
pub fn render_tab_bar(tabs: &[&str], active_index: usize) -> String {
    tabs.iter()
        .enumerate()
        .map(|(i, tab)| {
            if i == active_index {
                format!("▸ {tab} ◂")
            } else {
                format!("  {tab}  ")
            }
        })
        .collect::<Vec<_>>()
        .join("│")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_pulse_value_range() {
        for i in 0..100 {
            let elapsed = i as f64 * 0.1;
            let pulse = pulse_value(elapsed, 1.0);
            assert!((0.0..=1.0).contains(&pulse));
        }
    }

    #[test]
    fn test_active_border_color() {
        let color = active_border_color(0.0);
        match color {
            Color::Rgb(r, _g, _b) => {
                assert!(r > 100); // Should be in purple range
            }
            _ => panic!("Expected RGB color"),
        }
    }

    #[test]
    fn test_effect_state_animations_disabled() {
        let mut state = EffectState::new();
        state.set_animations_enabled(false);
        state.trigger_modal_open();
        assert!(state.modal_open.is_none());
        state.trigger_page_transition();
        assert!(state.page_transition.is_none());
    }

    #[test]
    fn test_effect_state_animations_enabled() {
        let mut state = EffectState::new();
        state.trigger_modal_open();
        assert!(state.modal_open.is_some());
        state.trigger_page_transition();
        assert!(state.page_transition.is_some());
    }

    #[test]
    fn test_dim_background() {
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        // Set a known RGB color
        buf.cell_mut((0, 0))
            .unwrap()
            .set_fg(Color::Rgb(100, 200, 50));
        EffectState::dim_background(&mut buf, area, 180);
        match buf.cell((0, 0)).unwrap().fg {
            Color::Rgb(r, g, b) => {
                assert!(r < 100);
                assert!(g < 200);
                assert!(b < 50);
            }
            _ => panic!("Expected RGB color"),
        }
    }
}
