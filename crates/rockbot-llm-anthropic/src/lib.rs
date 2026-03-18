//! Anthropic provider using the Messages API.

use async_trait::async_trait;
use futures_util::StreamExt;
use rockbot_llm::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, FunctionCall, ImageContent, LlmError,
    LlmProvider, Message, MessageRole, ModelInfo, ProviderCapabilities, ResponseFormat, Result,
    StreamingChoice, StreamingChunk, StreamingDelta, ToolCall, ToolDefinition, Usage,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    default_model: String,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<AnthropicToolResultContent>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicToolResultContent {
    Text { text: String },
}

#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorEnvelope {
    error: AnthropicErrorDetail,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorDetail {
    message: String,
}

#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    #[allow(dead_code)]
    message: Option<AnthropicResponse>,
    #[serde(default)]
    delta: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    usage: Option<AnthropicUsage>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
}

#[derive(Debug, Default)]
struct PartialToolUse {
    id: String,
    name: String,
    input_json: String,
}

impl AnthropicProvider {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| LlmError::AuthenticationFailed)?;
        Ok(Self::with_config(api_key, DEFAULT_BASE_URL.to_string(), DEFAULT_MODEL.to_string()))
    }

    pub fn with_api_key(api_key: String) -> Self {
        Self::with_config(api_key, DEFAULT_BASE_URL.to_string(), DEFAULT_MODEL.to_string())
    }

    pub fn with_config(api_key: String, base_url: String, default_model: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            default_model,
        }
    }

    pub fn has_credentials() -> bool {
        std::env::var("ANTHROPIC_API_KEY").is_ok() || Self::credentials_path().is_some_and(|p| p.exists())
    }

    pub fn credentials_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".claude").join(".credentials.json"))
    }

    pub fn credentials_valid() -> bool {
        std::env::var("ANTHROPIC_API_KEY").is_ok()
    }

    #[allow(clippy::unused_self)]
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("anthropic/")
            .unwrap_or(model_id)
            .to_string()
    }

    fn build_request(&self, request: ChatCompletionRequest, stream: bool) -> Result<AnthropicRequest> {
        let model = if request.model.is_empty() {
            self.default_model.clone()
        } else {
            self.normalize_model(&request.model)
        };
        let max_tokens = request.max_tokens.unwrap_or(4096);
        let mut system_parts = Vec::new();
        let mut messages = Vec::new();
        let mut current: Option<AnthropicMessage> = None;

        for msg in request.messages {
            if matches!(msg.role, MessageRole::System) {
                if !msg.content.is_empty() {
                    system_parts.push(msg.content);
                }
                continue;
            }

            let role = match msg.role {
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System => unreachable!(),
            }
            .to_string();

            let mut blocks = Vec::new();
            if !msg.content.is_empty() {
                blocks.push(AnthropicContentBlock::Text {
                    text: msg.content.clone(),
                });
            }

            for image in msg.images {
                blocks.push(AnthropicContentBlock::Image {
                    source: AnthropicImageSource {
                        source_type: "base64".to_string(),
                        media_type: image.media_type,
                        data: image.data,
                    },
                });
            }

            if matches!(msg.role, MessageRole::Assistant) {
                if let Some(tool_calls) = msg.tool_calls {
                    for tool_call in tool_calls {
                        let input = serde_json::from_str(&tool_call.function.arguments)
                            .unwrap_or_else(|_| serde_json::json!({}));
                        blocks.push(AnthropicContentBlock::ToolUse {
                            id: tool_call.id,
                            name: tool_call.function.name,
                            input,
                        });
                    }
                }
            } else if matches!(msg.role, MessageRole::Tool) {
                blocks.push(AnthropicContentBlock::ToolResult {
                    tool_use_id: msg.tool_call_id.unwrap_or_default(),
                    content: vec![AnthropicToolResultContent::Text {
                        text: msg.content,
                    }],
                });
            }

            if blocks.is_empty() {
                continue;
            }

            match &mut current {
                Some(existing) if existing.role == role => existing.content.extend(blocks),
                Some(existing) => {
                    messages.push(existing.clone());
                    current = Some(AnthropicMessage { role, content: blocks });
                }
                None => current = Some(AnthropicMessage { role, content: blocks }),
            }
        }

        if let Some(existing) = current {
            messages.push(existing);
        }

        let tools = request.tools.map(|tools| {
            tools
                .into_iter()
                .map(|tool: ToolDefinition| AnthropicTool {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.parameters,
                })
                .collect()
        });

        let system = if system_parts.is_empty() {
            request.response_format.as_ref().and_then(json_mode_hint)
        } else {
            let mut system = system_parts.join("\n\n");
            if let Some(hint) = request.response_format.as_ref().and_then(json_mode_hint) {
                system.push_str("\n\n");
                system.push_str(&hint);
            }
            Some(system)
        };

        Ok(AnthropicRequest {
            model,
            max_tokens,
            messages,
            system,
            tools,
            temperature: request.temperature,
            stream: stream.then_some(true),
        })
    }

    async fn send_request(&self, request: &AnthropicRequest) -> Result<reqwest::Response> {
        self.client
            .post(format!("{}/v1/messages", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(request)
            .send()
            .await
            .map_err(LlmError::Request)
    }
}

fn json_mode_hint(format: &ResponseFormat) -> Option<String> {
    match format {
        ResponseFormat::Text => None,
        ResponseFormat::JsonObject => Some(
            "IMPORTANT: Return valid JSON only. Do not include markdown fences or extra prose."
                .to_string(),
        ),
        ResponseFormat::JsonSchema { schema } => Some(format!(
            "IMPORTANT: Return valid JSON only, conforming to this schema:\n{}",
            serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
        )),
    }
}

fn decode_error(status: reqwest::StatusCode, body: &str) -> LlmError {
    if let Ok(err) = serde_json::from_str::<AnthropicErrorEnvelope>(body) {
        return LlmError::ApiError {
            message: err.error.message,
        };
    }
    LlmError::ApiError {
        message: format!("HTTP {status}: {body}"),
    }
}

fn response_to_choice(response: &AnthropicResponse) -> Choice {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in &response.content {
        match block {
            AnthropicContentBlock::Text { text: chunk } => text.push_str(chunk),
            AnthropicContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id: id.clone(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                    },
                });
            }
            AnthropicContentBlock::Image { .. } | AnthropicContentBlock::ToolResult { .. } => {}
        }
    }
    Choice {
        index: 0,
        message: Message {
            role: MessageRole::Assistant,
            content: text,
            images: Vec::<ImageContent>::new(),
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            tool_call_id: None,
        },
        finish_reason: response
            .stop_reason
            .clone()
            .unwrap_or_else(|| "stop".to_string()),
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
            auth_methods: vec![AuthMethod {
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
                        default: Some(DEFAULT_BASE_URL.to_string()),
                        placeholder: Some(DEFAULT_BASE_URL.to_string()),
                        required: false,
                        env_var: None,
                    },
                ],
                hint: Some("Anthropic Messages API".to_string()),
                docs_url: Some("https://docs.anthropic.com/".to_string()),
            }],
        })
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let api_request = self.build_request(request, false)?;
        let response = self.send_request(&api_request).await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(decode_error(status, &body));
        }
        let api_response: AnthropicResponse = serde_json::from_str(&body)?;
        Ok(ChatCompletionResponse {
            id: api_response.id.clone(),
            object: "chat.completion".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            model: api_response.model.clone(),
            choices: vec![response_to_choice(&api_response)],
            usage: Usage {
                prompt_tokens: api_response.usage.input_tokens,
                completion_tokens: api_response.usage.output_tokens,
                total_tokens: api_response.usage.input_tokens + api_response.usage.output_tokens,
            },
        })
    }

    async fn stream_completion(&self, request: ChatCompletionRequest) -> Result<CompletionStream> {
        let api_request = self.build_request(request.clone(), true)?;
        let response = self.send_request(&api_request).await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(decode_error(status, &body));
        }

        let model = api_request.model.clone();
        let stream = async_stream::stream! {
            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();
            let mut tool_uses: Vec<Option<PartialToolUse>> = Vec::new();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(LlmError::Request(e));
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(event_end) = buffer.find("\n\n") {
                    let event = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();
                    let mut data_line = None;
                    for line in event.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            data_line = Some(data.to_string());
                        }
                    }
                    let Some(data) = data_line else { continue; };
                    if data == "[DONE]" {
                        return;
                    }
                    let parsed: StreamEvent = match serde_json::from_str(&data) {
                        Ok(value) => value,
                        Err(e) => {
                            yield Err(LlmError::ApiError {
                                message: format!("Failed to parse Anthropic streaming event: {e}"),
                            });
                            return;
                        }
                    };

                    match parsed.event_type.as_str() {
                        "content_block_start" => {
                            if let (Some(index), Some(AnthropicContentBlock::ToolUse { id, name, .. })) =
                                (parsed.index, parsed.content_block)
                            {
                                if tool_uses.len() <= index {
                                    tool_uses.resize_with(index + 1, || None);
                                }
                                tool_uses[index] = Some(PartialToolUse {
                                    id,
                                    name,
                                    input_json: String::new(),
                                });
                            }
                        }
                        "content_block_delta" => {
                            if let Some(delta) = parsed.delta {
                                if delta.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                                    let text = delta
                                        .get("text")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default()
                                        .to_string();
                                    yield Ok(StreamingChunk {
                                        id: format!("anthropic-stream-{}", uuid::Uuid::new_v4()),
                                        object: "chat.completion.chunk".to_string(),
                                        created: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                        model: model.clone(),
                                        choices: vec![StreamingChoice {
                                            index: 0,
                                            delta: StreamingDelta {
                                                role: None,
                                                content: Some(text),
                                                tool_calls: None,
                                            },
                                            finish_reason: None,
                                        }],
                                    });
                                } else if delta.get("type").and_then(|v| v.as_str()) == Some("input_json_delta") {
                                    if let (Some(index), Some(partial_json)) = (
                                        parsed.index,
                                        delta.get("partial_json").and_then(|v| v.as_str()),
                                    ) {
                                        if let Some(Some(tool)) = tool_uses.get_mut(index) {
                                            tool.input_json.push_str(partial_json);
                                        }
                                    }
                                }
                            }
                        }
                        "content_block_stop" => {
                            if let Some(index) = parsed.index {
                                if let Some(Some(tool)) = tool_uses.get_mut(index) {
                                    let input = serde_json::from_str::<serde_json::Value>(&tool.input_json)
                                        .unwrap_or_else(|_| serde_json::json!({}));
                                    yield Ok(StreamingChunk {
                                        id: format!("anthropic-stream-{}", uuid::Uuid::new_v4()),
                                        object: "chat.completion.chunk".to_string(),
                                        created: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                        model: model.clone(),
                                        choices: vec![StreamingChoice {
                                            index: 0,
                                            delta: StreamingDelta {
                                                role: None,
                                                content: None,
                                                tool_calls: Some(vec![ToolCall {
                                                    id: tool.id.clone(),
                                                    r#type: "function".to_string(),
                                                    function: FunctionCall {
                                                        name: tool.name.clone(),
                                                        arguments: serde_json::to_string(&input)
                                                            .unwrap_or_else(|_| "{}".to_string()),
                                                    },
                                                }]),
                                            },
                                            finish_reason: None,
                                        }],
                                    });
                                }
                            }
                        }
                        "message_delta" => {
                            let finish_reason = parsed
                                .delta
                                .as_ref()
                                .and_then(|delta| delta.get("stop_reason"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            if finish_reason.is_some() {
                                yield Ok(StreamingChunk {
                                    id: format!("anthropic-stream-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
                                    created: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                    model: model.clone(),
                                    choices: vec![StreamingChoice {
                                        index: 0,
                                        delta: StreamingDelta {
                                            role: None,
                                            content: None,
                                            tool_calls: None,
                                        },
                                        finish_reason,
                                    }],
                                });
                            }
                        }
                        "message_stop" => return,
                        "message_start" | "ping" => {}
                        _ => {}
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        Err(LlmError::ApiError {
            message: "Anthropic embedding support is not available".to_string(),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "anthropic/claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                description: "General purpose Anthropic model".to_string(),
                kind: Some("chat".to_string()),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic/claude-opus-4-1-20250805".to_string(),
                name: "Claude Opus 4.1".to_string(),
                description: "High-capability Anthropic model".to_string(),
                kind: Some("chat".to_string()),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
        ])
    }

    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo> {
        self.list_models()
            .await?
            .into_iter()
            .find(|model| model.id == model_id || model.id.ends_with(&self.normalize_model(model_id)))
            .ok_or_else(|| LlmError::ModelNotFound {
                model: model_id.to_string(),
            })
    }

    async fn is_configured(&self) -> bool {
        !self.api_key.is_empty()
    }
}
