//! Discord channel implementation using Serenity
//!
//! This crate provides a Discord bot integration for RockBot agents.
//!
//! # Configuration
//!
//! Set `DISCORD_BOT_TOKEN` environment variable with your bot token.

use chrono::{DateTime, Utc};
use rockbot_channels::{
    Attachment, Channel, ChannelCapabilities, ChannelError, ChannelEvent, ChannelEventType,
    ChannelHealth, ChannelInfo, ChannelMessage, Embed, MessageContent, Result, UserInfo,
};
use rockbot_credentials_schema::{
    AuthMethod, CredentialCategory, CredentialField, CredentialSchema,
};

/// Convert time::OffsetDateTime (used by serenity) to chrono::DateTime<Utc>
fn serenity_time_to_chrono(ts: time::OffsetDateTime) -> DateTime<Utc> {
    DateTime::from_timestamp(ts.unix_timestamp(), ts.nanosecond()).unwrap_or_else(Utc::now)
}
use async_trait::async_trait;
use futures::Stream;
use serenity::all::{
    ChannelId, Context, CreateEmbed, CreateMessage, EditMessage, EventHandler, GatewayIntents,
    Http, Message as SerenityMessage, MessageId, Ready, UserId,
};
use serenity::Client;
use std::env;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Discord channel implementation
pub struct DiscordChannel {
    token: String,
    #[allow(dead_code)]
    client: Option<Client>,
    http: Option<Arc<Http>>,
    event_tx: mpsc::UnboundedSender<ChannelEvent>,
    event_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<ChannelEvent>>>>,
    connected: Arc<RwLock<bool>>,
    bot_user_id: Arc<RwLock<Option<u64>>>,
    /// Callback for incoming messages
    #[allow(clippy::type_complexity)]
    message_handler: Arc<RwLock<Option<Box<dyn Fn(IncomingMessage) + Send + Sync>>>>,
}

/// Incoming message from Discord
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncomingMessage {
    pub message_id: String,
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub author: UserInfo,
    pub content: String,
    pub is_dm: bool,
    pub mentions_bot: bool,
    pub attachments: Vec<Attachment>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Event handler for Discord gateway events
struct DiscordEventHandler {
    event_tx: mpsc::UnboundedSender<ChannelEvent>,
    connected: Arc<RwLock<bool>>,
    bot_user_id: Arc<RwLock<Option<u64>>>,
    #[allow(clippy::type_complexity)]
    message_handler: Arc<RwLock<Option<Box<dyn Fn(IncomingMessage) + Send + Sync>>>>,
}

#[async_trait]
impl EventHandler for DiscordEventHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        tracing::info!("Discord bot connected as {}", ready.user.name);
        *self.connected.write().await = true;
        *self.bot_user_id.write().await = Some(ready.user.id.get());
    }

    async fn message(&self, _ctx: Context, msg: SerenityMessage) {
        // Ignore messages from the bot itself
        if let Some(bot_id) = *self.bot_user_id.read().await {
            if msg.author.id.get() == bot_id {
                return;
            }
        }

        let is_dm = msg.guild_id.is_none();
        let mentions_bot = if let Some(bot_id) = *self.bot_user_id.read().await {
            msg.mentions.iter().any(|u| u.id.get() == bot_id)
        } else {
            false
        };

        let incoming = IncomingMessage {
            message_id: msg.id.to_string(),
            channel_id: msg.channel_id.to_string(),
            guild_id: msg.guild_id.map(|g| g.to_string()),
            author: UserInfo {
                id: msg.author.id.to_string(),
                username: msg.author.name.clone(),
                display_name: msg.author.global_name.clone(),
                avatar_url: msg.author.avatar_url(),
                is_bot: msg.author.bot,
                is_verified: false,
            },
            content: msg.content.clone(),
            is_dm,
            mentions_bot,
            attachments: msg
                .attachments
                .iter()
                .map(|a| Attachment {
                    filename: a.filename.clone(),
                    content_type: a.content_type.clone(),
                    size: Some(a.size as u64),
                    url: Some(a.url.clone()),
                    data: None,
                })
                .collect(),
            timestamp: serenity_time_to_chrono(*msg.timestamp),
        };

        let _ = self.event_tx.send(ChannelEvent {
            event_type: ChannelEventType::MessageReceived,
            channel_id: msg.channel_id.to_string(),
            user_id: Some(msg.author.id.to_string()),
            message_id: Some(msg.id.to_string()),
            data: serde_json::to_value(&incoming).unwrap_or_default(),
            timestamp: serenity_time_to_chrono(*msg.timestamp),
        });

        if let Some(handler) = self.message_handler.read().await.as_ref() {
            handler(incoming);
        }
    }
}

impl DiscordChannel {
    /// Create a new Discord channel with the given bot token
    pub fn new(token: impl Into<String>) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Self {
            token: token.into(),
            client: None,
            http: None,
            event_tx,
            event_rx: Arc::new(RwLock::new(Some(event_rx))),
            connected: Arc::new(RwLock::new(false)),
            bot_user_id: Arc::new(RwLock::new(None)),
            message_handler: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a schema-only instance (no real connection, just provides credential_schema)
    pub fn default_schema() -> Self {
        Self::new("__schema_only__")
    }

    /// Create from environment variable
    pub fn from_env() -> Result<Self> {
        let token =
            env::var("DISCORD_BOT_TOKEN").map_err(|_| ChannelError::ConfigurationError {
                message: "DISCORD_BOT_TOKEN environment variable not set".to_string(),
            })?;
        Ok(Self::new(token))
    }

    /// Set a callback for incoming messages
    pub async fn set_message_handler<F>(&self, handler: F)
    where
        F: Fn(IncomingMessage) + Send + Sync + 'static,
    {
        *self.message_handler.write().await = Some(Box::new(handler));
    }

    /// Get the HTTP client for direct API calls
    pub fn http(&self) -> Option<Arc<Http>> {
        self.http.clone()
    }

    /// Send a message to a specific channel
    pub async fn send_to_channel(&self, channel_id: u64, content: &str) -> Result<SerenityMessage> {
        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);
        let message = channel
            .send_message(&http, CreateMessage::new().content(content))
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: e.to_string(),
            })?;

        Ok(message)
    }

    /// Send a rich embed to a channel
    pub async fn send_embed(&self, channel_id: u64, embed: &Embed) -> Result<SerenityMessage> {
        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);

        let mut serenity_embed = CreateEmbed::new();

        if let Some(title) = &embed.title {
            serenity_embed = serenity_embed.title(title);
        }
        if let Some(description) = &embed.description {
            serenity_embed = serenity_embed.description(description);
        }
        if let Some(color) = embed.color {
            serenity_embed = serenity_embed.color(color);
        }

        for field in &embed.fields {
            serenity_embed = serenity_embed.field(&field.name, &field.value, field.inline);
        }

        let message = channel
            .send_message(&http, CreateMessage::new().embed(serenity_embed))
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: e.to_string(),
            })?;

        Ok(message)
    }

    /// Reply to a message
    pub async fn reply(
        &self,
        channel_id: u64,
        message_id: u64,
        content: &str,
    ) -> Result<SerenityMessage> {
        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);
        let message = channel
            .send_message(
                &http,
                CreateMessage::new()
                    .content(content)
                    .reference_message((channel, MessageId::new(message_id))),
            )
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: e.to_string(),
            })?;

        Ok(message)
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn id(&self) -> &str {
        "discord"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_edit: true,
            supports_delete: true,
            supports_reactions: true,
            supports_threads: true,
            supports_media: true,
            max_message_length: 2000,
            supported_media_types: vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/gif".to_string(),
                "image/webp".to_string(),
                "video/mp4".to_string(),
                "audio/mpeg".to_string(),
            ],
        }
    }

    async fn connect(&mut self) -> Result<()> {
        let intents = GatewayIntents::GUILD_MESSAGES
            | GatewayIntents::DIRECT_MESSAGES
            | GatewayIntents::MESSAGE_CONTENT
            | GatewayIntents::GUILDS;

        let handler = DiscordEventHandler {
            event_tx: self.event_tx.clone(),
            connected: self.connected.clone(),
            bot_user_id: self.bot_user_id.clone(),
            message_handler: self.message_handler.clone(),
        };

        let client = Client::builder(&self.token, intents)
            .event_handler(handler)
            .await
            .map_err(|e| ChannelError::ConnectionFailed {
                message: e.to_string(),
            })?;

        self.http = Some(client.http.clone());

        let mut client_clone = client;
        tokio::spawn(async move {
            if let Err(e) = client_clone.start().await {
                tracing::error!("Discord client error: {}", e);
            }
        });

        let mut attempts = 0;
        while !*self.connected.read().await && attempts < 50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            attempts += 1;
        }

        if !*self.connected.read().await {
            return Err(ChannelError::ConnectionFailed {
                message: "Timeout waiting for Discord connection".to_string(),
            });
        }

        tracing::info!("Discord channel connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        *self.connected.write().await = false;
        self.http = None;
        tracing::info!("Discord channel disconnected");
        Ok(())
    }

    async fn health_check(&self) -> Result<ChannelHealth> {
        Ok(ChannelHealth {
            channel_id: "discord".to_string(),
            connected: *self.connected.read().await,
            last_heartbeat: Some(chrono::Utc::now()),
            message_queue_size: 0,
            error_count: 0,
        })
    }

    async fn send_message(&self, target: &str, message: ChannelMessage) -> Result<String> {
        let channel_id: u64 = target
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: format!("Invalid channel ID: {target}"),
            })?;

        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);

        let sent_message = match message.content {
            MessageContent::Text { text } => channel
                .send_message(&http, CreateMessage::new().content(&text))
                .await
                .map_err(|e| ChannelError::MessageSendFailed {
                    message: e.to_string(),
                })?,
            MessageContent::Rich { text, embeds, .. } => {
                let mut create_message = CreateMessage::new().content(&text);

                for embed in embeds {
                    let mut serenity_embed = CreateEmbed::new();
                    if let Some(title) = &embed.title {
                        serenity_embed = serenity_embed.title(title);
                    }
                    if let Some(description) = &embed.description {
                        serenity_embed = serenity_embed.description(description);
                    }
                    if let Some(color) = embed.color {
                        serenity_embed = serenity_embed.color(color);
                    }
                    for field in &embed.fields {
                        serenity_embed =
                            serenity_embed.field(&field.name, &field.value, field.inline);
                    }
                    create_message = create_message.embed(serenity_embed);
                }

                channel
                    .send_message(&http, create_message)
                    .await
                    .map_err(|e| ChannelError::MessageSendFailed {
                        message: e.to_string(),
                    })?
            }
            MessageContent::Media { url, caption, .. } => {
                let text = if let Some(cap) = caption {
                    format!("{cap}\n{url}")
                } else {
                    url
                };
                channel
                    .send_message(&http, CreateMessage::new().content(&text))
                    .await
                    .map_err(|e| ChannelError::MessageSendFailed {
                        message: e.to_string(),
                    })?
            }
        };

        Ok(sent_message.id.to_string())
    }

    async fn edit_message(&self, message_id: &str, new_content: &str) -> Result<()> {
        let parts: Vec<&str> = message_id.split(':').collect();
        if parts.len() != 2 {
            return Err(ChannelError::InvalidMessageFormat {
                message: "Expected format: channel_id:message_id".to_string(),
            });
        }

        let channel_id: u64 = parts[0]
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: "Invalid channel ID".to_string(),
            })?;
        let msg_id: u64 = parts[1]
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: "Invalid message ID".to_string(),
            })?;

        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);
        let message = MessageId::new(msg_id);

        channel
            .edit_message(&http, message, EditMessage::new().content(new_content))
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: e.to_string(),
            })?;

        Ok(())
    }

    async fn delete_message(&self, message_id: &str) -> Result<()> {
        let parts: Vec<&str> = message_id.split(':').collect();
        if parts.len() != 2 {
            return Err(ChannelError::InvalidMessageFormat {
                message: "Expected format: channel_id:message_id".to_string(),
            });
        }

        let channel_id: u64 = parts[0]
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: "Invalid channel ID".to_string(),
            })?;
        let msg_id: u64 = parts[1]
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: "Invalid message ID".to_string(),
            })?;

        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id);
        let message = MessageId::new(msg_id);

        channel.delete_message(&http, message).await.map_err(|e| {
            ChannelError::MessageSendFailed {
                message: e.to_string(),
            }
        })?;

        Ok(())
    }

    async fn event_stream(&self) -> Result<Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>> {
        let rx = self
            .event_rx
            .write()
            .await
            .take()
            .ok_or(ChannelError::ConfigurationError {
                message: "Event stream already consumed".to_string(),
            })?;

        Ok(Box::pin(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        ))
    }

    async fn get_user_info(&self, user_id: &str) -> Result<UserInfo> {
        let user_id: u64 = user_id
            .parse()
            .map_err(|_| ChannelError::InvalidMessageFormat {
                message: "Invalid user ID".to_string(),
            })?;

        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let user = UserId::new(user_id).to_user(&http).await.map_err(|e| {
            ChannelError::MessageSendFailed {
                message: e.to_string(),
            }
        })?;

        Ok(UserInfo {
            id: user.id.to_string(),
            username: user.name.clone(),
            display_name: user.global_name.clone(),
            avatar_url: user.avatar_url(),
            is_bot: user.bot,
            is_verified: false,
        })
    }

    async fn get_channel_info(&self, channel_id: &str) -> Result<ChannelInfo> {
        let channel_id: u64 =
            channel_id
                .parse()
                .map_err(|_| ChannelError::InvalidMessageFormat {
                    message: "Invalid channel ID".to_string(),
                })?;

        let http = self.http.as_ref().ok_or(ChannelError::ConnectionFailed {
            message: "Not connected".to_string(),
        })?;

        let channel = ChannelId::new(channel_id)
            .to_channel(&http)
            .await
            .map_err(|e| ChannelError::MessageSendFailed {
                message: e.to_string(),
            })?;

        let (name, channel_type, description) = match channel {
            serenity::all::Channel::Guild(gc) => (
                gc.name.clone(),
                gc.kind.name().to_string(),
                gc.topic.clone(),
            ),
            serenity::all::Channel::Private(_pc) => ("DM".to_string(), "dm".to_string(), None),
            _ => ("Unknown".to_string(), "unknown".to_string(), None),
        };

        Ok(ChannelInfo {
            id: channel_id.to_string(),
            name,
            channel_type,
            description,
            member_count: None,
            created_at: None,
        })
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "discord".to_string(),
            provider_name: "Discord".to_string(),
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
                    env_var: Some("DISCORD_BOT_TOKEN".to_string()),
                }],
                hint: None,
                docs_url: Some("https://discord.com/developers/applications".to_string()),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_discord_channel_creation() {
        let channel = DiscordChannel::new("test-token");
        assert_eq!(channel.id(), "discord");
    }

    #[test]
    fn test_credential_schema() {
        let channel = DiscordChannel::default_schema();
        let schema = channel.credential_schema().unwrap();
        assert_eq!(schema.provider_id, "discord");
        assert_eq!(schema.category, CredentialCategory::Communication);
    }
}
