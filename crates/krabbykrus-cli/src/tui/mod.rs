//! Terminal User Interface for Krabbykrus
//!
//! A responsive async TUI built with ratatui that mirrors the web UI.
//!
//! Architecture:
//! - Async event loop using tokio::select! for concurrent event + task handling
//! - Message-passing via channels for state updates
//! - Component-based rendering with shared state
//! - Background task system for non-blocking data fetching

pub mod app;
pub mod components;
pub mod credentials;
pub mod event;
pub mod state;
pub mod ui;

pub use app::{App, run_app};
pub use credentials::CredentialsTui;
pub use state::{AppState, Message};
