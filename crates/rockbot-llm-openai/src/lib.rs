//! OpenAI GPT API provider

use async_trait::async_trait;
use rockbot_llm::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, FunctionCall, LlmError, LlmProvider,
    Message, MessageRole, ModelInfo, ProviderCapabilities, ResponseFormat, Result, StreamingChunk,
    ToolCall, Usage,
};
use serde::{Deserialize, Serialize};
use std::env;
use zeroize::Zeroizing;

/// OpenAI API provider
pub struct OpenAiProvider {
    client: reqwest::Client,
    api_key: Zeroizing<String>,
    base_url: String,
}

/// OpenAI API request format
#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiOutMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<OpenAiResponseFormat>,
}

/// OpenAI response_format field
#[derive(Debug, Serialize)]
struct OpenAiResponseFormat {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<serde_json::Value>,
}

/// A single content part for multi-modal OpenAI messages.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OpenAiContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Debug, Serialize)]
struct OpenAiImageUrl {
    /// Data URI: `data:<media_type>;base64,<data>` or a plain URL.
    url: String,
}

/// Content field for an outgoing OpenAI message – either plain text or a
/// multi-part array (used when images are present).
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OpenAiMessageContent {
    Text(Option<String>),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Outgoing message with optional multi-part content.
#[derive(Debug, Serialize)]
struct OpenAiOutMessage {
    role: String,
    content: OpenAiMessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAiFunctionDef,
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// OpenAI API response format
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAiChoice>,
    usage: OpenAiUsage,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    index: u32,
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OpenAiError {
    error: OpenAiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorDetail {
    message: String,
    #[serde(rename = "type")]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

/// OpenAI models list response
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelInfo>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct OpenAiModelInfo {
    id: String,
    owned_by: String,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider from environment variable
    pub fn new() -> Result<Self> {
        let api_key = env::var("OPENAI_API_KEY").map_err(|_| LlmError::AuthenticationFailed)?;

        Ok(Self {
            client: Self::build_client(),
            api_key: Zeroizing::new(api_key),
            base_url: "https://api.openai.com".to_string(),
        })
    }

    /// Create with explicit API key
    pub fn with_api_key(api_key: String) -> Self {
        Self {
            client: Self::build_client(),
            api_key: Zeroizing::new(api_key),
            base_url: "https://api.openai.com".to_string(),
        }
    }

    /// Create with explicit API key and custom base URL (for Azure OpenAI, etc.)
    pub fn with_config(api_key: String, base_url: String) -> Self {
        Self {
            client: Self::build_client(),
            api_key: Zeroizing::new(api_key),
            base_url,
        }
    }

    /// Build an HTTP client with sensible timeouts to prevent indefinite hangs.
    fn build_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// Extract model name from full ID (e.g., "openai/gpt-4" -> "gpt-4")
    #[allow(clippy::unused_self)]
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("openai/")
            .unwrap_or(model_id)
            .to_string()
    }

    /// Convert our message format to OpenAI wire format.
    /// When the message carries images, the content is serialised as a
    /// multi-part array; otherwise it is a plain string.
    #[allow(clippy::unused_self)]
    fn convert_message(&self, msg: &Message) -> OpenAiOutMessage {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        let tool_calls = msg.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|tc| OpenAiToolCall {
                    id: tc.id.clone(),
                    call_type: tc.r#type.clone(),
                    function: OpenAiFunction {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                })
                .collect()
        });

        let content = if msg.images.is_empty() {
            OpenAiMessageContent::Text(Some(msg.content.clone()))
        } else {
            let mut parts = vec![OpenAiContentPart::Text {
                text: msg.content.clone(),
            }];
            for img in &msg.images {
                parts.push(OpenAiContentPart::ImageUrl {
                    image_url: OpenAiImageUrl {
                        url: format!("data:{};base64,{}", img.media_type, img.data),
                    },
                });
            }
            OpenAiMessageContent::Parts(parts)
        };

        OpenAiOutMessage {
            role: role.to_string(),
            content,
            tool_calls,
            tool_call_id: msg.tool_call_id.clone(),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            supports_embeddings: true,
            max_tokens: Some(16384),
            context_window: 128000,
        }
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            category: CredentialCategory::Model,
            auth_methods: vec![AuthMethod {
                id: "api_key".to_string(),
                label: "API Key".to_string(),
                fields: vec![
                    CredentialField {
                        id: "api_key".to_string(),
                        label: "API Key".to_string(),
                        secret: true,
                        default: None,
                        placeholder: Some("sk-...".to_string()),
                        required: true,
                        env_var: Some("OPENAI_API_KEY".to_string()),
                    },
                    CredentialField {
                        id: "base_url".to_string(),
                        label: "Base URL".to_string(),
                        secret: false,
                        default: Some("https://api.openai.com".to_string()),
                        placeholder: Some("https://api.openai.com".to_string()),
                        required: false,
                        env_var: None,
                    },
                ],
                hint: None,
                docs_url: Some("https://platform.openai.com/api-keys".to_string()),
            }],
        })
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let model = self.normalize_model(&request.model);

        let messages: Vec<OpenAiOutMessage> = request
            .messages
            .iter()
            .map(|m| self.convert_message(m))
            .collect();

        let tools: Option<Vec<OpenAiTool>> = request.tools.map(|t| {
            t.into_iter()
                .map(|tool| OpenAiTool {
                    tool_type: "function".to_string(),
                    function: OpenAiFunctionDef {
                        name: tool.name,
                        description: tool.description,
                        parameters: tool.parameters,
                    },
                })
                .collect()
        });

        let response_format = request.response_format.as_ref().map(|rf| match rf {
            ResponseFormat::Text => OpenAiResponseFormat {
                r#type: "text".to_string(),
                json_schema: None,
            },
            ResponseFormat::JsonObject => OpenAiResponseFormat {
                r#type: "json_object".to_string(),
                json_schema: None,
            },
            ResponseFormat::JsonSchema { schema } => OpenAiResponseFormat {
                r#type: "json_schema".to_string(),
                json_schema: Some(schema.clone()),
            },
        });

        let api_request = OpenAiRequest {
            model: model.clone(),
            messages,
            tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: if request.stream { Some(true) } else { None },
            response_format,
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key.as_str()))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            if let Ok(error) = serde_json::from_str::<OpenAiError>(&body) {
                return Err(LlmError::ApiError {
                    message: format!(
                        "{}: {}",
                        error
                            .error
                            .error_type
                            .unwrap_or_else(|| "error".to_string()),
                        error.error.message
                    ),
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {status}: {body}"),
            });
        }

        let api_response: OpenAiResponse = serde_json::from_str(&body)?;

        // Convert first choice to our format
        let choice = api_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::ApiError {
                message: "No choices in response".to_string(),
            })?;

        let tool_calls: Option<Vec<ToolCall>> = choice.message.tool_calls.map(|calls| {
            calls
                .into_iter()
                .map(|tc| ToolCall {
                    id: tc.id,
                    r#type: tc.call_type,
                    function: FunctionCall {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                })
                .collect()
        });

        Ok(ChatCompletionResponse {
            id: api_response.id,
            object: api_response.object,
            created: api_response.created,
            model: api_response.model,
            choices: vec![Choice {
                index: choice.index,
                message: Message {
                    role: MessageRole::Assistant,
                    content: choice.message.content.unwrap_or_default(),
                    images: vec![],
                    tool_calls,
                    tool_call_id: None,
                },
                finish_reason: choice.finish_reason.unwrap_or_else(|| "stop".to_string()),
            }],
            usage: Usage {
                prompt_tokens: api_response.usage.prompt_tokens,
                completion_tokens: api_response.usage.completion_tokens,
                total_tokens: api_response.usage.total_tokens,
            },
        })
    }

    async fn stream_completion(&self, request: ChatCompletionRequest) -> Result<CompletionStream> {
        let model = self.normalize_model(&request.model);

        let messages: Vec<OpenAiOutMessage> = request
            .messages
            .iter()
            .map(|m| self.convert_message(m))
            .collect();

        let tools: Option<Vec<OpenAiTool>> = request.tools.map(|t| {
            t.into_iter()
                .map(|tool| OpenAiTool {
                    tool_type: "function".to_string(),
                    function: OpenAiFunctionDef {
                        name: tool.name,
                        description: tool.description,
                        parameters: tool.parameters,
                    },
                })
                .collect()
        });

        let api_request = OpenAiRequest {
            model: model.clone(),
            messages,
            tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: Some(true),
            response_format: request.response_format.as_ref().map(|rf| match rf {
                ResponseFormat::Text => OpenAiResponseFormat {
                    r#type: "text".to_string(),
                    json_schema: None,
                },
                ResponseFormat::JsonObject => OpenAiResponseFormat {
                    r#type: "json_object".to_string(),
                    json_schema: None,
                },
                ResponseFormat::JsonSchema { schema } => OpenAiResponseFormat {
                    r#type: "json_schema".to_string(),
                    json_schema: Some(schema.clone()),
                },
            }),
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key.as_str()))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let body = response.text().await?;
            if let Ok(error) = serde_json::from_str::<OpenAiError>(&body) {
                return Err(LlmError::ApiError {
                    message: format!(
                        "{}: {}",
                        error
                            .error
                            .error_type
                            .unwrap_or_else(|| "error".to_string()),
                        error.error.message
                    ),
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {status}: {body}"),
            });
        }

        // Implement proper SSE streaming for OpenAI
        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            // Get response stream
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(e) => {
                        yield Err(LlmError::Request(e));
                        return;
                    }
                };

                let chunk_str = String::from_utf8_lossy(&chunk);
                buffer.push_str(&chunk_str);

                // Process complete SSE events
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Parse SSE event
                    if let Some(data) = parse_openai_sse_event(&event_data) {
                        if data == "[DONE]" {
                            // End of stream
                            return;
                        }

                        match handle_openai_stream_event(&data, &model) {
                            Ok(Some(chunk)) => yield Ok(chunk),
                            Ok(None) => continue, // Event handled, but no chunk to yield
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        #[derive(Serialize)]
        struct EmbeddingRequest {
            model: String,
            input: String,
        }

        #[derive(Deserialize)]
        struct EmbeddingResponse {
            data: Vec<EmbeddingData>,
        }

        #[derive(Deserialize)]
        struct EmbeddingData {
            embedding: Vec<f32>,
        }

        let request = EmbeddingRequest {
            model: "text-embedding-3-small".to_string(),
            input: text.to_string(),
        };

        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key.as_str()))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            if let Ok(error) = serde_json::from_str::<OpenAiError>(&body) {
                return Err(LlmError::ApiError {
                    message: error.error.message,
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {status}: {body}"),
            });
        }

        let embedding_response: EmbeddingResponse = serde_json::from_str(&body)?;
        embedding_response
            .data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .ok_or_else(|| LlmError::ApiError {
                message: "No embedding in response".to_string(),
            })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        // Return well-known models with their capabilities
        // We could fetch from API, but the info returned is minimal
        Ok(vec![
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: "Most capable GPT-4 model with vision".to_string(),
                kind: None,
                context_window: 128000,
                max_output_tokens: Some(16384),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o Mini".to_string(),
                description: "Fast and affordable GPT-4 model".to_string(),
                kind: None,
                context_window: 128000,
                max_output_tokens: Some(16384),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "gpt-4-turbo".to_string(),
                name: "GPT-4 Turbo".to_string(),
                description: "GPT-4 Turbo with 128k context".to_string(),
                kind: None,
                context_window: 128000,
                max_output_tokens: Some(4096),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "o1".to_string(),
                name: "o1".to_string(),
                description: "Reasoning model for complex tasks".to_string(),
                kind: None,
                context_window: 200000,
                max_output_tokens: Some(100000),
                supports_tools: false,
                supports_vision: true,
            },
            ModelInfo {
                id: "o1-mini".to_string(),
                name: "o1 Mini".to_string(),
                description: "Faster reasoning model".to_string(),
                kind: None,
                context_window: 128000,
                max_output_tokens: Some(65536),
                supports_tools: false,
                supports_vision: true,
            },
            ModelInfo {
                id: "o3-mini".to_string(),
                name: "o3 Mini".to_string(),
                description: "Latest compact reasoning model".to_string(),
                kind: None,
                context_window: 200000,
                max_output_tokens: Some(100000),
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

/// Parse OpenAI Server-Sent Events (SSE) format
fn parse_openai_sse_event(event_data: &str) -> Option<String> {
    for line in event_data.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            // Skip "data: "
            return Some(data.to_string());
        }
    }
    None
}

/// Handle OpenAI-specific streaming events
fn handle_openai_stream_event(
    data: &str,
    model: &str,
) -> std::result::Result<Option<StreamingChunk>, LlmError> {
    // OpenAI returns complete streaming chunks in JSON format
    let chunk: StreamingChunk = serde_json::from_str(data).map_err(|e| LlmError::ApiError {
        message: format!("Failed to parse OpenAI streaming chunk: {e}"),
    })?;

    // Update the model in the chunk to match our request
    let mut updated_chunk = chunk;
    updated_chunk.model = model.to_string();

    Ok(Some(updated_chunk))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_normalize_model() {
        let provider = OpenAiProvider {
            client: reqwest::Client::new(),
            api_key: Zeroizing::new("test".to_string()),
            base_url: "https://api.openai.com".to_string(),
        };

        assert_eq!(provider.normalize_model("openai/gpt-4o"), "gpt-4o");
        assert_eq!(provider.normalize_model("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn test_convert_message() {
        let provider = OpenAiProvider {
            client: reqwest::Client::new(),
            api_key: Zeroizing::new("test".to_string()),
            base_url: "https://api.openai.com".to_string(),
        };

        let msg = Message {
            role: MessageRole::User,
            content: "Hello".to_string(),
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
        };

        let converted = provider.convert_message(&msg);
        assert_eq!(converted.role, "user");
        // text-only messages use plain string content
        assert!(
            matches!(converted.content, OpenAiMessageContent::Text(Some(ref s)) if s == "Hello")
        );
    }

    #[test]
    fn test_convert_message_with_image() {
        use rockbot_llm::ImageContent;
        let provider = OpenAiProvider {
            client: reqwest::Client::new(),
            api_key: Zeroizing::new("test".to_string()),
            base_url: "https://api.openai.com".to_string(),
        };

        let msg = Message {
            role: MessageRole::User,
            content: "Describe this image".to_string(),
            images: vec![ImageContent {
                data: "abc123".to_string(),
                media_type: "image/png".to_string(),
            }],
            tool_calls: None,
            tool_call_id: None,
        };

        let converted = provider.convert_message(&msg);
        assert_eq!(converted.role, "user");
        assert!(matches!(converted.content, OpenAiMessageContent::Parts(_)));
        if let OpenAiMessageContent::Parts(parts) = &converted.content {
            assert_eq!(parts.len(), 2);
            assert!(matches!(parts[0], OpenAiContentPart::Text { .. }));
            assert!(matches!(parts[1], OpenAiContentPart::ImageUrl { .. }));
        }
    }
}
