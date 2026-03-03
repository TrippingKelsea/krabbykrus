//! Communication channels for Krabbykrus
//! 
//! This module provides abstractions for different communication channels
//! like Discord, Telegram, WhatsApp, etc.

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use thiserror::Error;

/// Channel errors
#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("Channel not found: {name}")]
    NotFound { name: String },
    
    #[error("Connection failed: {message}")]
    ConnectionFailed { message: String },
    
    #[error("Authentication failed")]
    AuthenticationFailed,
    
    #[error("Message send failed: {message}")]
    MessageSendFailed { message: String },
    
    #[error("Invalid message format: {message}")]
    InvalidMessageFormat { message: String },
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Channel configuration error: {message}")]
    ConfigurationError { message: String },
}

/// Result type for channel operations
pub type Result<T> = std::result::Result<T, ChannelError>;

/// Channel abstraction trait
#[async_trait]
pub trait Channel: Send + Sync {
    /// Channel identifier
    fn id(&self) -> &str;
    
    /// Channel capabilities
    fn capabilities(&self) -> ChannelCapabilities;
    
    /// Connect to the channel
    async fn connect(&mut self) -> Result<()>;
    
    /// Disconnect from the channel
    async fn disconnect(&mut self) -> Result<()>;
    
    /// Check channel health
    async fn health_check(&self) -> Result<ChannelHealth>;
    
    /// Send a message
    async fn send_message(&self, target: &str, message: ChannelMessage) -> Result<String>;
    
    /// Edit a message
    async fn edit_message(&self, message_id: &str, new_content: &str) -> Result<()>;
    
    /// Delete a message
    async fn delete_message(&self, message_id: &str) -> Result<()>;
    
    /// Get event stream
    async fn event_stream(&self) -> Result<Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>>;
    
    /// Get user information
    async fn get_user_info(&self, user_id: &str) -> Result<UserInfo>;
    
    /// Get channel information
    async fn get_channel_info(&self, channel_id: &str) -> Result<ChannelInfo>;
}

/// Channel capabilities
#[derive(Debug, Clone)]
pub struct ChannelCapabilities {
    pub supports_edit: bool,
    pub supports_delete: bool,
    pub supports_reactions: bool,
    pub supports_threads: bool,
    pub supports_media: bool,
    pub max_message_length: usize,
    pub supported_media_types: Vec<String>,
}

/// Channel health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelHealth {
    pub channel_id: String,
    pub connected: bool,
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    pub message_queue_size: usize,
    pub error_count: u32,
}

/// Channel message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelMessage {
    pub content: MessageContent,
    pub metadata: HashMap<String, serde_json::Value>,
    pub attachments: Vec<Attachment>,
}

/// Message content types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MessageContent {
    Text { text: String },
    Rich { 
        text: String, 
        embeds: Vec<Embed>,
        components: Option<Vec<Component>>,
    },
    Media { 
        url: String, 
        media_type: String,
        caption: Option<String>,
    },
}

/// Rich embed content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub color: Option<u32>,
    pub fields: Vec<EmbedField>,
    pub image: Option<EmbedImage>,
    pub thumbnail: Option<EmbedImage>,
}

/// Embed field
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

/// Embed image
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedImage {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Message component (buttons, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    pub component_type: String,
    pub label: Option<String>,
    pub custom_id: Option<String>,
    pub style: Option<String>,
    pub emoji: Option<String>,
}

/// File attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub filename: String,
    pub content_type: Option<String>,
    pub size: Option<u64>,
    pub url: Option<String>,
    pub data: Option<Vec<u8>>,
}

/// Channel event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelEvent {
    pub event_type: ChannelEventType,
    pub channel_id: String,
    pub user_id: Option<String>,
    pub message_id: Option<String>,
    pub data: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Channel event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelEventType {
    MessageReceived,
    MessageEdited,
    MessageDeleted,
    UserJoined,
    UserLeft,
    ReactionAdded,
    ReactionRemoved,
    ChannelCreated,
    ChannelDeleted,
}

/// User information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_bot: bool,
    pub is_verified: bool,
}

/// Channel information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub description: Option<String>,
    pub member_count: Option<u32>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Channel manager handles multiple channels
pub struct ChannelManager {
    channels: HashMap<String, Box<dyn Channel>>,
}

impl ChannelManager {
    /// Create a new channel manager
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
        }
    }
    
    /// Register a channel
    pub fn register_channel(&mut self, channel: Box<dyn Channel>) {
        let channel_id = channel.id().to_string();
        self.channels.insert(channel_id, channel);
    }
    
    /// Get a channel by ID
    pub fn get_channel(&self, channel_id: &str) -> Option<&dyn Channel> {
        self.channels.get(channel_id).map(|c| c.as_ref())
    }
    
    /// Get a mutable channel by ID
    pub fn get_channel_mut(&mut self, channel_id: &str) -> Option<&mut Box<dyn Channel>> {
        self.channels.get_mut(channel_id)
    }
    
    /// List all channels
    pub fn list_channels(&self) -> Vec<&str> {
        self.channels.keys().map(|k| k.as_str()).collect()
    }
    
    /// Connect all channels
    pub async fn connect_all(&mut self) -> Result<()> {
        for channel in self.channels.values_mut() {
            if let Err(e) = channel.connect().await {
                tracing::error!("Failed to connect channel {}: {}", channel.id(), e);
            }
        }
        Ok(())
    }
    
    /// Disconnect all channels
    pub async fn disconnect_all(&mut self) -> Result<()> {
        for channel in self.channels.values_mut() {
            if let Err(e) = channel.disconnect().await {
                tracing::error!("Failed to disconnect channel {}: {}", channel.id(), e);
            }
        }
        Ok(())
    }
    
    /// Get health status of all channels
    pub async fn get_health_status(&self) -> HashMap<String, ChannelHealth> {
        let mut health_map = HashMap::new();
        
        for (id, channel) in &self.channels {
            match channel.health_check().await {
                Ok(health) => {
                    health_map.insert(id.clone(), health);
                }
                Err(e) => {
                    tracing::error!("Health check failed for channel {}: {}", id, e);
                }
            }
        }
        
        health_map
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Mock channel implementation for development
pub struct MockChannel {
    id: String,
    connected: bool,
}

impl MockChannel {
    pub fn new(id: String) -> Self {
        Self {
            id,
            connected: false,
        }
    }
}

#[async_trait]
impl Channel for MockChannel {
    fn id(&self) -> &str {
        &self.id
    }
    
    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_edit: true,
            supports_delete: true,
            supports_reactions: true,
            supports_threads: false,
            supports_media: true,
            max_message_length: 2000,
            supported_media_types: vec!["image/png".to_string(), "image/jpeg".to_string()],
        }
    }
    
    async fn connect(&mut self) -> Result<()> {
        self.connected = true;
        Ok(())
    }
    
    async fn disconnect(&mut self) -> Result<()> {
        self.connected = false;
        Ok(())
    }
    
    async fn health_check(&self) -> Result<ChannelHealth> {
        Ok(ChannelHealth {
            channel_id: self.id.clone(),
            connected: self.connected,
            last_heartbeat: Some(chrono::Utc::now()),
            message_queue_size: 0,
            error_count: 0,
        })
    }
    
    async fn send_message(&self, _target: &str, message: ChannelMessage) -> Result<String> {
        tracing::debug!("Mock channel {} sending message: {:?}", self.id, message.content);
        Ok(format!("mock-message-{}", uuid::Uuid::new_v4()))
    }
    
    async fn edit_message(&self, message_id: &str, new_content: &str) -> Result<()> {
        tracing::debug!("Mock channel {} editing message {}: {}", self.id, message_id, new_content);
        Ok(())
    }
    
    async fn delete_message(&self, message_id: &str) -> Result<()> {
        tracing::debug!("Mock channel {} deleting message {}", self.id, message_id);
        Ok(())
    }
    
    async fn event_stream(&self) -> Result<Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>> {
        // Return an empty stream for mock
        use futures::stream;
        Ok(Box::pin(stream::empty()))
    }
    
    async fn get_user_info(&self, user_id: &str) -> Result<UserInfo> {
        Ok(UserInfo {
            id: user_id.to_string(),
            username: format!("user_{}", user_id),
            display_name: Some(format!("User {}", user_id)),
            avatar_url: None,
            is_bot: false,
            is_verified: false,
        })
    }
    
    async fn get_channel_info(&self, channel_id: &str) -> Result<ChannelInfo> {
        Ok(ChannelInfo {
            id: channel_id.to_string(),
            name: format!("Channel {}", channel_id),
            channel_type: "text".to_string(),
            description: Some("Mock channel for development".to_string()),
            member_count: Some(1),
            created_at: Some(chrono::Utc::now()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_mock_channel() {
        let mut channel = MockChannel::new("test".to_string());
        
        assert_eq!(channel.id(), "test");
        assert!(!channel.connected);
        
        channel.connect().await.unwrap();
        assert!(channel.connected);
        
        let health = channel.health_check().await.unwrap();
        assert!(health.connected);
    }
    
    #[tokio::test]
    async fn test_channel_manager() {
        let mut manager = ChannelManager::new();
        
        let channel = Box::new(MockChannel::new("test".to_string()));
        manager.register_channel(channel);
        
        assert_eq!(manager.list_channels(), vec!["test"]);
        assert!(manager.get_channel("test").is_some());
        assert!(manager.get_channel("nonexistent").is_none());
    }
}