//! Ollama local model provider
//!
//! Ollama exposes an OpenAI-compatible API at `http://localhost:11434/v1`.
//! No API key is required; users configure the base URL if they run Ollama
//! on a non-default address.

use crate::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, FunctionCall, LlmError, LlmProvider,
    Message, MessageRole, ModelInfo, ProviderCapabilities, Result, StreamingChunk, ToolCall, Usage,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default Ollama base URL
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama local model provider
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

// ---------------------------------------------------------------------------
// Wire-format types (OpenAI-compatible)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OllamaFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OllamaFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaFunctionDef,
}

#[derive(Debug, Serialize)]
struct OllamaFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OllamaChoice>,
    usage: OllamaUsage,
}

#[derive(Debug, Deserialize)]
struct OllamaChoice {
    index: u32,
    message: OllamaMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OllamaUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct OllamaError {
    error: OllamaErrorDetail,
}

#[derive(Debug, Deserialize)]
struct OllamaErrorDetail {
    message: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

/// Response from `GET /api/tags`
#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OllamaModelEntry {
    name: String,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl OllamaProvider {
    /// Create a new Ollama provider targeting the default local address.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Create with an explicit base URL (e.g. `http://192.168.1.5:11434`).
    pub fn with_base_url(base_url: String) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    /// Strip the `ollama/` provider prefix from a model identifier.
    #[allow(clippy::unused_self)]
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("ollama/")
            .unwrap_or(model_id)
            .to_string()
    }

    /// Convert our `Message` into the OpenAI-compatible wire format.
    #[allow(clippy::unused_self)]
    fn convert_message(&self, msg: &Message) -> OllamaMessage {
        let role = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::System => "system",
            MessageRole::Tool => "tool",
        };

        let tool_calls = msg.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|tc| OllamaToolCall {
                    id: tc.id.clone(),
                    call_type: tc.r#type.clone(),
                    function: OllamaFunction {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                })
                .collect()
        });

        OllamaMessage {
            role: role.to_string(),
            content: Some(msg.content.clone()),
            tool_calls,
            tool_call_id: msg.tool_call_id.clone(),
        }
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_vision: false,
            supports_embeddings: false,
            max_tokens: None,
            context_window: 128000,
        }
    }

    fn credential_schema(&self) -> Option<CredentialSchema> {
        Some(CredentialSchema {
            provider_id: "ollama".to_string(),
            provider_name: "Ollama".to_string(),
            category: CredentialCategory::Model,
            auth_methods: vec![AuthMethod {
                id: "local".to_string(),
                label: "Local Server".to_string(),
                fields: vec![CredentialField {
                    id: "base_url".to_string(),
                    label: "Base URL".to_string(),
                    secret: false,
                    default: Some(DEFAULT_BASE_URL.to_string()),
                    placeholder: Some(DEFAULT_BASE_URL.to_string()),
                    required: false,
                    env_var: Some("OLLAMA_HOST".to_string()),
                }],
                hint: Some("Run `ollama serve` to start the local server.".to_string()),
                docs_url: Some("https://ollama.com".to_string()),
            }],
        })
    }

    async fn is_configured(&self) -> bool {
        // Probe the Ollama health endpoint — no credentials needed.
        self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let model = self.normalize_model(&request.model);

        let messages: Vec<OllamaMessage> = request
            .messages
            .iter()
            .map(|m| self.convert_message(m))
            .collect();

        let tools: Option<Vec<OllamaTool>> = request.tools.map(|t| {
            t.into_iter()
                .map(|tool| OllamaTool {
                    tool_type: "function".to_string(),
                    function: OllamaFunctionDef {
                        name: tool.name,
                        description: tool.description,
                        parameters: tool.parameters,
                    },
                })
                .collect()
        });

        let api_request = OllamaRequest {
            model: model.clone(),
            messages,
            tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: None,
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            if let Ok(error) = serde_json::from_str::<OllamaError>(&body) {
                return Err(LlmError::ApiError {
                    message: error.error.message,
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {status}: {body}"),
            });
        }

        let api_response: OllamaResponse = serde_json::from_str(&body)?;

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

        let messages: Vec<OllamaMessage> = request
            .messages
            .iter()
            .map(|m| self.convert_message(m))
            .collect();

        let tools: Option<Vec<OllamaTool>> = request.tools.map(|t| {
            t.into_iter()
                .map(|tool| OllamaTool {
                    tool_type: "function".to_string(),
                    function: OllamaFunctionDef {
                        name: tool.name,
                        description: tool.description,
                        parameters: tool.parameters,
                    },
                })
                .collect()
        });

        let api_request = OllamaRequest {
            model: model.clone(),
            messages,
            tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: Some(true),
        };

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Content-Type", "application/json")
            .json(&api_request)
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let body = response.text().await?;
            if let Ok(error) = serde_json::from_str::<OllamaError>(&body) {
                return Err(LlmError::ApiError {
                    message: error.error.message,
                });
            }
            return Err(LlmError::ApiError {
                message: format!("HTTP {status}: {body}"),
            });
        }

        let stream = async_stream::stream! {
            use futures_util::StreamExt;

            let mut byte_stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(LlmError::Request(e));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Consume complete SSE events (terminated by "\n\n")
                while let Some(event_end) = buffer.find("\n\n") {
                    let event_data = buffer[..event_end].to_string();
                    buffer = buffer[event_end + 2..].to_string();

                    // Extract the payload after "data: "
                    let data = match parse_sse_data(&event_data) {
                        Some(d) => d,
                        None => continue,
                    };

                    if data == "[DONE]" {
                        return;
                    }

                    match serde_json::from_str::<StreamingChunk>(&data) {
                        Ok(mut parsed) => {
                            parsed.model = model.clone();
                            yield Ok(parsed);
                        }
                        Err(e) => {
                            yield Err(LlmError::ApiError {
                                message: format!("Failed to parse streaming chunk: {e}"),
                            });
                            return;
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        Err(LlmError::ApiError {
            message: "Ollama embedding support is not yet implemented in RockBot".to_string(),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let response = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await;

        let tags: Vec<String> = match response {
            Ok(r) if r.status().is_success() => {
                match r.json::<OllamaTagsResponse>().await {
                    Ok(tags) => tags.models.into_iter().map(|m| m.name).collect(),
                    Err(_) => vec![],
                }
            }
            _ => vec![],
        };

        if tags.is_empty() {
            // Return a placeholder when Ollama is not reachable
            return Ok(vec![ModelInfo {
                id: "ollama/llama3".to_string(),
                name: "Llama 3 (example)".to_string(),
                description: "Pull with: ollama pull llama3".to_string(),
                context_window: 8192,
                max_output_tokens: None,
                supports_tools: false,
                supports_vision: false,
            }]);
        }

        Ok(tags
            .into_iter()
            .map(|name| ModelInfo {
                id: format!("ollama/{name}"),
                name: name.clone(),
                description: format!("Local Ollama model: {name}"),
                context_window: 128000,
                max_output_tokens: None,
                supports_tools: true,
                supports_vision: false,
            })
            .collect())
    }

    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo> {
        let models = self.list_models().await?;
        let normalized = self.normalize_model(model_id);

        models
            .into_iter()
            .find(|m| m.id == model_id || m.id == format!("ollama/{normalized}") || m.name == normalized)
            .ok_or_else(|| LlmError::ModelNotFound {
                model: model_id.to_string(),
            })
    }
}

/// Extract the payload string from an SSE `data:` line.
fn parse_sse_data(event: &str) -> Option<String> {
    for line in event.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            return Some(data.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_provider_name() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.id(), "ollama");
    }

    #[test]
    fn test_credential_schema() {
        let provider = OllamaProvider::new();
        let schema = provider.credential_schema().unwrap();
        assert_eq!(schema.provider_id, "ollama");
        assert_eq!(schema.provider_name, "Ollama");
        assert!(!schema.auth_methods.is_empty());
        let fields = &schema.auth_methods[0].fields;
        assert!(fields.iter().any(|f| f.id == "base_url"));
    }

    #[test]
    fn test_with_base_url_strips_trailing_slash() {
        let provider = OllamaProvider::with_base_url("http://localhost:11434/".to_string());
        assert_eq!(provider.base_url, "http://localhost:11434");
    }

    #[test]
    fn test_normalize_model() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.normalize_model("ollama/llama3"), "llama3");
        assert_eq!(provider.normalize_model("llama3"), "llama3");
    }

    #[test]
    fn test_convert_message_user() {
        let provider = OllamaProvider::new();
        let msg = Message {
            role: MessageRole::User,
            content: "Hello".to_string(),
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
        };
        let converted = provider.convert_message(&msg);
        assert_eq!(converted.role, "user");
        assert_eq!(converted.content, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_sse_data_found() {
        let event = "data: {\"id\":\"1\"}";
        assert_eq!(parse_sse_data(event), Some("{\"id\":\"1\"}".to_string()));
    }

    #[test]
    fn test_parse_sse_data_done() {
        let event = "data: [DONE]";
        assert_eq!(parse_sse_data(event), Some("[DONE]".to_string()));
    }

    #[test]
    fn test_parse_sse_data_missing() {
        let event = "event: ping";
        assert_eq!(parse_sse_data(event), None);
    }
}
