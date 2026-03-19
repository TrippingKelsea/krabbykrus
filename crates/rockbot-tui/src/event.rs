//! Terminal event handling — async EventStream, RAII terminal guard, and input normalization.
//!
//! The main TUI uses [`AppEvent`] as a unified event bus. Terminal input is read
//! from a dedicated async task via crossterm's [`EventStream`], not poll/read.
//!
//! [`TerminalGuard`] owns terminal lifecycle (raw mode, alternate screen,
//! keyboard enhancement, bracketed paste, focus change) and restores state on drop.

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EventStream, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, KeyboardEnhancementFlags, MouseEvent, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags, EnableBracketedPaste, EnableFocusChange,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{event::Event as CrosstermEvent, execute};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::panic::{self, PanicHookInfo};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::state::Message;

// ---------------------------------------------------------------------------
// AppEvent — unified event bus
// ---------------------------------------------------------------------------

/// All events the main loop can receive.
#[derive(Debug)]
pub enum AppEvent {
    /// Key press (already filtered to Press kind only)
    Key(KeyEvent),
    /// Bracketed paste
    Paste(String),
    /// Mouse event
    Mouse(MouseEvent),
    /// Terminal resized
    Resize(u16, u16),
    /// Terminal gained focus
    FocusGained,
    /// Terminal lost focus
    FocusLost,
    /// Tick for animations / periodic counters
    Tick,
    /// Background task message (state updates)
    Msg(Message),
    /// Gateway event from WebSocket
    Gateway(rockbot_client::GatewayEvent),
}

// ---------------------------------------------------------------------------
// Terminal input stream
// ---------------------------------------------------------------------------

/// Spawn a task that reads terminal events via crossterm's `EventStream`
/// and forwards them as `AppEvent`s. Returns when the stream ends or the
/// receiver is dropped.
pub fn spawn_terminal_input(tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let mut stream = EventStream::new();

        while let Some(result) = stream.next().await {
            let event = match result {
                Ok(evt) => evt,
                Err(err) => {
                    tracing::warn!("terminal event error: {err}");
                    continue;
                }
            };

            let app_event = match event {
                CrosstermEvent::Key(key) => {
                    // Only forward key-press events (ignore release/repeat)
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    AppEvent::Key(key)
                }
                CrosstermEvent::Paste(text) => AppEvent::Paste(text),
                CrosstermEvent::Mouse(mouse) => AppEvent::Mouse(mouse),
                CrosstermEvent::Resize(w, h) => AppEvent::Resize(w, h),
                CrosstermEvent::FocusGained => AppEvent::FocusGained,
                CrosstermEvent::FocusLost => AppEvent::FocusLost,
            };

            if tx.send(app_event).is_err() {
                break; // receiver dropped, app is shutting down
            }
        }
    });
}

// ---------------------------------------------------------------------------
// TerminalGuard — RAII terminal lifecycle
// ---------------------------------------------------------------------------

/// Owns the terminal session. On drop, restores raw mode, alternate screen,
/// keyboard enhancement, bracketed paste, and focus change — even on panic.
pub struct TerminalGuard {
    keyboard_enhanced: bool,
}

impl TerminalGuard {
    /// Enter raw mode, alternate screen, and enable optional terminal features.
    /// Returns the guard (for RAII cleanup) and the ratatui Terminal.
    pub fn enter() -> Result<(Self, Terminal<CrosstermBackend<io::Stdout>>)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();

        execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableFocusChange,
        )?;

        // Try to enable Kitty keyboard protocol for Shift+Enter detection.
        // This writes CSI > flags u to the terminal. Terminals that support it
        // will report modifier keys on Enter; others silently ignore the sequence.
        // We don't use supports_keyboard_enhancement() because it requires
        // the `use-dev-tty` feature which has a build incompatibility with
        // `event-stream` in crossterm 0.28.
        let keyboard_enhanced = execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )
        .is_ok();

        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok((Self { keyboard_enhanced }, terminal))
    }

    /// Whether the Kitty keyboard protocol is active.
    pub fn keyboard_enhanced(&self) -> bool {
        self.keyboard_enhanced
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal_state(self.keyboard_enhanced);
    }
}

/// Best-effort terminal restoration for panic and error paths.
pub fn force_restore_terminal() {
    restore_terminal_state(true);
}

fn restore_terminal_state(pop_keyboard_flags: bool) {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    if pop_keyboard_flags {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        Show,
    );
}

pub struct PanicTerminalRestoreGuard {
    previous_hook: Arc<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>,
}

impl PanicTerminalRestoreGuard {
    pub fn install() -> Self {
        let previous_hook: Arc<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static> =
            Arc::from(panic::take_hook());
        let previous_for_hook = Arc::clone(&previous_hook);
        let hook = move |info: &PanicHookInfo<'_>| {
            force_restore_terminal();
            previous_for_hook(info);
        };
        panic::set_hook(Box::new(hook));
        Self { previous_hook }
    }
}

impl Drop for PanicTerminalRestoreGuard {
    fn drop(&mut self) {
        let previous_hook = Arc::clone(&self.previous_hook);
        panic::set_hook(Box::new(move |info| previous_hook(info)));
    }
}

// ---------------------------------------------------------------------------
// Input normalization
// ---------------------------------------------------------------------------

/// High-level input actions produced by normalizing raw key events.
/// Consumers match on these instead of raw `KeyCode` + `KeyModifiers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Submit / confirm (plain Enter)
    Submit,
    /// Insert newline (Shift+Enter, Ctrl+J, Ctrl+N)
    Newline,
    /// Cancel / escape
    Cancel,
    /// Typed text (only when modifiers are empty or Shift-only)
    Text(char),
    /// Navigation
    NavUp,
    NavDown,
    NavLeft,
    NavRight,
    /// Editing
    Backspace,
    Delete,
    Home,
    End,
    /// Scroll
    PageUp,
    PageDown,
    /// Raw key event that doesn't map to a known action
    Raw(KeyEvent),
}

/// Normalize a key event into an [`InputAction`] suitable for text-input contexts.
///
/// This is the single source of truth for "what does this key mean in a text field."
/// Modal/navigation contexts should use the keybinding system instead.
pub fn normalize_for_text_input(key: KeyEvent) -> InputAction {
    let mods = key.modifiers;

    // Newline: Shift+Enter (all representations), Ctrl+J, Ctrl+N
    if mods.contains(KeyModifiers::SHIFT)
        && matches!(
            key.code,
            KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n')
        )
    {
        return InputAction::Newline;
    }
    if mods.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('j') | KeyCode::Char('n'))
    {
        return InputAction::Newline;
    }

    // Submit: plain Enter
    if key.code == KeyCode::Enter && mods.is_empty() {
        return InputAction::Submit;
    }

    // Cancel
    if key.code == KeyCode::Esc {
        return InputAction::Cancel;
    }

    // Navigation
    match key.code {
        KeyCode::Up if mods.is_empty() => return InputAction::NavUp,
        KeyCode::Down if mods.is_empty() => return InputAction::NavDown,
        KeyCode::Left if mods.is_empty() => return InputAction::NavLeft,
        KeyCode::Right if mods.is_empty() => return InputAction::NavRight,
        KeyCode::PageUp => return InputAction::PageUp,
        KeyCode::PageDown => return InputAction::PageDown,
        _ => {}
    }

    // Editing
    match key.code {
        KeyCode::Backspace => return InputAction::Backspace,
        KeyCode::Delete => return InputAction::Delete,
        KeyCode::Home => return InputAction::Home,
        KeyCode::End => return InputAction::End,
        _ => {}
    }

    // Text: only accept when modifiers are empty or Shift-only.
    // This prevents Alt/Ctrl combos from being inserted as text.
    if let KeyCode::Char(c) = key.code {
        if mods.is_empty() || mods == KeyModifiers::SHIFT {
            return InputAction::Text(c);
        }
    }

    InputAction::Raw(key)
}

// ---------------------------------------------------------------------------
// Legacy event handler (kept for standalone credentials TUI)
// ---------------------------------------------------------------------------

/// Terminal events (legacy synchronous API)
#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Mouse(crossterm::event::MouseEvent),
    Resize(u16, u16),
    Tick,
}

/// Synchronous event handler for standalone TUIs (credentials, doctor).
pub struct EventHandler {
    tick_rate: std::time::Duration,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64) -> Self {
        Self {
            tick_rate: std::time::Duration::from_millis(tick_rate_ms),
        }
    }

    pub fn next(&self) -> Result<Event> {
        if crossterm::event::poll(self.tick_rate)? {
            match crossterm::event::read()? {
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
