//! Models/Providers component

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::tui::state::AppState;

/// Render the models page
pub fn render_models(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(area);

    render_provider_list(frame, chunks[0], state);
    render_provider_details(frame, chunks[1], state);
}

fn render_provider_list(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Model Providers");
    
    // Default providers if none loaded
    let default_providers = vec![
        ("Anthropic", true),
        ("OpenAI", false),
        ("Ollama", true),
        ("Bedrock", false),
    ];
    
    let items: Vec<ListItem> = if state.providers.is_empty() {
        default_providers.iter().map(|(name, _configured)| {
            ListItem::new(Line::from(vec![
                Span::styled("○ ", Style::default().fg(Color::DarkGray)),
                Span::raw(*name),
            ]))
        }).collect()
    } else {
        state.providers.iter().map(|provider| {
            let status = if provider.configured {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };
            
            ListItem::new(Line::from(vec![
                status,
                Span::raw(&provider.name),
            ]))
        }).collect()
    };

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.selected_provider));
    
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_provider_details(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Provider Configuration");

    if let Some(provider) = state.providers.get(state.selected_provider) {
        let status_text = if provider.configured { "Configured" } else { "Not Configured" };
        let status_color = if provider.configured { Color::Green } else { Color::Yellow };
        
        let mut content = vec![
            Line::from(vec![
                Span::styled("Provider: ", Style::default().fg(Color::Cyan)),
                Span::raw(&provider.name),
            ]),
            Line::from(vec![
                Span::styled("Type: ", Style::default().fg(Color::Cyan)),
                Span::raw(&provider.provider_type),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]),
        ];
        
        if let Some(url) = &provider.base_url {
            content.push(Line::from(vec![
                Span::styled("URL: ", Style::default().fg(Color::Cyan)),
                Span::raw(url),
            ]));
        }
        
        if !provider.models.is_empty() {
            content.push(Line::from(""));
            content.push(Line::from(Span::styled("Available Models:", Style::default().fg(Color::Cyan))));
            for model in &provider.models {
                content.push(Line::from(format!("  • {}", model)));
            }
        }
        
        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "[e]dit  [t]est connection",
            Style::default().fg(Color::DarkGray),
        )));
        
        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, area);
    } else {
        // Show default provider info
        let content = vec![
            Line::from(Span::styled("Anthropic", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
            Line::from(""),
            Line::from("Claude models (Opus, Sonnet, Haiku)"),
            Line::from(""),
            Line::from(vec![
                Span::styled("API Key: ", Style::default().fg(Color::Cyan)),
                Span::styled("Not configured", Style::default().fg(Color::Yellow)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Set ANTHROPIC_API_KEY or configure in vault",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        
        let paragraph = Paragraph::new(content)
            .block(block);
        frame.render_widget(paragraph, area);
    }
}
