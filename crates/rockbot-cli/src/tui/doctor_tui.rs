//! Standalone Doctor AI chat TUI — no gateway required.
//!
//! A minimal ratatui terminal for free-form conversation with the Doctor AI
//! local model. Loads/downloads the model on startup, then runs an interactive
//! chat loop.

use anyhow::Result;
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame, Terminal,
};
use rockbot_doctor::{DoctorAi, Role};
use std::{io, path::PathBuf, time::Duration};

/// State for the Doctor AI chat TUI.
struct DoctorTui {
    doctor: DoctorAi,
    /// Conversation history (role, content) pairs.
    messages: Vec<(Role, String)>,
    /// Current text the user is typing.
    input_buffer: String,
    /// Byte cursor position within `input_buffer`.
    input_cursor: usize,
    /// Vertical scroll offset for the message area.
    scroll: usize,
    /// Whether the TUI should exit on the next loop iteration.
    should_exit: bool,
    /// Status line message (e.g. "Thinking…" or errors).
    status: String,
    /// Whether we are currently waiting for a model response.
    thinking: bool,
}

impl DoctorTui {
    fn new(doctor: DoctorAi) -> Self {
        Self {
            doctor,
            messages: Vec::new(),
            input_buffer: String::new(),
            input_cursor: 0,
            scroll: 0,
            should_exit: false,
            status: String::from("Type a message and press Enter. /quit or Ctrl+C to exit."),
            thinking: false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        // Layout: message area (fill) + input (3 rows) + status (1 row)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        // --- Message area ---
        let mut lines: Vec<Line> = Vec::new();
        for (role, content) in &self.messages {
            let (label, label_style, content_style) = match role {
                Role::User => (
                    "You",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(Color::White),
                ),
                Role::Assistant => (
                    "Doctor",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(Color::Gray),
                ),
            };
            lines.push(Line::from(vec![Span::styled(
                format!("{label}: "),
                label_style,
            )]));
            for text_line in content.lines() {
                lines.push(Line::from(vec![Span::styled(
                    format!("  {text_line}"),
                    content_style,
                )]));
            }
            lines.push(Line::from(""));
        }

        if self.thinking {
            lines.push(Line::from(vec![Span::styled(
                "Doctor: Thinking…",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            )]));
        }

        let scroll_offset = self.scroll as u16;
        let messages = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Doctor AI ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green)),
            )
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset, 0));
        frame.render_widget(messages, chunks[0]);

        // --- Input area ---
        let input_display = format!("{}_", self.input_buffer);
        let input = Paragraph::new(input_display)
            .block(
                Block::default()
                    .title(" Message ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .style(Style::default().fg(Color::White));
        frame.render_widget(input, chunks[1]);

        // --- Status bar ---
        let status = Paragraph::new(self.status.as_str())
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(status, chunks[2]);
    }

    fn scroll_to_bottom(&mut self, message_area_height: u16) {
        // Rough estimate: count total rendered lines across messages
        let total_lines: usize = self
            .messages
            .iter()
            .map(|(_, content)| content.lines().count() + 2) // content + label + blank
            .sum::<usize>()
            + if self.thinking { 1 } else { 0 };

        let visible = message_area_height.saturating_sub(2) as usize; // subtract borders
        self.scroll = total_lines.saturating_sub(visible);
    }

    fn handle_char(&mut self, c: char) {
        let byte_pos = self
            .input_buffer
            .char_indices()
            .nth(self.input_cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.input_buffer.len());
        self.input_buffer.insert(byte_pos, c);
        self.input_cursor += 1;
    }

    fn handle_backspace(&mut self) {
        if self.input_cursor > 0 {
            self.input_cursor -= 1;
            let byte_pos = self
                .input_buffer
                .char_indices()
                .nth(self.input_cursor)
                .map(|(i, _)| i)
                .unwrap_or(self.input_buffer.len());
            self.input_buffer.remove(byte_pos);
        }
    }
}

/// Entry point called from the doctor command dispatcher.
pub async fn run_doctor_tui(config_path: &PathBuf) -> Result<()> {
    // Setup terminal before doing any work so we can show a loading screen.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Show loading screen while the model initializes.
    terminal.draw(|frame| {
        let area = frame.area();
        let msg = Paragraph::new("Doctor AI: loading model… (this may download on first run)")
            .block(
                Block::default()
                    .title(" Doctor AI ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green)),
            )
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(msg, area);
    })?;

    // Load doctor config from the config file (best-effort).
    let doctor_config = if config_path.exists() {
        if let Ok(raw) = tokio::fs::read_to_string(config_path).await {
            crate::commands::doctor::try_parse_doctor_config_from_raw(&raw)
        } else {
            rockbot_doctor::DoctorConfig::default()
        }
    } else {
        rockbot_doctor::DoctorConfig::default()
    };

    let doctor = match DoctorAi::init(doctor_config).await {
        Ok(d) => d,
        Err(e) => {
            // Restore terminal before showing the error.
            disable_raw_mode()?;
            execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
            anyhow::bail!("Doctor AI failed to initialize: {e}");
        }
    };

    let mut tui = DoctorTui::new(doctor);

    let result = run_event_loop(&mut terminal, &mut tui).await;

    // Always restore the terminal.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    tui: &mut DoctorTui,
) -> Result<()> {
    loop {
        let message_area_height = terminal.size()?.height.saturating_sub(4); // 3 input + 1 status
        tui.scroll_to_bottom(message_area_height);

        terminal.draw(|frame| tui.render(frame))?;

        if tui.should_exit {
            break;
        }

        // Poll for input with a short timeout so the loop stays responsive.
        if event::poll(Duration::from_millis(50))? {
            if let CrosstermEvent::Key(key) = event::read()? {
                // Ctrl+C exits
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')
                {
                    tui.should_exit = true;
                    continue;
                }

                match key.code {
                    KeyCode::Enter => {
                        let text = tui.input_buffer.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }

                        // Handle /quit command
                        if text == "/quit" || text == "/exit" {
                            tui.should_exit = true;
                            continue;
                        }

                        tui.input_buffer.clear();
                        tui.input_cursor = 0;

                        // Push user message to display
                        tui.messages.push((Role::User, text.clone()));
                        tui.thinking = true;
                        tui.status = "Thinking…".to_string();

                        // Render "thinking" state before awaiting the model
                        tui.scroll_to_bottom(message_area_height);
                        terminal.draw(|frame| tui.render(frame))?;

                        // Call the model — build history slice for the chat call
                        let history: Vec<(Role, String)> = tui.messages
                            [..tui.messages.len().saturating_sub(1)]
                            .iter()
                            .map(|(r, c)| (r.clone(), c.clone()))
                            .collect();

                        match tui.doctor.chat(&history, &text).await {
                            Ok(response) => {
                                tui.messages.push((Role::Assistant, response));
                                tui.status = "Type a message and press Enter. /quit or Ctrl+C to exit.".to_string();
                            }
                            Err(e) => {
                                tui.status = format!("Error: {e}");
                            }
                        }
                        tui.thinking = false;
                    }

                    KeyCode::Backspace => tui.handle_backspace(),

                    KeyCode::Char(c) => tui.handle_char(c),

                    KeyCode::Up => {
                        tui.scroll = tui.scroll.saturating_sub(1);
                    }

                    KeyCode::Down => {
                        tui.scroll += 1;
                    }

                    KeyCode::PageUp => {
                        tui.scroll = tui.scroll.saturating_sub(10);
                    }

                    KeyCode::PageDown => {
                        tui.scroll += 10;
                    }

                    _ => {}
                }
            }
        }
    }

    Ok(())
}
