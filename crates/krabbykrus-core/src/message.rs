//! Message system for Krabbykrus
//!
//! Defines the message structures and metadata used throughout the system
//! for communication between agents, tools, and channels.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique identifier for messages
pub type MessageId = String;

/// A message in the Krabbykrus system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier
    pub id: MessageId,
    /// Message content
    pub content: MessageContent,
    /// Message metadata
    pub metadata: MessageMetadata,
    /// File attachments
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// Timestamp when message was created
    pub created_at: DateTime<Utc>,
}

/// Different types of message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessageContent {
    /// Plain text message
    Text { text: String },
    /// Rich content with formatting
    Rich { content: RichContent },
    /// Tool result
    ToolResult { result: ToolResult },
    /// System message
    System { message: String, level: SystemLevel },
    /// Error message
    Error { error: String, code: Option<String> },
}

/// Rich content with formatting and structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichContent {
    /// Structured blocks of content
    pub blocks: Vec<ContentBlock>,
}

/// A block of content within rich content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    /// Text block with optional formatting
    Text {
        text: String,
        #[serde(default)]
        formatting: TextFormatting,
    },
    /// Code block with syntax highlighting
    Code { code: String, language: Option<String> },
    /// Image block
    Image { url: String, alt: Option<String> },
    /// List block
    List {
        items: Vec<String>,
        ordered: bool,
    },
    /// Table block
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}

/// Text formatting options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextFormatting {
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub underline: bool,
    pub color: Option<String>,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResult {
    /// Successful text result
    Text { content: String },
    /// JSON data result
    Json { data: serde_json::Value },
    /// File result
    File {
        path: String,
        content: Option<Vec<u8>>,
        mime_type: Option<String>,
    },
    /// Error result
    Error {
        message: String,
        code: Option<String>,
        details: Option<serde_json::Value>,
    },
}

/// System message levels
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemLevel {
    Debug,
    Info,
    Warning,
    Error,
}

/// Message metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageMetadata {
    /// Message role in conversation
    pub role: MessageRole,
    /// Source of the message
    pub source: Option<String>,
    /// Target of the message
    pub target: Option<String>,
    /// Session ID this message belongs to
    pub session_id: Option<String>,
    /// Agent ID that processed this message
    pub agent_id: Option<String>,
    /// Channel this message came from/goes to
    pub channel: Option<String>,
    /// Additional arbitrary metadata
    #[serde(default)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Role of a message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// Message from the user/human
    #[default]
    User,
    /// Message from the AI assistant
    Assistant,
    /// System message
    System,
    /// Tool call or result
    Tool,
}

/// File attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    /// Attachment filename
    pub filename: String,
    /// MIME type
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size: Option<u64>,
    /// File data (base64 encoded for JSON)
    pub data: Option<Vec<u8>>,
    /// URL reference instead of inline data
    pub url: Option<String>,
}

/// Message builder for easier construction
pub struct MessageBuilder {
    content: Option<MessageContent>,
    metadata: MessageMetadata,
    attachments: Vec<Attachment>,
}

impl Message {
    /// Create a new message with given content
    pub fn new(content: MessageContent) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content,
            metadata: MessageMetadata::default(),
            attachments: Vec::new(),
            created_at: Utc::now(),
        }
    }
    
    /// Create a simple text message
    pub fn text<S: Into<String>>(text: S) -> Self {
        Self::new(MessageContent::Text {
            text: text.into(),
        })
    }
    
    /// Create a system message
    pub fn system<S: Into<String>>(message: S, level: SystemLevel) -> Self {
        Self::new(MessageContent::System {
            message: message.into(),
            level,
        })
    }
    
    /// Create an error message
    pub fn error<S: Into<String>>(error: S) -> Self {
        Self::new(MessageContent::Error {
            error: error.into(),
            code: None,
        })
    }
    
    /// Create a message builder
    pub fn builder() -> MessageBuilder {
        MessageBuilder {
            content: None,
            metadata: MessageMetadata::default(),
            attachments: Vec::new(),
        }
    }
    
    /// Set the session ID
    pub fn with_session_id<S: Into<String>>(mut self, session_id: S) -> Self {
        self.metadata.session_id = Some(session_id.into());
        self
    }
    
    /// Set the agent ID
    pub fn with_agent_id<S: Into<String>>(mut self, agent_id: S) -> Self {
        self.metadata.agent_id = Some(agent_id.into());
        self
    }
    
    /// Set the message role
    pub fn with_role(mut self, role: MessageRole) -> Self {
        self.metadata.role = role;
        self
    }
    
    /// Add an attachment
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }
    
    /// Extract plain text from message content
    pub fn extract_text(&self) -> Option<String> {
        match &self.content {
            MessageContent::Text { text } => Some(text.clone()),
            MessageContent::Rich { content } => {
                let mut text_parts = Vec::new();
                for block in &content.blocks {
                    match block {
                        ContentBlock::Text { text, .. } => text_parts.push(text.clone()),
                        ContentBlock::Code { code, .. } => text_parts.push(code.clone()),
                        ContentBlock::List { items, .. } => {
                            text_parts.extend(items.iter().cloned());
                        }
                        _ => {} // Skip other block types for text extraction
                    }
                }
                if text_parts.is_empty() {
                    None
                } else {
                    Some(text_parts.join("\n"))
                }
            }
            MessageContent::ToolResult { result } => match result {
                ToolResult::Text { content } => Some(content.clone()),
                ToolResult::Json { data } => Some(data.to_string()),
                _ => None,
            },
            MessageContent::System { message, .. } => Some(message.clone()),
            MessageContent::Error { error, .. } => Some(error.clone()),
        }
    }
}

impl MessageBuilder {
    /// Set the message content
    pub fn content(mut self, content: MessageContent) -> Self {
        self.content = Some(content);
        self
    }
    
    /// Set text content
    pub fn text<S: Into<String>>(self, text: S) -> Self {
        self.content(MessageContent::Text {
            text: text.into(),
        })
    }
    
    /// Set the session ID
    pub fn session_id<S: Into<String>>(mut self, session_id: S) -> Self {
        self.metadata.session_id = Some(session_id.into());
        self
    }
    
    /// Set the agent ID
    pub fn agent_id<S: Into<String>>(mut self, agent_id: S) -> Self {
        self.metadata.agent_id = Some(agent_id.into());
        self
    }
    
    /// Set the role
    pub fn role(mut self, role: MessageRole) -> Self {
        self.metadata.role = role;
        self
    }
    
    /// Add an attachment
    pub fn attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }
    
    /// Build the message
    pub fn build(self) -> Result<Message, &'static str> {
        let content = self.content.ok_or("Message content is required")?;
        
        Ok(Message {
            id: Uuid::new_v4().to_string(),
            content,
            metadata: self.metadata,
            attachments: self.attachments,
            created_at: Utc::now(),
        })
    }
}

impl ToolResult {
    /// Create a text result
    pub fn text<S: Into<String>>(content: S) -> Self {
        Self::Text {
            content: content.into(),
        }
    }
    
    /// Create a JSON result
    pub fn json(data: serde_json::Value) -> Self {
        Self::Json { data }
    }
    
    /// Create a file result
    pub fn file<S: Into<String>>(path: S) -> Self {
        Self::File {
            path: path.into(),
            content: None,
            mime_type: None,
        }
    }
    
    /// Create an error result
    pub fn error<S: Into<String>>(message: S) -> Self {
        Self::Error {
            message: message.into(),
            code: None,
            details: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_message_creation() {
        let message = Message::text("Hello, world!");
        assert!(!message.id.is_empty());
        assert!(matches!(message.content, MessageContent::Text { .. }));
        assert_eq!(message.extract_text(), Some("Hello, world!".to_string()));
    }
    
    #[test]
    fn test_message_builder() {
        let message = Message::builder()
            .text("Test message")
            .session_id("session-123")
            .agent_id("agent-456")
            .role(MessageRole::Assistant)
            .build()
            .unwrap();
        
        assert_eq!(message.metadata.session_id, Some("session-123".to_string()));
        assert_eq!(message.metadata.agent_id, Some("agent-456".to_string()));
        assert!(matches!(message.metadata.role, MessageRole::Assistant));
        assert_eq!(message.extract_text(), Some("Test message".to_string()));
    }
    
    #[test]
    fn test_tool_result() {
        let result = ToolResult::text("Success");
        assert!(matches!(result, ToolResult::Text { .. }));
        
        let json_result = ToolResult::json(serde_json::json!({"key": "value"}));
        assert!(matches!(json_result, ToolResult::Json { .. }));
    }
    
    #[test]
    fn test_rich_content_text_extraction() {
        let rich_content = RichContent {
            blocks: vec![
                ContentBlock::Text {
                    text: "Hello".to_string(),
                    formatting: TextFormatting::default(),
                },
                ContentBlock::Code {
                    code: "println!(\"world\");".to_string(),
                    language: Some("rust".to_string()),
                },
            ],
        };
        
        let message = Message::new(MessageContent::Rich {
            content: rich_content,
        });
        
        let extracted = message.extract_text().unwrap();
        assert!(extracted.contains("Hello"));
        assert!(extracted.contains("println!"));
    }
}