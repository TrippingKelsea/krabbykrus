//! Keybinding configuration and action dispatch for the TUI.
//!
//! Provides a data-driven keybinding system where actions are named, serializable,
//! and can be loaded from config. The `KeybindingConfig` holds bindings per mode
//! and can be serialized to/from TOML or JSON for user configuration.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Actions that can be triggered by keybindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TuiAction {
    Quit,
    NavLeft,
    NavRight,
    NavUp,
    NavDown,
    JumpToSection(u8), // 1-7 quick jump
    Add,               // 'a' action
    Edit,              // 'e' action
    Delete,            // 'd' action
    Refresh,           // 'r' / F5
    View,              // 'v' action
    Chat,              // 'c' action
    Kill,              // 'k' in non-sidebar context (kill session)
    OpenContextMenu,   // '?' action
    NewSession,        // 'n' on Sessions page
    StartGateway,      // 's' action
    StopGateway,       // 'S' action
    InitVault,         // 'i' action
    UnlockVault,       // 'u' action
    LockVault,         // 'l' action
    ContextFiles,      // 'f' on Agents page
    Permissions,       // 'p' action
    TestAction,        // 't' action
    PrevTab,           // Shift+[
    NextTab,           // Shift+]
    ScrollUp,          // PageUp / Ctrl+b
    ScrollDown,        // PageDown / Ctrl+f
    ScrollEnd,         // End / 'G'
    Enter,             // Enter key action
    Escape,            // Esc key action
    CardLeft,          // Alt+Left: navigate card slots left
    CardRight,         // Alt+Right: navigate card slots right
    CardUp,            // Alt+Up: cycle card bar mode up
    CardDown,          // Alt+Down: cycle card bar mode down
    CardActivate,      // Alt+Enter: open card detail overlay
    OpenVault,         // Alt+V: vault/credentials overlay
    OpenSettings,      // Alt+S: settings overlay (note: not 's' which is StartGateway)
    OpenModels,        // Alt+M: models overlay
    OpenCron,          // Alt+C: cron jobs overlay
}

/// A parsed key specification, e.g. "q", "ctrl+c", "Left", "Shift+Tab"
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySpec {
    Char(char),
    Key(KeyCode),
    Modified {
        modifiers: KeyModifiers,
        code: KeyCode,
    },
}

impl KeySpec {
    /// Returns true if this KeySpec matches the given crossterm KeyEvent.
    pub fn matches(&self, event: &KeyEvent) -> bool {
        match self {
            KeySpec::Char(c) => event.code == KeyCode::Char(*c) && event.modifiers.is_empty(),
            KeySpec::Key(code) => event.code == *code && event.modifiers.is_empty(),
            KeySpec::Modified { modifiers, code } => {
                event.code == *code && event.modifiers.contains(*modifiers)
            }
        }
    }

    /// Parse a string like "q", "ctrl+c", "Left", "Shift+Tab", "F5" into a KeySpec.
    pub fn parse(s: &str) -> Result<Self, String> {
        // Split on '+' to extract modifiers and the key.
        // We need to handle "Shift+]" carefully since ']' contains no '+'.
        let s = s.trim();
        let parts: Vec<&str> = s.splitn(2, '+').collect();

        if parts.len() == 1 {
            // No modifier
            let key_str = parts[0];
            if let Some(code) = parse_key_code(key_str) {
                // Single named key (Up, Down, F5, Tab, etc.)
                return Ok(KeySpec::Key(code));
            }
            // Single character
            let mut chars = key_str.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                return Ok(KeySpec::Char(c));
            }
            Err(format!("Unknown key: {s}"))
        } else {
            // Has a modifier prefix
            let modifier_str = parts[0].to_lowercase();
            let key_str = parts[1];

            let modifiers = match modifier_str.as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                other => return Err(format!("Unknown modifier: {other}")),
            };

            let code = if let Some(code) = parse_key_code(key_str) {
                code
            } else {
                let mut chars = key_str.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => return Err(format!("Unknown key after modifier: {key_str}")),
                }
            };

            Ok(KeySpec::Modified { modifiers, code })
        }
    }
}

/// Parse a named key code string (case-insensitive for most, exact for "F1"–"F12").
fn parse_key_code(s: &str) -> Option<KeyCode> {
    // Function keys: F1..F12
    if let Some(n_str) = s.strip_prefix('F').or_else(|| s.strip_prefix('f')) {
        if let Ok(n) = n_str.parse::<u8>() {
            return Some(KeyCode::F(n));
        }
    }
    match s {
        "Up" | "up" => Some(KeyCode::Up),
        "Down" | "down" => Some(KeyCode::Down),
        "Left" | "left" => Some(KeyCode::Left),
        "Right" | "right" => Some(KeyCode::Right),
        "Enter" | "enter" | "Return" | "return" => Some(KeyCode::Enter),
        "Esc" | "esc" | "Escape" | "escape" => Some(KeyCode::Esc),
        "Tab" | "tab" => Some(KeyCode::Tab),
        "BackTab" | "backtab" | "ShiftTab" | "shifttab" => Some(KeyCode::BackTab),
        "Backspace" | "backspace" => Some(KeyCode::Backspace),
        "Delete" | "delete" | "Del" | "del" => Some(KeyCode::Delete),
        "Insert" | "insert" | "Ins" | "ins" => Some(KeyCode::Insert),
        "Home" | "home" => Some(KeyCode::Home),
        "End" | "end" => Some(KeyCode::End),
        "PageUp" | "pageup" | "PgUp" | "pgup" => Some(KeyCode::PageUp),
        "PageDown" | "pagedown" | "PgDown" | "pgdown" => Some(KeyCode::PageDown),
        _ => None,
    }
}

fn key_code_to_str(code: &KeyCode) -> String {
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}

impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeySpec::Char(c) => write!(f, "{c}"),
            KeySpec::Key(code) => write!(f, "{}", key_code_to_str(code)),
            KeySpec::Modified { modifiers, code } => {
                let mod_str = if modifiers.contains(KeyModifiers::CONTROL) {
                    "ctrl"
                } else if modifiers.contains(KeyModifiers::ALT) {
                    "alt"
                } else if modifiers.contains(KeyModifiers::SHIFT) {
                    "shift"
                } else {
                    "unknown"
                };
                write!(f, "{}+{}", mod_str, key_code_to_str(code))
            }
        }
    }
}

impl Serialize for KeySpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for KeySpec {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for KeySpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// A single keybinding rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: KeySpec,
    pub action: TuiAction,
    /// Optional context guard (e.g. "sidebar_focused", "chat_active")
    #[serde(default)]
    pub context: Option<String>,
}

impl KeyBinding {
    fn new(key: KeySpec, action: TuiAction) -> Self {
        Self {
            key,
            action,
            context: None,
        }
    }

    pub fn with_context(key: KeySpec, action: TuiAction, ctx: &str) -> Self {
        Self {
            key,
            action,
            context: Some(ctx.to_string()),
        }
    }
}

/// All configurable keybindings, organized by input mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingConfig {
    pub normal: Vec<KeyBinding>,
    pub chat: Vec<KeyBinding>,
}

impl KeybindingConfig {
    /// Look up the action for a key event in the given mode.
    /// Returns the first matching binding's action, or None.
    pub fn lookup(&self, mode: &str, event: &KeyEvent) -> Option<TuiAction> {
        let bindings = match mode {
            "normal" => &self.normal,
            "chat" => &self.chat,
            _ => return None,
        };
        bindings
            .iter()
            .find(|b| b.key.matches(event))
            .map(|b| b.action.clone())
    }
}

impl Default for KeybindingConfig {
    fn default() -> Self {
        use KeyCode::*;
        use KeyModifiers as Km;

        fn char(c: char) -> KeySpec {
            KeySpec::Char(c)
        }
        fn key(code: KeyCode) -> KeySpec {
            KeySpec::Key(code)
        }
        fn modified(modifier: KeyModifiers, code: KeyCode) -> KeySpec {
            KeySpec::Modified {
                modifiers: modifier,
                code,
            }
        }

        let normal = vec![
            // Quit
            KeyBinding::new(char('q'), TuiAction::Quit),
            // Navigation (used by both slot_bar and content areas)
            KeyBinding::new(key(Up), TuiAction::NavUp),
            KeyBinding::new(char('k'), TuiAction::NavUp),
            KeyBinding::new(key(Down), TuiAction::NavDown),
            KeyBinding::new(char('j'), TuiAction::NavDown),
            KeyBinding::new(key(Left), TuiAction::NavLeft),
            KeyBinding::new(char('h'), TuiAction::NavLeft),
            KeyBinding::new(key(Right), TuiAction::NavRight),
            KeyBinding::new(char('l'), TuiAction::NavRight),
            // Enter / Esc
            KeyBinding::new(key(Enter), TuiAction::Enter),
            KeyBinding::new(key(Esc), TuiAction::Escape),
            // Quick section jump 1-7
            KeyBinding::new(char('1'), TuiAction::JumpToSection(1)),
            KeyBinding::new(char('2'), TuiAction::JumpToSection(2)),
            KeyBinding::new(char('3'), TuiAction::JumpToSection(3)),
            KeyBinding::new(char('4'), TuiAction::JumpToSection(4)),
            KeyBinding::new(char('5'), TuiAction::JumpToSection(5)),
            KeyBinding::new(char('6'), TuiAction::JumpToSection(6)),
            KeyBinding::new(char('7'), TuiAction::JumpToSection(7)),
            // Tab navigation within views
            KeyBinding::new(modified(Km::SHIFT, Char('[')), TuiAction::PrevTab),
            KeyBinding::new(modified(Km::SHIFT, Char(']')), TuiAction::NextTab),
            KeyBinding::new(key(BackTab), TuiAction::PrevTab),
            // Scroll
            KeyBinding::new(key(PageUp), TuiAction::ScrollUp),
            KeyBinding::new(modified(Km::CONTROL, Char('b')), TuiAction::ScrollUp),
            KeyBinding::new(key(PageDown), TuiAction::ScrollDown),
            KeyBinding::new(modified(Km::CONTROL, Char('f')), TuiAction::ScrollDown),
            KeyBinding::new(key(End), TuiAction::ScrollEnd),
            KeyBinding::new(char('G'), TuiAction::ScrollEnd),
            // Page-specific actions
            KeyBinding::new(char('a'), TuiAction::Add),
            KeyBinding::new(char('d'), TuiAction::Delete),
            KeyBinding::new(char('r'), TuiAction::Refresh),
            KeyBinding::new(key(F(5)), TuiAction::Refresh),
            KeyBinding::new(char('v'), TuiAction::View),
            KeyBinding::new(char('c'), TuiAction::Chat),
            KeyBinding::new(char('e'), TuiAction::Edit),
            KeyBinding::new(char('n'), TuiAction::NewSession),
            KeyBinding::new(char('s'), TuiAction::StartGateway),
            KeyBinding::new(char('S'), TuiAction::StopGateway),
            KeyBinding::new(char('i'), TuiAction::InitVault),
            KeyBinding::new(char('u'), TuiAction::UnlockVault),
            KeyBinding::new(char('f'), TuiAction::ContextFiles),
            KeyBinding::new(char('p'), TuiAction::Permissions),
            KeyBinding::new(char('t'), TuiAction::TestAction),
            KeyBinding::new(char('?'), TuiAction::OpenContextMenu),
            // Card bar navigation (Alt+arrows)
            KeyBinding::new(modified(Km::ALT, Left), TuiAction::CardLeft),
            KeyBinding::new(modified(Km::ALT, Right), TuiAction::CardRight),
            KeyBinding::new(modified(Km::ALT, Up), TuiAction::CardUp),
            KeyBinding::new(modified(Km::ALT, Down), TuiAction::CardDown),
            KeyBinding::new(modified(Km::ALT, Enter), TuiAction::CardActivate),
            // Overlay shortcuts
            KeyBinding::new(modified(Km::ALT, Char('v')), TuiAction::OpenVault),
            KeyBinding::new(modified(Km::ALT, Char('s')), TuiAction::OpenSettings),
            KeyBinding::new(modified(Km::ALT, Char('m')), TuiAction::OpenModels),
            KeyBinding::new(modified(Km::ALT, Char('c')), TuiAction::OpenCron),
        ];

        let chat = vec![
            KeyBinding::new(key(Enter), TuiAction::Enter),
            KeyBinding::new(key(Esc), TuiAction::Escape),
            KeyBinding::new(key(Up), TuiAction::ScrollUp),
            KeyBinding::new(key(Down), TuiAction::ScrollDown),
            KeyBinding::new(key(PageUp), TuiAction::ScrollUp),
            KeyBinding::new(key(PageDown), TuiAction::ScrollDown),
            // Card bar navigation works in chat mode too
            KeyBinding::new(modified(Km::ALT, Left), TuiAction::CardLeft),
            KeyBinding::new(modified(Km::ALT, Right), TuiAction::CardRight),
            KeyBinding::new(modified(Km::ALT, Up), TuiAction::CardUp),
            KeyBinding::new(modified(Km::ALT, Down), TuiAction::CardDown),
            KeyBinding::new(modified(Km::ALT, Enter), TuiAction::CardActivate),
            // Overlay shortcuts
            KeyBinding::new(modified(Km::ALT, Char('v')), TuiAction::OpenVault),
            KeyBinding::new(modified(Km::ALT, Char('s')), TuiAction::OpenSettings),
            KeyBinding::new(modified(Km::ALT, Char('m')), TuiAction::OpenModels),
            KeyBinding::new(modified(Km::ALT, Char('c')), TuiAction::OpenCron),
        ];

        Self { normal, chat }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn test_keyspec_parse_char() {
        let spec = KeySpec::parse("q").unwrap();
        assert_eq!(spec, KeySpec::Char('q'));
    }

    #[test]
    fn test_keyspec_parse_named_key() {
        let spec = KeySpec::parse("Tab").unwrap();
        assert_eq!(spec, KeySpec::Key(KeyCode::Tab));
    }

    #[test]
    fn test_keyspec_parse_modified() {
        let spec = KeySpec::parse("ctrl+c").unwrap();
        assert_eq!(
            spec,
            KeySpec::Modified {
                modifiers: KeyModifiers::CONTROL,
                code: KeyCode::Char('c'),
            }
        );
    }

    #[test]
    fn test_keyspec_parse_shift_bracket() {
        let spec = KeySpec::parse("shift+[").unwrap();
        assert_eq!(
            spec,
            KeySpec::Modified {
                modifiers: KeyModifiers::SHIFT,
                code: KeyCode::Char('['),
            }
        );
    }

    #[test]
    fn test_keyspec_parse_f5() {
        let spec = KeySpec::parse("F5").unwrap();
        assert_eq!(spec, KeySpec::Key(KeyCode::F(5)));
    }

    #[test]
    fn test_keyspec_matches_char() {
        let spec = KeySpec::Char('q');
        let event = key_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(spec.matches(&event));
    }

    #[test]
    fn test_keyspec_matches_char_with_modifier_fails() {
        let spec = KeySpec::Char('q');
        let event = key_event(KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert!(!spec.matches(&event));
    }

    #[test]
    fn test_keyspec_matches_modified() {
        let spec = KeySpec::Modified {
            modifiers: KeyModifiers::CONTROL,
            code: KeyCode::Char('c'),
        };
        let event = key_event(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(spec.matches(&event));
    }

    #[test]
    fn test_keyspec_roundtrip_display_parse() {
        let cases = ["q", "Tab", "ctrl+c", "shift+[", "F5", "PageUp"];
        for s in cases {
            let spec = KeySpec::parse(s).unwrap();
            let roundtrip = spec.to_string();
            let re_parsed = KeySpec::parse(&roundtrip).unwrap();
            assert_eq!(spec, re_parsed, "roundtrip failed for {s}: got {roundtrip}");
        }
    }

    #[test]
    fn test_default_config_lookup_quit() {
        // 'q' without sidebar_focused context should not match Quit (context guard)
        // The lookup method doesn't filter by context — that's done at dispatch time.
        // So lookup just finds the first matching key.
        let config = KeybindingConfig::default();
        let event = key_event(KeyCode::Char('q'), KeyModifiers::NONE);
        let action = config.lookup("normal", &event);
        assert_eq!(action, Some(TuiAction::Quit));
    }

    #[test]
    fn test_default_config_lookup_nav_up() {
        let config = KeybindingConfig::default();
        let event = key_event(KeyCode::Up, KeyModifiers::NONE);
        let action = config.lookup("normal", &event);
        assert_eq!(action, Some(TuiAction::NavUp));
    }

    #[test]
    fn test_default_config_lookup_jump_section() {
        let config = KeybindingConfig::default();
        let event = key_event(KeyCode::Char('3'), KeyModifiers::NONE);
        let action = config.lookup("normal", &event);
        assert_eq!(action, Some(TuiAction::JumpToSection(3)));
    }

    #[test]
    fn test_default_config_lookup_chat_enter() {
        let config = KeybindingConfig::default();
        let event = key_event(KeyCode::Enter, KeyModifiers::NONE);
        let action = config.lookup("chat", &event);
        assert_eq!(action, Some(TuiAction::Enter));
    }

    #[test]
    fn test_default_config_lookup_unknown_mode() {
        let config = KeybindingConfig::default();
        let event = key_event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(config.lookup("unknown_mode", &event).is_none());
    }

    #[test]
    fn test_keyspec_serde_roundtrip() {
        let spec = KeySpec::Modified {
            modifiers: KeyModifiers::SHIFT,
            code: KeyCode::Char('['),
        };
        let json = serde_json::to_string(&spec).unwrap();
        let de: KeySpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, de);
    }
}
