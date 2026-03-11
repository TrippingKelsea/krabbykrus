//! TUI visual effects using tachyonfx
//!
//! This module provides animated effects for the TUI, including:
//! - Active pane glow/pulse effect
//! - Transition animations
//! - Status indicators

use ratatui::style::Color;
use std::time::Instant;

/// Effect state for animated UI elements
#[derive(Debug, Clone)]
pub struct EffectState {
    /// Start time for animation calculations
    start_time: Instant,
    /// Whether this element is currently active/focused
    pub is_active: bool,
}

impl Default for EffectState {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            is_active: false,
        }
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
}

/// Color palette for the TUI
pub mod palette {
    use ratatui::style::Color;

    /// Active/focused element color (purple theme)
    pub const ACTIVE_PRIMARY: Color = Color::Rgb(147, 112, 219); // Medium purple
    pub const ACTIVE_SECONDARY: Color = Color::Rgb(186, 85, 211); // Medium orchid
    pub const ACTIVE_GLOW: Color = Color::Rgb(218, 112, 214); // Orchid
    
    /// Inactive element colors
    pub const INACTIVE_BORDER: Color = Color::Rgb(88, 88, 88); // Dark gray
    pub const INACTIVE_TEXT: Color = Color::Rgb(128, 128, 128); // Gray
    
    /// Status colors
    pub const SUCCESS: Color = Color::Rgb(46, 204, 113); // Green
    pub const WARNING: Color = Color::Rgb(241, 196, 15); // Yellow
    pub const ERROR: Color = Color::Rgb(231, 76, 60); // Red
    pub const INFO: Color = Color::Rgb(52, 152, 219); // Blue
    
    /// Provider status colors
    pub const CONFIGURED: Color = Color::Rgb(46, 204, 113); // Green
    pub const UNCONFIGURED: Color = Color::Rgb(241, 196, 15); // Yellow/amber
    pub const VAULT_HINT: Color = Color::Rgb(147, 112, 219); // Purple (vault action)
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
    
    Style::default()
        .fg(palette::INACTIVE_BORDER)
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
                format!("▸ {} ◂", tab)
            } else {
                format!("  {}  ", tab)
            }
        })
        .collect::<Vec<_>>()
        .join("│")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pulse_value_range() {
        for i in 0..100 {
            let elapsed = i as f64 * 0.1;
            let pulse = pulse_value(elapsed, 1.0);
            assert!(pulse >= 0.0 && pulse <= 1.0);
        }
    }

    #[test]
    fn test_active_border_color() {
        let color = active_border_color(0.0);
        match color {
            Color::Rgb(r, g, b) => {
                assert!(r > 100); // Should be in purple range
            }
            _ => panic!("Expected RGB color"),
        }
    }
}
