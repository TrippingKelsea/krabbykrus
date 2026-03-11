//! Telegram channel implementation using teloxide
//!
//! This crate provides a Telegram bot integration for RockBot agents.
//!
//! # Configuration
//!
//! Set `TELEGRAM_BOT_TOKEN` environment variable with your bot token.

use rockbot_channels::{
    Channel, ChannelCapabilities, ChannelError, ChannelEvent, ChannelEventType,
    ChannelHealth, ChannelInfo, ChannelMessage, MessageContent, Result, UserInfo,
};
use rockbot_credentials_schema::{
    AuthMethod, CredentialCategory, CredentialField, CredentialSchema,
};
use async_trait::async_trait;
use chrono::Utc;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{ChatId, ParseMode, User},
    Bot,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// Telegram channel implementation
pub struct TelegramChannel {
    bot: Bot,
    token: String,
    event_tx: mpsc::UnboundedSender<ChannelEvent>,
    event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<ChannelEvent>>>>,
    connected: Arc<RwLock<bool>>,
    health: Arc<RwLock<ChannelHealth>>,
    known_chats: Arc<RwLock<HashMap<i64, TelegramChatInfo>>>,
}

#[derive(Debug, Clone)]
struct TelegramChatInfo {
    id: i64,
    title: Option<String>,
    username: Option<String>,
    chat_type: String,
    member_count: Option<i32>,
}

impl TelegramChannel {
    /// Create a new Telegram channel with the given token
    pub async fn new(token: String) -> Result<Self> {
        let bot = Bot::new(&token);

        match bot.get_me().await {
            Ok(_) => {
                info!("Telegram bot token verified successfully");
            }
            Err(e) => {
                error!("Failed to verify Telegram bot token: {}", e);
                return Err(ChannelError::AuthenticationFailed);
            }
        }

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            bot,
            token,
            event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
            connected: Arc::new(RwLock::new(false)),
            health: Arc::new(RwLock::new(ChannelHealth {
                channel_id: "telegram".to_string(),
                connected: false,
                last_heartbeat: Some(Utc::now()),
                message_queue_size: 0,
                error_count: 0,
            })),
            known_chats: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create a schema-only instance (no real connection, just provides credential_schema)
    pub fn default_schema() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            bot: Bot::new("__schema_only__"),
            token: "__schema_only__".to_string(),
            event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
            connected: Arc::new(RwLock::new(false)),
            health: Arc::new(RwLock::new(ChannelHealth {
                channel_id: "telegram".to_string(),
                connected: false,
                last_heartbeat: None,
                message_queue_size: 0,
                error_count: 0,
            })),
            known_chats: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a Telegram channel from environment variables
    pub async fn from_env() -> Result<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN")
            .map_err(|_| ChannelError::ConfigurationError {
                message: "TELEGRAM_BOT_TOKEN environment variable not set".to_string(),
            })?;

        Self::new(token).await
    }

    /// Convert Telegram user to UserInfo
    fn user_to_user_info(user: &User) -> UserInfo {
        UserInfo {
            id: user.id.0.to_string(),
            username: user.username.clone().unwrap_or_else(|| user.first_name.clone()),
            display_name: Some(format!("{} {}", user.first_name, user.last_name.as_deref().unwrap_or("")).trim().to_string()),
            avatar_url: None,
            is_bot: user.is_bot,
            is_verified: user.is_premium,
        }
    }

    /// Parse target to chat ID
    fn parse_chat_id(target: &str) -> Result<ChatId> {
        target.parse::<i64>()
            .map(ChatId)
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: format!("Invalid chat ID: {}", target),
            })
    }

    /// Convert message content to Telegram message text
    fn message_content_to_text(content: &MessageContent) -> String {
        match content {
            MessageContent::Text { text } => text.clone(),
            MessageContent::Rich { text, embeds, .. } => {
                let mut result = text.clone();

                for embed in embeds {
                    if let Some(title) = &embed.title {
                        result.push_str(&format!("\n\n*{}*", title));
                    }
                    if let Some(description) = &embed.description {
                        result.push_str(&format!("\n{}", description));
                    }
                    for field in &embed.fields {
                        result.push_str(&format!("\n\n*{}:* {}", field.name, field.value));
                    }
                }

                result
            }
            MessageContent::Media { caption, .. } => {
                caption.clone().unwrap_or_else(|| "Media file".to_string())
            }
        }
    }

    /// Start message handler loop
    async fn start_message_loop(
        bot: Bot,
        event_tx: mpsc::UnboundedSender<ChannelEvent>,
        connected: Arc<RwLock<bool>>,
        health: Arc<RwLock<ChannelHealth>>,
        known_chats: Arc<RwLock<HashMap<i64, TelegramChatInfo>>>,
    ) {
        info!("Starting Telegram message loop");

        let mut offset = 0;

        loop {
            match bot.get_updates().offset(offset).await {
                Ok(updates) => {
                    for update in updates {
                        if let teloxide::types::UpdateKind::Message(msg) = update.kind {
                            {
                                let mut chats = known_chats.write().await;
                                let chat = &msg.chat;
                                chats.insert(chat.id.0, TelegramChatInfo {
                                    id: chat.id.0,
                                    title: chat.title().map(|s| s.to_string()),
                                    username: chat.username().map(|s| s.to_string()),
                                    chat_type: "telegram".to_string(),
                                    member_count: None,
                                });
                            }

                            let mut event_data = serde_json::Map::new();
                            event_data.insert("text".to_string(), serde_json::Value::String(msg.text().unwrap_or("").to_string()));
                            event_data.insert("chat_id".to_string(), serde_json::Value::Number(msg.chat.id.0.into()));

                            if let Some(from) = &msg.from() {
                                event_data.insert("user_id".to_string(), serde_json::Value::String(from.id.0.to_string()));
                                event_data.insert("username".to_string(), serde_json::Value::String(
                                    from.username.clone().unwrap_or_else(|| from.first_name.clone())
                                ));
                            }

                            let event = ChannelEvent {
                                event_type: ChannelEventType::MessageReceived,
                                channel_id: "telegram".to_string(),
                                user_id: msg.from().as_ref().map(|u| u.id.0.to_string()),
                                message_id: Some(msg.id.0.to_string()),
                                data: serde_json::Value::Object(event_data),
                                timestamp: Utc::now(),
                            };

                            if let Err(e) = event_tx.send(event) {
                                error!("Failed to send Telegram event: {}", e);
                                let mut health_lock = health.write().await;
                                health_lock.error_count += 1;
                            } else {
                                let mut health_lock = health.write().await;
                                health_lock.last_heartbeat = Some(Utc::now());
                            }
                        }

                        offset = update.id + 1;
                    }
                }
                Err(e) => {
                    error!("Failed to get Telegram updates: {}", e);
                    *connected.write().await = false;
                    let mut health_lock = health.write().await;
                    health_lock.connected = false;
                    health_lock.error_count += 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn id(&self) -> &str {
        "telegram"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_edit: true,
            supports_delete: true,
            supports_reactions: false,
            supports_threads: false,
            supports_media: true,
            max_message_length: 4096,
            supported_media_types: vec![
                "image/jpeg".to_string(),
                "image/png".to_string(),
                "image/gif".to_string(),
                "video/mp4".to_string(),
                "audio/mpeg".to_string(),
                "application/pdf".to_string(),
            ],
        }
    }

    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Telegram");

        let bot = self.bot.clone();
        let event_tx = self.event_tx.clone();
        let connected = self.connected.clone();
        let health = self.health.clone();
        let known_chats = self.known_chats.clone();

        tokio::spawn(async move {
            Self::start_message_loop(bot, event_tx, connected, health, known_chats).await;
        });

        *self.connected.write().await = true;
        let mut health = self.health.write().await;
        health.connected = true;
        health.last_heartbeat = Some(Utc::now());

        info!("Connected to Telegram successfully");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from Telegram");
        *self.connected.write().await = false;
        let mut health = self.health.write().await;
        health.connected = false;
        info!("Disconnected from Telegram");
        Ok(())
    }

    async fn health_check(&self) -> Result<ChannelHealth> {
        let health = self.health.read().await;
        Ok(health.clone())
    }

    async fn send_message(&self, target: &str, message: ChannelMessage) -> Result<String> {
        let chat_id = Self::parse_chat_id(target)?;
        let text = Self::message_content_to_text(&message.content);

        debug!("Sending Telegram message to {}: {}", target, text);

        let sent_message = self.bot
            .send_message(chat_id, &text)
            .parse_mode(ParseMode::MarkdownV2)
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: format!("Failed to send Telegram message: {}", e),
            })?;

        Ok(sent_message.id.0.to_string())
    }

    async fn edit_message(&self, message_id: &str, new_content: &str) -> Result<()> {
        let _msg_id: i32 = message_id.parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: format!("Invalid message ID: {}", message_id),
            })?;

        warn!("Telegram message editing requires chat ID - message ID {} cannot be edited without chat context", message_id);

        Err(ChannelError::MessageSendFailed {
            message: "Telegram message editing requires chat context".to_string(),
        })
    }

    async fn delete_message(&self, message_id: &str) -> Result<()> {
        let _msg_id: i32 = message_id.parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: format!("Invalid message ID: {}", message_id),
            })?;

        warn!("Telegram message deletion requires chat ID - message ID {} cannot be deleted without chat context", message_id);

        Err(ChannelError::MessageSendFailed {
            message: "Telegram message deletion requires chat context".to_string(),
        })
    }

    async fn event_stream(&self) -> Result<Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>> {
        let mut rx_guard = self.event_rx.write().await;
        if let Some(rx) = rx_guard.take() {
            use tokio_stream::wrappers::UnboundedReceiverStream;
            Ok(Box::pin(UnboundedReceiverStream::new(rx)))
        } else {
            Err(ChannelError::ConnectionFailed {
                message: "Event stream already consumed".to_string(),
            })
        }
    }

    async fn get_user_info(&self, user_id: &str) -> Result<UserInfo> {
        let user_id_num: u64 = user_id.parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: format!("Invalid user ID: {}", user_id),
            })?;

        Ok(UserInfo {
            id: user_id.to_string(),
            username: format!("user_{}", user_id_num),
            display_name: Some(format!("User {}", user_id_num)),
            avatar_url: None,
            is_bot: false,
            is_verified: false,
        })
    }

    async fn get_channel_info(&self, channel_id: &str) -> Result<ChannelInfo> {
        let chat_id = Self::parse_chat_id(channel_id)?;

        match self.bot.get_chat(chat_id).await {
            Ok(chat) => {
                Ok(ChannelInfo {
                    id: chat.id.0.to_string(),
                    name: chat.title().unwrap_or("Private Chat").to_string(),
                    channel_type: "telegram".to_string(),
                    description: chat.description().map(|s| s.to_string()),
                    member_count: None,
                    created_at: None,
                })
            }
            Err(e) => {
                Err(ChannelError::NotFound {
                    name: format!("Telegram chat {}: {}", channel_id, e),
                })
            }
        }
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "telegram".to_string(),
            provider_name: "Telegram".to_string(),
            category: CredentialCategory::Communication,
            auth_methods: vec![AuthMethod {
                id: "bot_token".to_string(),
                label: "Bot Token".to_string(),
                fields: vec![CredentialField {
                    id: "bot_token".to_string(),
                    label: "Bot Token".to_string(),
                    secret: true,
                    default: None,
                    placeholder: None,
                    required: true,
                    env_var: Some("TELEGRAM_BOT_TOKEN".to_string()),
                }],
                hint: Some("Get from @BotFather on Telegram".to_string()),
                docs_url: Some("https://core.telegram.org/bots#botfather".to_string()),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_id_parsing() {
        assert!(TelegramChannel::parse_chat_id("123456789").is_ok());
        assert!(TelegramChannel::parse_chat_id("-123456789").is_ok());
        assert!(TelegramChannel::parse_chat_id("invalid").is_err());
    }

    #[test]
    fn test_message_content_conversion() {
        let content = MessageContent::Text {
            text: "Hello, world!".to_string(),
        };
        let text = TelegramChannel::message_content_to_text(&content);
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn test_credential_schema() {
        let channel = TelegramChannel::default_schema();
        let schema = channel.credential_schema().unwrap();
        assert_eq!(schema.provider_id, "telegram");
        assert_eq!(schema.category, CredentialCategory::Communication);
    }
}
