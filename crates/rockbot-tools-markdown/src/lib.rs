//! Markdown/documentation processing tool for RockBot.
//!
//! Provides markdown parsing and rendering capabilities to agents.

use rockbot_security::Capabilities;
use rockbot_tools::{Tool, ToolError, message::ToolResult, ToolExecutionContext};
use std::future::Future;
use std::pin::Pin;

/// Markdown processing tool
pub struct MarkdownTool;

impl MarkdownTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MarkdownTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for MarkdownTool {
    fn name(&self) -> &str {
        "markdown"
    }

    fn description(&self) -> &str {
        "Parse and render markdown documents"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Markdown content to process"
                },
                "operation": {
                    "type": "string",
                    "enum": ["parse", "render", "extract_headings"],
                    "description": "Operation to perform"
                }
            },
            "required": ["content", "operation"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + '_>> {
        Box::pin(async move {
            let content = params.get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidParameters {
                    message: "Missing 'content' parameter".to_string(),
                })?;

            let operation = params.get("operation")
                .and_then(|v| v.as_str())
                .unwrap_or("parse");

            match operation {
                "extract_headings" => {
                    let headings: Vec<&str> = content
                        .lines()
                        .filter(|line| line.starts_with('#'))
                        .collect();
                    Ok(ToolResult::json(serde_json::json!({
                        "headings": headings,
                    })))
                }
                _ => {
                    Ok(ToolResult::text(content))
                }
            }
        })
    }

    // No credential_schema needed — markdown processing is local
}
