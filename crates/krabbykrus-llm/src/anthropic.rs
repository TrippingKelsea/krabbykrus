//! Anthropic Claude API provider

use crate::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream, LlmError,
    LlmProvider, Message, MessageRole, ModelInfo, ProviderCapabilities, Result, ToolDefinition,
    Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::env;

/// Anthropic API provider
pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

/// Anthropic API request format
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Anthropic API response format
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    #[serde(rename = "type")]
    response_type: String,
    role: String,
    content: Vec<AnthropicContent>,
    model: String,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type")]
    error_type: String,
    error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new() -> Result<Self> {
        let api_key = env::var("ANTHROPIC_API_KEY").map_err(|_| LlmError::AuthenticationFailed)?;

        Ok(Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
        })
    }

    /// Create with explicit API key
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// Extract model name from full ID (e.g., "anthropic/claude-3-opus" -> "claude-3-opus")
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

    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let model = self.normalize_model(&request.model);

        // Extract system message and convert others
        let mut system_message: Option<String> = None;
        let messages: Vec<AnthropicMessage> = request
            .messages
            .iter()
            .filter_map(|m| {
                match m.role {
                    MessageRole::System => {
                        system_message = Some(m.content.clone());
                        None
                    }
                    MessageRole::User => Some(AnthropicMessage {
                        role: "user".to_string(),
                        content: m.content.clone(),
                    }),
                    MessageRole::Assistant => Some(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: m.content.clone(),
                    }),
                    MessageRole::Tool => Some(AnthropicMessage {
                        role: "user".to_string(), // Tool results come as user messages in Anthropic
                        content: m.content.clone(),
                    }),
                }
            })
            .collect();

        // Convert tools
        let tools: Option<Vec<AnthropicTool>> = request.tools.map(|t| {
            t.into_iter()
                .map(|tool| AnthropicTool {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.parameters,
                })
                .collect()
        });

        let api_request = AnthropicRequest {
            model: model.clone(),
            max_tokens: request.max_tokens.unwrap_or(4096),
            messages,
            system: system_message,
            tools,
            temperature: request.temperature,
        };

        let response = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&api_request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            // Try to parse error response
            if let Ok(error) = serde_json::from_str::<AnthropicError>(&body) {
                return Err(LlmError::ApiError {
                    message: format!("{}: {}", error.error.error_type, error.error.message),
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {}: {}", status, body),
            });
        }

        let api_response: AnthropicResponse = serde_json::from_str(&body)?;

        // Convert response to our format
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for block in api_response.content {
            match block {
                AnthropicContent::Text { text } => {
                    content.push_str(&text);
                }
                AnthropicContent::ToolUse { id, name, input } => {
                    tool_calls.push(crate::ToolCall {
                        id,
                        r#type: "function".to_string(),
                        function: crate::FunctionCall {
                            name,
                            arguments: serde_json::to_string(&input).unwrap_or_default(),
                        },
                    });
                }
            }
        }

        Ok(ChatCompletionResponse {
            id: api_response.id,
            object: "chat.completion".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: MessageRole::Assistant,
                    content,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                },
                finish_reason: api_response.stop_reason.unwrap_or_else(|| "stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: api_response.usage.input_tokens,
                completion_tokens: api_response.usage.output_tokens,
                total_tokens: api_response.usage.input_tokens + api_response.usage.output_tokens,
            },
        })
    }

    async fn stream_completion(&self, _request: ChatCompletionRequest) -> Result<CompletionStream> {
        // TODO: Implement streaming with SSE
        Err(LlmError::ApiError {
            message: "Streaming not yet implemented".to_string(),
        })
    }

    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        Err(LlmError::ApiError {
            message: "Anthropic does not support embeddings".to_string(),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        // Anthropic doesn't have a model listing API, return known models
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
        let provider = AnthropicProvider {
            client: reqwest::Client::new(),
            api_key: "test".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
        };

        assert_eq!(
            provider.normalize_model("anthropic/claude-3-opus"),
            "claude-3-opus"
        );
        assert_eq!(provider.normalize_model("claude-3-opus"), "claude-3-opus");
    }
}
