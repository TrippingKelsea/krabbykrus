//! Terminal User Interface for RockBot
//!
//! A responsive async TUI built with ratatui that mirrors the web UI.
//!
//! Architecture:
//! - Async event loop using tokio::select! for concurrent event + task handling
//! - Message-passing via channels for state updates
//! - Component-based rendering with shared state
//! - Background task system for non-blocking data fetching
//! - Visual effects via tachyonfx for active element indication
//!
//! # Navigation
//!
//! - **Tab**: Switch between sidebar and content pane
//! - **Up/Down** or **j/k**: Navigate within pane
//! - **Shift+[** / **Shift+]**: Switch between tabs within a view
//! - **1-6**: Quick jump to section
//! - **Enter**: Select / Confirm
//! - **Esc**: Cancel / Back

pub mod app;
pub mod chat_commands;
pub mod components;
pub mod credentials;
#[cfg(feature = "doctor-ai")]
pub mod doctor_tui;
pub mod effects;
pub mod event;
pub mod keybindings;
pub mod state;
pub mod ui;

pub use app::{run_app, App};
pub use credentials::CredentialsTui;
pub use effects::{palette, EffectState};
pub use keybindings::KeybindingConfig;
pub use state::{AppState, Message};
