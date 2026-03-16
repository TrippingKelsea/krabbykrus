//! Event handling utilities
//!
//! Legacy event handler kept for standalone credentials TUI.
//! The main TUI now uses async tokio::select! directly in app.rs.

use crossterm::event::{self, Event as CrosstermEvent, KeyEvent, MouseEvent};
use std::time::Duration;

/// Terminal events
#[derive(Debug)]
pub enum Event {
    /// Key press event
    Key(KeyEvent),
    /// Mouse event
    Mouse(MouseEvent),
    /// Terminal resize
    Resize(u16, u16),
    /// Tick for periodic updates
    Tick,
}

/// Synchronous event handler (for standalone TUIs)
///
/// Note: The main TUI uses async event handling via tokio::select!.
/// This is kept for the standalone credentials TUI and backward compatibility.
pub struct EventHandler {
    tick_rate: Duration,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64) -> Self {
        Self {
            tick_rate: Duration::from_millis(tick_rate_ms),
        }
    }

    /// Poll for the next event (blocking)
    pub fn next(&self) -> anyhow::Result<Event> {
        if event::poll(self.tick_rate)? {
            match event::read()? {
                CrosstermEvent::Key(key) => Ok(Event::Key(key)),
                CrosstermEvent::Mouse(mouse) => Ok(Event::Mouse(mouse)),
                CrosstermEvent::Resize(w, h) => Ok(Event::Resize(w, h)),
                _ => Ok(Event::Tick),
            }
        } else {
            Ok(Event::Tick)
        }
    }
}

// Note: For async event handling, see app.rs which uses tokio::select! directly
