//! Signal channel plugin for RockBot (placeholder)
//!
//! This crate provides a placeholder Signal integration. The actual implementation
//! requires signal-cli to be configured locally.

use async_trait::async_trait;
use futures::Stream;
use rockbot_channels::{
    Channel, ChannelCapabilities, ChannelError, ChannelEvent, ChannelHealth, ChannelInfo,
    ChannelMessage, Result, UserInfo,
};
use rockbot_credentials_schema::{
    AuthMethod, CredentialCategory, CredentialField, CredentialSchema,
};
use std::pin::Pin;

/// Signal channel placeholder
pub struct SignalChannel;

impl SignalChannel {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SignalChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn id(&self) -> &str {
        "signal"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            supports_edit: false,
            supports_delete: false,
            supports_reactions: true,
            supports_threads: false,
            supports_media: true,
            max_message_length: 4096,
            supported_media_types: vec!["image/jpeg".to_string(), "image/png".to_string()],
        }
    }

    async fn connect(&mut self) -> Result<()> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn disconnect(&mut self) -> Result<()> {
        Ok(())
    }

    async fn health_check(&self) -> Result<ChannelHealth> {
        Ok(ChannelHealth {
            channel_id: "signal".to_string(),
            connected: false,
            last_heartbeat: None,
            message_queue_size: 0,
            error_count: 0,
        })
    }

    async fn send_message(&self, _target: &str, _message: ChannelMessage) -> Result<String> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn edit_message(&self, _message_id: &str, _new_content: &str) -> Result<()> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn delete_message(&self, _message_id: &str) -> Result<()> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn event_stream(&self) -> Result<Pin<Box<dyn Stream<Item = ChannelEvent> + Send>>> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn get_user_info(&self, _user_id: &str) -> Result<UserInfo> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    async fn get_channel_info(&self, _channel_id: &str) -> Result<ChannelInfo> {
        Err(ChannelError::ConnectionFailed {
            message: "Signal channel not yet implemented".to_string(),
        })
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "signal".to_string(),
            provider_name: "Signal".to_string(),
            category: CredentialCategory::Communication,
            auth_methods: vec![AuthMethod {
                id: "credentials".to_string(),
                label: "Signal Credentials".to_string(),
                fields: vec![CredentialField {
                    id: "phone_number".to_string(),
                    label: "Phone Number".to_string(),
                    secret: false,
                    default: None,
                    placeholder: Some("+1234567890".to_string()),
                    required: true,
                    env_var: None,
                }],
                hint: Some("Requires signal-cli configured locally".to_string()),
                docs_url: None,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_signal_credential_schema() {
        let channel = SignalChannel::new();
        let schema = channel.credential_schema().unwrap();
        assert_eq!(schema.provider_id, "signal");
        assert_eq!(schema.category, CredentialCategory::Communication);
        assert_eq!(schema.auth_methods[0].fields[0].id, "phone_number");
    }
}
