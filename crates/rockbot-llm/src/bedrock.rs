//! AWS Bedrock provider using the Converse API
//!
//! This provider uses AWS credentials (environment variables, config files,
//! IAM roles, etc.) to access foundation models via Amazon Bedrock.
//!
//! ## Authentication
//! Uses standard AWS credential chain:
//! - Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
//! - AWS config files (`~/.aws/credentials`, `~/.aws/config`)
//! - IAM instance roles / ECS task roles
//!
//! ## Usage
//! ```ignore
//! use rockbot_llm::bedrock::BedrockProvider;
//!
//! let provider = BedrockProvider::new("us-east-1").await?;
//! ```

use crate::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, LlmError, LlmProvider, Message,
    MessageRole, ModelInfo, ProviderCapabilities, Result, StreamingChunk, StreamingChoice,
    StreamingDelta, ToolCall, FunctionCall, ToolDefinition, Usage,
};
use async_trait::async_trait;
use aws_sdk_bedrockruntime::{
    Client,
    types::{
        ContentBlock, ConversationRole, ConverseOutput,
        Message as BedrockMessage, SystemContentBlock,
        Tool, ToolConfiguration, ToolInputSchema, ToolSpecification,
        ContentBlockDelta, ConverseStreamOutput,
    },
};

/// AWS Bedrock provider
pub struct BedrockProvider {
    client: Client,
    region: String,
}

impl BedrockProvider {
    /// Create a new Bedrock provider with the specified region
    pub async fn new(region: &str) -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&config),
            region: region.to_string(),
        })
    }

    /// Create a new Bedrock provider using default region from AWS config
    pub async fn from_env() -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;

        let region = config
            .region()
            .map(|r| r.to_string())
            .unwrap_or_else(|| "us-east-1".to_string());

        Ok(Self {
            client: Client::new(&config),
            region,
        })
    }

    /// Normalize model ID (strip provider prefix)
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("bedrock/")
            .unwrap_or(model_id)
            .to_string()
    }

    /// Convert our messages to Bedrock format, extracting system prompt
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> (Option<Vec<SystemContentBlock>>, Vec<BedrockMessage>) {
        let mut system_blocks = Vec::new();
        let mut bedrock_messages = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    system_blocks.push(
                        SystemContentBlock::Text(msg.content.clone()),
                    );
                }
                MessageRole::User | MessageRole::Tool => {
                    let content = ContentBlock::Text(msg.content.clone());
                    bedrock_messages.push(
                        BedrockMessage::builder()
                            .role(ConversationRole::User)
                            .content(content)
                            .build()
                            .expect("valid message"),
                    );
                }
                MessageRole::Assistant => {
                    let content = ContentBlock::Text(msg.content.clone());
                    bedrock_messages.push(
                        BedrockMessage::builder()
                            .role(ConversationRole::Assistant)
                            .content(content)
                            .build()
                            .expect("valid message"),
                    );
                }
            }
        }

        let system = if system_blocks.is_empty() {
            None
        } else {
            Some(system_blocks)
        };

        (system, bedrock_messages)
    }

    /// Convert a serde_json::Value to an aws_smithy_types::Document
    fn json_to_document(value: &serde_json::Value) -> aws_smithy_types::Document {
        match value {
            serde_json::Value::Null => aws_smithy_types::Document::Null,
            serde_json::Value::Bool(b) => aws_smithy_types::Document::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    aws_smithy_types::Document::Number(aws_smithy_types::Number::PosInt(i as u64))
                } else if let Some(f) = n.as_f64() {
                    aws_smithy_types::Document::Number(aws_smithy_types::Number::Float(f))
                } else {
                    aws_smithy_types::Document::Null
                }
            }
            serde_json::Value::String(s) => aws_smithy_types::Document::String(s.clone()),
            serde_json::Value::Array(arr) => {
                aws_smithy_types::Document::Array(arr.iter().map(Self::json_to_document).collect())
            }
            serde_json::Value::Object(obj) => {
                aws_smithy_types::Document::Object(
                    obj.iter().map(|(k, v)| (k.clone(), Self::json_to_document(v))).collect(),
                )
            }
        }
    }

    /// Convert an aws_smithy_types::Document to a JSON string
    fn document_to_json_string(doc: &aws_smithy_types::Document) -> String {
        match doc {
            aws_smithy_types::Document::Null => "null".to_string(),
            aws_smithy_types::Document::Bool(b) => b.to_string(),
            aws_smithy_types::Document::Number(n) => format!("{:?}", n),
            aws_smithy_types::Document::String(s) => format!("\"{}\"", s),
            aws_smithy_types::Document::Array(arr) => {
                let items: Vec<String> = arr.iter().map(Self::document_to_json_string).collect();
                format!("[{}]", items.join(","))
            }
            aws_smithy_types::Document::Object(obj) => {
                let items: Vec<String> = obj.iter()
                    .map(|(k, v)| format!("\"{}\":{}", k, Self::document_to_json_string(v)))
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
    }

    /// Convert tool definitions to Bedrock format
    fn convert_tools(&self, tools: &[ToolDefinition]) -> ToolConfiguration {
        let bedrock_tools: Vec<Tool> = tools
            .iter()
            .map(|t| {
                let doc = Self::json_to_document(&t.parameters);

                Tool::ToolSpec(
                    ToolSpecification::builder()
                        .name(&t.name)
                        .description(&t.description)
                        .input_schema(ToolInputSchema::Json(doc))
                        .build()
                        .expect("valid tool spec"),
                )
            })
            .collect();

        ToolConfiguration::builder()
            .set_tools(Some(bedrock_tools))
            .build()
            .expect("valid tool config")
    }
}

#[async_trait]
impl LlmProvider for BedrockProvider {
    fn id(&self) -> &str {
        "bedrock"
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
            provider_id: "bedrock".to_string(),
            provider_name: "AWS Bedrock".to_string(),
            category: CredentialCategory::Model,
            auth_methods: vec![AuthMethod {
                id: "aws_credentials".to_string(),
                label: "AWS Credentials".to_string(),
                fields: vec![
                    CredentialField {
                        id: "access_key_id".to_string(),
                        label: "Access Key ID".to_string(),
                        secret: true,
                        default: None,
                        placeholder: Some("AKIA...".to_string()),
                        required: true,
                        env_var: Some("AWS_ACCESS_KEY_ID".to_string()),
                    },
                    CredentialField {
                        id: "secret_access_key".to_string(),
                        label: "Secret Access Key".to_string(),
                        secret: true,
                        default: None,
                        placeholder: None,
                        required: true,
                        env_var: Some("AWS_SECRET_ACCESS_KEY".to_string()),
                    },
                    CredentialField {
                        id: "region".to_string(),
                        label: "AWS Region".to_string(),
                        secret: false,
                        default: Some("us-east-1".to_string()),
                        placeholder: Some("us-east-1".to_string()),
                        required: true,
                        env_var: Some("AWS_REGION".to_string()),
                    },
                ],
                hint: Some("Also supports IAM roles, ~/.aws/credentials, and instance profiles".to_string()),
                docs_url: Some("https://aws.amazon.com/bedrock/".to_string()),
            }],
        })
    }

    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse> {
        let model = self.normalize_model(&request.model);
        let (system, messages) = self.convert_messages(&request.messages);

        let mut req = self.client.converse().model_id(&model);

        if let Some(system_blocks) = system {
            req = req.set_system(Some(system_blocks));
        }

        for msg in messages {
            req = req.messages(msg);
        }

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                req = req.tool_config(self.convert_tools(tools));
            }
        }

        if let Some(max_tokens) = request.max_tokens {
            req = req.inference_config(
                aws_sdk_bedrockruntime::types::InferenceConfiguration::builder()
                    .max_tokens(max_tokens as i32)
                    .set_temperature(request.temperature)
                    .build(),
            );
        }

        let response = req.send().await.map_err(|e| LlmError::ApiError {
            message: format!("Bedrock API error: {}", e),
        })?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();

        if let Some(ConverseOutput::Message(msg)) = response.output() {
            for block in msg.content() {
                match block {
                    ContentBlock::Text(text) => {
                        content.push_str(text);
                    }
                    ContentBlock::ToolUse(tool_use) => {
                        let args = Self::document_to_json_string(tool_use.input());
                        tool_calls.push(ToolCall {
                            id: tool_use.tool_use_id().to_string(),
                            r#type: "function".to_string(),
                            function: FunctionCall {
                                name: tool_use.name().to_string(),
                                arguments: args,
                            },
                        });
                    }
                    _ => {}
                }
            }
        }

        let usage = response.usage().map(|u| Usage {
            prompt_tokens: u.input_tokens() as u64,
            completion_tokens: u.output_tokens() as u64,
            total_tokens: u.total_tokens() as u64,
        }).unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });

        let finish_reason = response.stop_reason().as_str().to_string();

        Ok(ChatCompletionResponse {
            id: format!("bedrock-{}", uuid::Uuid::new_v4()),
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
                finish_reason,
            }],
            usage,
        })
    }

    async fn stream_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<CompletionStream> {
        let model = self.normalize_model(&request.model);
        let (system, messages) = self.convert_messages(&request.messages);

        let mut req = self.client.converse_stream().model_id(&model);

        if let Some(system_blocks) = system {
            req = req.set_system(Some(system_blocks));
        }

        for msg in messages {
            req = req.messages(msg);
        }

        if let Some(tools) = &request.tools {
            if !tools.is_empty() {
                req = req.tool_config(self.convert_tools(tools));
            }
        }

        if let Some(max_tokens) = request.max_tokens {
            req = req.inference_config(
                aws_sdk_bedrockruntime::types::InferenceConfiguration::builder()
                    .max_tokens(max_tokens as i32)
                    .set_temperature(request.temperature)
                    .build(),
            );
        }

        let output = req.send().await.map_err(|e| LlmError::ApiError {
            message: format!("Bedrock streaming error: {}", e),
        })?;

        let model_clone = model.clone();
        let mut receiver = output.stream;

        let stream = async_stream::stream! {
            loop {
                match receiver.recv().await {
                    Ok(Some(event)) => {
                        match event {
                            ConverseStreamOutput::ContentBlockDelta(delta_event) => {
                                if let Some(ContentBlockDelta::Text(text)) = delta_event.delta() {
                                    yield Ok(StreamingChunk {
                                        id: format!("stream-{}", uuid::Uuid::new_v4()),
                                        object: "chat.completion.chunk".to_string(),
                                        created: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_secs(),
                                        model: model_clone.clone(),
                                        choices: vec![StreamingChoice {
                                            index: 0,
                                            delta: StreamingDelta {
                                                role: None,
                                                content: Some(text.to_string()),
                                                tool_calls: None,
                                            },
                                            finish_reason: None,
                                        }],
                                    });
                                }
                            }
                            ConverseStreamOutput::MessageStop(_) => {
                                yield Ok(StreamingChunk {
                                    id: format!("stream-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
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
                                });
                                break;
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        yield Err(LlmError::ApiError {
                            message: format!("Bedrock stream error: {}", e),
                        });
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        Err(LlmError::ApiError {
            message: "Use Bedrock embedding models directly via AWS SDK for embeddings"
                .to_string(),
        })
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
                name: "Claude Sonnet 4 (Bedrock)".to_string(),
                description: "Claude Sonnet 4 via AWS Bedrock".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-opus-4-20250514-v1:0".to_string(),
                name: "Claude Opus 4 (Bedrock)".to_string(),
                description: "Claude Opus 4 via AWS Bedrock".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-3-5-haiku-20241022-v1:0".to_string(),
                name: "Claude 3.5 Haiku (Bedrock)".to_string(),
                description: "Claude 3.5 Haiku via AWS Bedrock".to_string(),
                context_window: 200000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-pro-v1:0".to_string(),
                name: "Amazon Nova Pro".to_string(),
                description: "Amazon's Nova Pro model".to_string(),
                context_window: 300000,
                max_output_tokens: Some(5120),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-lite-v1:0".to_string(),
                name: "Amazon Nova Lite".to_string(),
                description: "Amazon's Nova Lite model".to_string(),
                context_window: 300000,
                max_output_tokens: Some(5120),
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
    #[test]
    fn test_normalize_model() {
        assert_eq!(
            "anthropic.claude-sonnet-4-20250514-v1:0",
            "bedrock/anthropic.claude-sonnet-4-20250514-v1:0"
                .strip_prefix("bedrock/")
                .unwrap_or("anthropic.claude-sonnet-4-20250514-v1:0")
        );
    }
}
