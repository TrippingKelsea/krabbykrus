//! LLM provider abstraction for Krabbykrus

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;

pub mod anthropic;
pub use anthropic::AnthropicProvider;

/// LLM provider errors
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("Model not found: {model}")]
    ModelNotFound { model: String },
    
    #[error("API error: {message}")]
    ApiError { message: String },
    
    #[error("Authentication failed")]
    AuthenticationFailed,
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),
    
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result type for LLM operations
pub type Result<T> = std::result::Result<T, LlmError>;

/// LLM provider trait
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider identifier
    fn id(&self) -> &str;
    
    /// Provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;
    
    /// Chat completion
    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse>;
    
    /// Streaming chat completion
    async fn stream_completion(&self, request: ChatCompletionRequest) -> Result<CompletionStream>;
    
    /// Generate text embeddings
    async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>>;
    
    /// List available models
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    
    /// Get model information
    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo>;
}

/// Provider capabilities
#[derive(Debug, Clone)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_embeddings: bool,
    pub max_tokens: Option<u32>,
    pub context_window: u32,
}

/// Chat completion request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

/// Chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

/// Message in a chat completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Message role
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// Tool definition for function calling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Tool call in a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

/// Function call details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Choice in a chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: String,
}

/// Token usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Streaming completion response
pub struct CompletionStream {
    // Placeholder for streaming implementation
    _inner: (),
}

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub context_window: u32,
    pub max_output_tokens: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
}

/// LLM provider registry
pub struct LlmProviderRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    model_mapping: HashMap<String, String>, // model_id -> provider_id
}

impl LlmProviderRegistry {
    /// Create a new provider registry
    pub async fn new() -> Result<Self> {
        let mut registry = Self {
            providers: HashMap::new(),
            model_mapping: HashMap::new(),
        };
        
        // Register built-in providers
        registry.register_builtin_providers().await?;
        
        Ok(registry)
    }
    
    /// Register built-in providers
    async fn register_builtin_providers(&mut self) -> Result<()> {
        // Register mock provider for development
        let mock_provider = Arc::new(MockLlmProvider::new());
        self.register_provider(mock_provider).await;
        
        // Try to register Anthropic provider if API key is available
        if let Ok(anthropic) = AnthropicProvider::new() {
            self.register_provider(Arc::new(anthropic)).await;
        }
        
        Ok(())
    }
    
    /// Register a provider
    pub async fn register_provider(&mut self, provider: Arc<dyn LlmProvider>) {
        let provider_id = provider.id().to_string();
        self.providers.insert(provider_id.clone(), provider.clone());
        
        // Register model mappings
        if let Ok(models) = provider.list_models().await {
            for model in models {
                self.model_mapping.insert(model.id, provider_id.clone());
            }
        }
    }
    
    /// Get provider for a model
    pub async fn get_provider_for_model(&self, model_id: &str) -> Result<Arc<dyn LlmProvider>> {
        // Extract provider from model ID (e.g., "anthropic/claude-3-opus")
        let provider_id = if let Some(slash_pos) = model_id.find('/') {
            &model_id[..slash_pos]
        } else if let Some(provider_id) = self.model_mapping.get(model_id) {
            provider_id
        } else {
            "mock" // Default to mock provider
        };
        
        self.providers.get(provider_id)
            .cloned()
            .ok_or_else(|| {
                // Provide helpful error message
                let hint = match provider_id {
                    "anthropic" => " (hint: set ANTHROPIC_API_KEY environment variable)",
                    "openai" => " (hint: set OPENAI_API_KEY environment variable)",
                    _ => "",
                };
                LlmError::ApiError {
                    message: format!("Provider '{}' not available for model '{}'{}", provider_id, model_id, hint),
                }
            })
    }

    /// List available providers
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}

/// Mock LLM provider for development and testing
pub struct MockLlmProvider;

impl MockLlmProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn id(&self) -> &str {
        "mock"
    }
    
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: false,
            supports_tools: true,
            supports_vision: false,
            supports_embeddings: false,
            max_tokens: Some(4000),
            context_window: 128000,
        }
    }
    
    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        // Generate a mock response
        let response_content = format!(
            "Mock response to: {}",
            request.messages.last()
                .map(|m| m.content.chars().take(50).collect::<String>())
                .unwrap_or_default()
        );
        
        Ok(ChatCompletionResponse {
            id: format!("mock-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            model: request.model,
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: MessageRole::Assistant,
                    content: response_content,
                    tool_calls: None,
                },
                finish_reason: "stop".to_string(),
            }],
            usage: Usage {
                prompt_tokens: 50, // Mock values
                completion_tokens: 25,
                total_tokens: 75,
            },
        })
    }
    
    async fn stream_completion(&self, _request: ChatCompletionRequest) -> Result<CompletionStream> {
        // Mock streaming implementation
        Ok(CompletionStream { _inner: () })
    }
    
    async fn generate_embedding(&self, _text: &str) -> Result<Vec<f32>> {
        // Return mock embedding vector
        Ok(vec![0.1, 0.2, 0.3, 0.4, 0.5])
    }
    
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "mock-model".to_string(),
                name: "Mock Model".to_string(),
                description: "A mock model for development".to_string(),
                context_window: 128000,
                max_output_tokens: Some(4000),
                supports_tools: true,
                supports_vision: false,
            },
        ])
    }
    
    async fn get_model_info(&self, model_id: &str) -> Result<ModelInfo> {
        if model_id == "mock-model" {
            Ok(ModelInfo {
                id: model_id.to_string(),
                name: "Mock Model".to_string(),
                description: "A mock model for development".to_string(),
                context_window: 128000,
                max_output_tokens: Some(4000),
                supports_tools: true,
                supports_vision: false,
            })
        } else {
            Err(LlmError::ModelNotFound {
                model: model_id.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_mock_provider() {
        let provider = MockLlmProvider::new();
        
        let request = ChatCompletionRequest {
            model: "mock-model".to_string(),
            messages: vec![Message {
                role: MessageRole::User,
                content: "Hello, world!".to_string(),
                tool_calls: None,
            }],
            tools: None,
            temperature: Some(0.7),
            max_tokens: Some(100),
            stream: false,
        };
        
        let response = provider.chat_completion(request).await.unwrap();
        assert_eq!(response.choices.len(), 1);
        assert!(!response.choices[0].message.content.is_empty());
    }
    
    #[tokio::test]
    async fn test_provider_registry() {
        let registry = LlmProviderRegistry::new().await.unwrap();
        let provider = registry.get_provider_for_model("mock-model").await.unwrap();
        assert_eq!(provider.id(), "mock");
    }
}