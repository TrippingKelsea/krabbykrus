//! Anthropic Claude provider using Claude Code SDK
//!
//! This provider uses OAuth authentication via Claude Code CLI credentials
//! stored at `~/.claude/.credentials.json`. No API key support - OAuth only.
//!
//! ## Requirements
//! - Claude Code CLI installed: `npm install -g @anthropic-ai/claude-code`
//! - Authenticated: Run `claude` and complete OAuth flow
//!
//! ## Usage
//! ```ignore
//! use rockbot_llm::anthropic::AnthropicProvider;
//!
//! // Create provider (uses Claude Code OAuth automatically)
//! let provider = AnthropicProvider::new()?;
//!
//! // Check if credentials are available
//! if AnthropicProvider::has_credentials() {
//!     println!("Claude Code credentials found!");
//! }
//! ```

use crate::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, LlmError, LlmProvider, Message,
    MessageRole, ModelInfo, ProviderCapabilities, Result, StreamingChunk, StreamingChoice,
    StreamingDelta, Usage,
};
use async_trait::async_trait;
use claude_agent_sdk::{query, ClaudeAgentOptions, Message as SdkMessage, ContentBlock};
use futures_util::StreamExt;
use std::path::PathBuf;

/// Anthropic provider using Claude Code SDK (OAuth only)
pub struct AnthropicProvider {
    /// Default model to use
    #[allow(dead_code)]
    default_model: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider using Claude Code OAuth credentials
    pub fn new() -> Result<Self> {
        // Verify Claude Code credentials exist
        if !Self::has_credentials() {
            return Err(LlmError::AuthenticationFailed);
        }
        
        Ok(Self {
            default_model: "claude-sonnet-4-20250514".to_string(),
        })
    }
    
    /// Create provider with a specific default model
    pub fn with_model(model: impl Into<String>) -> Result<Self> {
        if !Self::has_credentials() {
            return Err(LlmError::AuthenticationFailed);
        }
        
        Ok(Self {
            default_model: model.into(),
        })
    }
    
    /// Check if Claude Code credentials exist
    pub fn has_credentials() -> bool {
        Self::credentials_path()
            .is_some_and(|p| p.exists())
    }
    
    /// Get the Claude Code credentials file path
    pub fn credentials_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
    }
    
    /// Check if credentials are valid (not expired)
    pub fn credentials_valid() -> bool {
        let Some(path) = Self::credentials_path() else {
            return false;
        };
        
        let Ok(content) = std::fs::read_to_string(&path) else {
            return false;
        };
        
        #[derive(serde::Deserialize)]
        struct Credentials {
            #[serde(rename = "claudeAiOauth")]
            oauth: Option<OAuthData>,
        }
        
        #[derive(serde::Deserialize)]
        struct OAuthData {
            #[serde(rename = "expiresAt")]
            expires_at: u64,
        }
        
        let Ok(creds) = serde_json::from_str::<Credentials>(&content) else {
            return false;
        };
        
        let Some(oauth) = creds.oauth else {
            return false;
        };
        
        // Check expiration with 5 minute buffer
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        oauth.expires_at > now + 300_000
    }
    
    /// Normalize model ID (strip provider prefix)
    #[allow(clippy::unused_self)]
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("anthropic/")
            .unwrap_or(model_id)
            .to_string()
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_embeddings: false,
            max_tokens: Some(8192),
            context_window: 200000,
        }
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "anthropic".to_string(),
            provider_name: "Anthropic".to_string(),
            category: CredentialCategory::Model,
            auth_methods: vec![
                AuthMethod {
                    id: "oauth".to_string(),
                    label: "Session Key (Claude Code)".to_string(),
                    fields: vec![],
                    hint: Some("Uses Claude Code credentials (~/.claude/.credentials.json). Install: npm i -g @anthropic-ai/claude-code, then run: claude".to_string()),
                    docs_url: Some("https://docs.anthropic.com/".to_string()),
                },
                AuthMethod {
                    id: "api_key".to_string(),
                    label: "API Key".to_string(),
                    fields: vec![
                        CredentialField {
                            id: "api_key".to_string(),
                            label: "API Key".to_string(),
                            secret: true,
                            default: None,
                            placeholder: Some("sk-ant-...".to_string()),
                            required: true,
                            env_var: Some("ANTHROPIC_API_KEY".to_string()),
                        },
                        CredentialField {
                            id: "base_url".to_string(),
                            label: "Base URL".to_string(),
                            secret: false,
                            default: Some("https://api.anthropic.com".to_string()),
                            placeholder: Some("https://api.anthropic.com".to_string()),
                            required: false,
                            env_var: None,
                        },
                    ],
                    hint: None,
                    docs_url: Some("https://console.anthropic.com/".to_string()),
                },
            ],
        })
    }

    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let model = self.normalize_model(&request.model);

        // Build the prompt from messages
        let mut system_prompt = None;
        let mut conversation = String::new();

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    system_prompt = Some(msg.content.clone());
                }
                MessageRole::User => {
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
                MessageRole::Assistant => {
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
                MessageRole::Tool => {
                    // Tool results formatted as part of conversation
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
            }
        }

        // Inject JSON mode hint into system prompt (Anthropic has no native json_mode)
        if let Some(ref response_format) = request.response_format {
            let json_hint = match response_format {
                crate::ResponseFormat::Text => None,
                crate::ResponseFormat::JsonObject => Some(
                    "IMPORTANT: You MUST respond with valid JSON only. No markdown, no explanation, no text outside the JSON object.".to_string()
                ),
                crate::ResponseFormat::JsonSchema { schema } => Some(
                    format!(
                        "IMPORTANT: You MUST respond with valid JSON conforming to this schema:\n```json\n{}\n```\nNo markdown, no explanation, no text outside the JSON object.",
                        serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
                    )
                ),
            };
            if let Some(hint) = json_hint {
                system_prompt = Some(match system_prompt {
                    Some(existing) => format!("{existing}\n\n{hint}"),
                    None => hint,
                });
            }
        }

        // Build options
        let mut options_builder = ClaudeAgentOptions::builder();

        if let Some(system) = system_prompt {
            options_builder = options_builder.system_prompt(system);
        }
        
        options_builder = options_builder.max_turns(1); // Single turn for completion
        
        let options = options_builder.build();
        
        // Query Claude Code SDK (with connection timeout)
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            query(&conversation, Some(options)),
        ).await.map_err(|_| LlmError::ApiError {
            message: "Timed out connecting to Claude Code SDK".to_string(),
        })?.map_err(|e| {
            LlmError::ApiError {
                message: format!("Claude Code SDK error: {e}"),
            }
        })?;

        let mut pinned_stream = Box::pin(stream);
        let mut response_content = String::new();
        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;

        // Collect all messages from stream (with per-message idle timeout)
        let chunk_timeout = std::time::Duration::from_secs(120);
        while let Ok(Some(message_result)) = tokio::time::timeout(
            chunk_timeout,
            pinned_stream.next(),
        ).await {
            match message_result {
                Ok(sdk_message) => {
                    match sdk_message {
                        SdkMessage::Assistant { message, .. } => {
                            // Extract text content from content blocks
                            for block in &message.content {
                                if let ContentBlock::Text { text } = block {
                                    response_content.push_str(text);
                                }
                            }
                        }
                        SdkMessage::Result { usage: Some(usage_val), .. } => {
                            // Final result with usage stats
                            input_tokens = usage_val.get("input_tokens")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            output_tokens = usage_val.get("output_tokens")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                        }
                        SdkMessage::Result { usage: None, .. } => {}
                        _ => {}
                    }
                }
                Err(e) => {
                    // Skip parse errors from unknown message types (e.g. rate_limit_event)
                    let err_msg = e.to_string();
                    if err_msg.contains("unknown variant") {
                        continue;
                    }
                    return Err(LlmError::ApiError {
                        message: format!("Stream error: {e}"),
                    });
                }
            }
        }

        Ok(ChatCompletionResponse {
            id: format!("claude-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            #[allow(clippy::unwrap_used)] // SystemTime::now() is always after UNIX_EPOCH
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: MessageRole::Assistant,
                    content: response_content,
                    images: vec![],
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: input_tokens,
                completion_tokens: output_tokens,
                total_tokens: input_tokens + output_tokens,
            },
        })
    }

    async fn stream_completion(&self, request: ChatCompletionRequest) -> Result<CompletionStream> {
        let model = self.normalize_model(&request.model);
        
        // Build the prompt from messages
        let mut system_prompt = None;
        let mut conversation = String::new();
        
        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    system_prompt = Some(msg.content.clone());
                }
                MessageRole::User => {
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
                MessageRole::Assistant => {
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
                MessageRole::Tool => {
                    if !conversation.is_empty() {
                        conversation.push_str("\n\n");
                    }
                    conversation.push_str(&msg.content);
                }
            }
        }
        
        // Build options
        let mut options_builder = ClaudeAgentOptions::builder();
        
        if let Some(system) = system_prompt {
            options_builder = options_builder.system_prompt(system);
        }
        
        let options = options_builder.build();
        let model_clone = model.clone();
        let conversation_owned = conversation.clone();
        
        // Convert to stream that owns its data
        let stream = async_stream::stream! {
            // Query Claude Code SDK inside the stream
            let sdk_stream = match query(&conversation_owned, Some(options)).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(LlmError::ApiError {
                        message: format!("Claude Code SDK error: {e}"),
                    });
                    return;
                }
            };
            
            let mut pinned = Box::pin(sdk_stream);
            
            while let Some(message_result) = pinned.next().await {
                match message_result {
                    Ok(sdk_message) => {
                        match sdk_message {
                            SdkMessage::Assistant { message, .. } => {
                                for block in &message.content {
                                    if let ContentBlock::Text { text } = block {
                                        let chunk = StreamingChunk {
                                            id: format!("stream-{}", uuid::Uuid::new_v4()),
                                            object: "chat.completion.chunk".to_string(),
                                            #[allow(clippy::unwrap_used)] // SystemTime::now() is always after UNIX_EPOCH
                                            created: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_secs(),
                                            model: model_clone.clone(),
                                            choices: vec![StreamingChoice {
                                                index: 0,
                                                delta: StreamingDelta {
                                                    role: None,
                                                    content: Some(text.clone()),
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                        };
                                        yield Ok(chunk);
                                    }
                                }
                            }
                            SdkMessage::Result { .. } => {
                                // Send final chunk with finish_reason
                                let chunk = StreamingChunk {
                                    id: format!("stream-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
                                    #[allow(clippy::unwrap_used)] // SystemTime::now() is always after UNIX_EPOCH
                                    created: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_secs(),
                                    model: model_clone.clone(),
                                    choices: vec![StreamingChoice {
                                        index: 0,
                                        delta: StreamingDelta {
                                            role: None,
                                            content: None,
                                            tool_calls: None,
                                        },
                                        finish_reason: Some("stop".to_string()),
                                    }],
                                };
                                yield Ok(chunk);
                            }
                            _ => {}
                        }
                    }
                    Err(e) => {
                        // Skip parse errors from unknown message types (e.g. rate_limit_event)
                        let err_msg = e.to_string();
                        if err_msg.contains("unknown variant") {
                            continue;
                        }
                        yield Err(LlmError::ApiError {
                            message: format!("Stream error: {e}"),
                        });
                    }
                }
            }
        };
        
        Ok(Box::pin(stream))
    }

    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        Err(LlmError::ApiError {
            message: "Anthropic does not support embeddings".to_string(),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "claude-opus-4-20250514".to_string(),
                name: "Claude Opus 4".to_string(),
                description: "Most capable Claude model".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                description: "Balanced performance and speed".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "claude-3-5-haiku-latest".to_string(),
                name: "Claude 3.5 Haiku".to_string(),
                description: "Fast and efficient".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
        ])
    }

    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo> {
        let models = self.list_models().await?;
        let normalized = self.normalize_model(model_id);

        models
            .into_iter()
            .find(|m| m.id == normalized || m.id == model_id)
            .ok_or_else(|| LlmError::ModelNotFound {
                model: model_id.to_string(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_model() {
        // Create a mock provider for testing
        let provider = AnthropicProvider {
            default_model: "test".to_string(),
        };
        
        assert_eq!(
            provider.normalize_model("anthropic/claude-3-opus"),
            "claude-3-opus"
        );
        assert_eq!(provider.normalize_model("claude-3-opus"), "claude-3-opus");
    }
    
    #[test]
    fn test_credentials_path() {
        let path = AnthropicProvider::credentials_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains(".claude"));
        assert!(path.to_string_lossy().contains(".credentials.json"));
    }
}
