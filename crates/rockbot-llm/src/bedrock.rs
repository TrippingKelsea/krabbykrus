//! AWS Bedrock provider using the Converse API
//!
//! This provider uses AWS credentials (environment variables, config files,
//! IAM roles, etc.) to access foundation models via Amazon Bedrock.
//!
//! ## Authentication
//!
//! ### Standard AWS Credentials
//! Uses the standard AWS credential chain:
//! - Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
//! - AWS config files (`~/.aws/credentials`, `~/.aws/config`)
//! - IAM instance roles / ECS task roles
//!
//! ### AgentCore Identity (OAuth2)
//! Uses Bedrock AgentCore Identity for OAuth2 credential providers:
//! - Supports 23+ built-in OAuth providers (Google, GitHub, Slack, Salesforce, etc.)
//! - OAuth2 Authorization Code Grant (USER_FEDERATION) for user-delegated access
//! - OAuth2 Client Credentials Grant for machine-to-machine auth
//! - Custom OAuth2 providers via discovery URL
//!
//! ### AgentCore Identity (API Key)
//! Uses Bedrock AgentCore Identity for API key credential providers:
//! - Store and retrieve API keys via AgentCore Token Vault
//! - Encrypted credential storage with IAM-scoped access
//!
//! ## Usage
//! ```ignore
//! use rockbot_llm::bedrock::BedrockProvider;
//!
//! // Standard AWS credentials
//! let provider = BedrockProvider::new("us-east-1").await?;
//!
//! // With AgentCore OAuth2 credential provider
//! let config = AgentCoreConfig {
//!     credential_provider_name: "my-google-provider".to_string(),
//!     auth_flow: AgentCoreAuthFlow::UserFederation,
//!     scopes: vec!["https://www.googleapis.com/auth/drive.readonly".to_string()],
//!     ..Default::default()
//! };
//! let provider = BedrockProvider::with_agentcore_oauth2("us-east-1", config).await?;
//! ```

use crate::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, LlmError, LlmProvider, Message,
    MessageRole, ModelInfo, ProviderCapabilities, Result, StreamingChunk, StreamingChoice,
    StreamingDelta, ToolCall, FunctionCall, ToolDefinition, Usage,
};
use async_trait::async_trait;
use aws_sdk_bedrockruntime::config::ProvideCredentials;
use aws_sdk_bedrockruntime::{
    Client,
    types::{
        ContentBlock, ConversationRole, ConverseOutput,
        Message as BedrockMessage, SystemContentBlock,
        Tool, ToolConfiguration, ToolInputSchema, ToolSpecification,
        ToolResultBlock, ToolResultContentBlock, ToolUseBlock,
        ContentBlockDelta, ConverseStreamOutput,
    },
};

/// AgentCore OAuth2 auth flow type
#[derive(Debug, Clone, Default)]
pub enum AgentCoreAuthFlow {
    /// OAuth2 Authorization Code Grant (3LO / USER_FEDERATION)
    /// Used for user-delegated access to third-party services
    #[default]
    UserFederation,
    /// OAuth2 Client Credentials Grant (2LO)
    /// Used for machine-to-machine authentication
    ClientCredentials,
}

impl std::fmt::Display for AgentCoreAuthFlow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserFederation => write!(f, "USER_FEDERATION"),
            Self::ClientCredentials => write!(f, "CLIENT_CREDENTIALS"),
        }
    }
}

/// Built-in OAuth2 vendor types supported by AgentCore Identity
#[derive(Debug, Clone)]
pub enum AgentCoreOAuthVendor {
    Google,
    GitHub,
    Slack,
    Salesforce,
    Microsoft,
    Sharepoint,
    Confluence,
    Jira,
    Zendesk,
    ServiceNow,
    Asana,
    Linear,
    Monday,
    Notion,
    Stripe,
    Shopify,
    HubSpot,
    Twilio,
    SendGrid,
    Zoom,
    Dropbox,
    Box,
    /// Custom OAuth2 provider with a discovery URL
    Custom {
        /// OIDC discovery URL (e.g., https://provider.com/.well-known/openid-configuration)
        discovery_url: String,
    },
}

impl AgentCoreOAuthVendor {
    /// Returns the vendor config identifier used by AgentCore API
    pub fn vendor_id(&self) -> &str {
        match self {
            Self::Google => "GoogleOauth2",
            Self::GitHub => "GithubOauth2",
            Self::Slack => "SlackOauth2",
            Self::Salesforce => "SalesforceOauth2",
            Self::Microsoft => "MicrosoftOauth2",
            Self::Sharepoint => "SharepointOauth2",
            Self::Confluence => "ConfluenceOauth2",
            Self::Jira => "JiraOauth2",
            Self::Zendesk => "ZendeskOauth2",
            Self::ServiceNow => "ServiceNowOauth2",
            Self::Asana => "AsanaOauth2",
            Self::Linear => "LinearOauth2",
            Self::Monday => "MondayOauth2",
            Self::Notion => "NotionOauth2",
            Self::Stripe => "StripeOauth2",
            Self::Shopify => "ShopifyOauth2",
            Self::HubSpot => "HubSpotOauth2",
            Self::Twilio => "TwilioOauth2",
            Self::SendGrid => "SendGridOauth2",
            Self::Zoom => "ZoomOauth2",
            Self::Dropbox => "DropboxOauth2",
            Self::Box => "BoxOauth2",
            Self::Custom { .. } => "CustomOauth2",
        }
    }
}

/// Configuration for AgentCore Identity credential providers
#[derive(Debug, Clone, Default)]
pub struct AgentCoreConfig {
    /// Name of the credential provider in AgentCore
    /// Created via `aws bedrock-agentcore-control create-oauth2-credential-provider`
    pub credential_provider_name: String,

    /// OAuth2 auth flow type (UserFederation or ClientCredentials)
    pub auth_flow: AgentCoreAuthFlow,

    /// OAuth2 scopes to request (e.g., ["https://www.googleapis.com/auth/drive.readonly"])
    pub scopes: Vec<String>,

    /// AWS Secrets Manager ARN for OAuth2 client credentials
    /// Contains client_id and client_secret for the OAuth2 application
    pub credentials_secret_arn: Option<String>,

    /// OAuth2 vendor type (Google, GitHub, Custom, etc.)
    pub vendor: Option<AgentCoreOAuthVendor>,
}

/// Authentication mode for the Bedrock provider
#[derive(Debug, Clone, Default)]
pub enum BedrockAuthMode {
    /// Standard AWS credential chain (env vars, config files, IAM roles)
    #[default]
    AwsCredentials,
    /// AgentCore Identity with OAuth2 credential provider
    AgentCoreOAuth2(AgentCoreConfig),
    /// AgentCore Identity with API key credential provider
    AgentCoreApiKey {
        /// Name of the API key credential provider in AgentCore
        credential_provider_name: String,
    },
}

/// AWS Bedrock provider
pub struct BedrockProvider {
    client: Client,
    /// Control plane client for listing models
    bedrock_client: aws_sdk_bedrock::Client,
    #[allow(dead_code)]
    region: String,
    #[allow(dead_code)]
    auth_mode: BedrockAuthMode,
    /// Stored AWS SDK config for credential probing in is_configured()
    sdk_config: aws_config::SdkConfig,
}

impl BedrockProvider {
    /// Create a new Bedrock provider with the specified region using standard AWS credentials
    pub async fn new(region: &str) -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&config),
            bedrock_client: aws_sdk_bedrock::Client::new(&config),
            region: region.to_string(),
            auth_mode: BedrockAuthMode::AwsCredentials,
            sdk_config: config,
        })
    }

    /// Create a new Bedrock provider using default region from AWS config.
    ///
    /// Always succeeds — credential validation is deferred to `is_configured()`.
    /// This ensures the provider's schema and auth forms are always visible in the UI.
    pub async fn from_env() -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;

        let region = config
            .region().map_or_else(|| "us-east-1".to_string(), std::string::ToString::to_string);

        Ok(Self {
            client: Client::new(&config),
            bedrock_client: aws_sdk_bedrock::Client::new(&config),
            region,
            auth_mode: BedrockAuthMode::AwsCredentials,
            sdk_config: config,
        })
    }

    /// Create a Bedrock provider configured with AgentCore OAuth2 credential provider.
    ///
    /// This uses AWS Bedrock AgentCore Identity to manage OAuth2 tokens for third-party
    /// services. The credential provider must first be created via:
    /// ```bash
    /// aws bedrock-agentcore-control create-oauth2-credential-provider \
    ///   --name "my-provider" \
    ///   --credential-provider-vendor '{"CustomOauth2": {"oauthDiscovery": {"discoveryUrl": "..."}}}' \
    ///   --credential-provider-secret-arn "arn:aws:secretsmanager:..."
    /// ```
    ///
    /// Required IAM permissions:
    /// - `bedrock-agentcore:GetResourceOauth2Token`
    /// - `secretsmanager:GetSecretValue` (for the credentials secret)
    pub async fn with_agentcore_oauth2(region: &str, config: AgentCoreConfig) -> Result<Self> {
        let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&aws_config),
            bedrock_client: aws_sdk_bedrock::Client::new(&aws_config),
            region: region.to_string(),
            auth_mode: BedrockAuthMode::AgentCoreOAuth2(config),
            sdk_config: aws_config,
        })
    }

    /// Create a Bedrock provider configured with AgentCore API key credential provider.
    ///
    /// This uses AWS Bedrock AgentCore Identity to manage API keys stored in
    /// the AgentCore Token Vault. The credential provider must first be created via:
    /// ```bash
    /// aws bedrock-agentcore-control create-api-key-credential-provider \
    ///   --name "my-api-key-provider" \
    ///   --api-key "sk-..."
    /// ```
    ///
    /// Required IAM permissions:
    /// - `bedrock-agentcore:GetApiKeyCredential`
    pub async fn with_agentcore_api_key(
        region: &str,
        credential_provider_name: &str,
    ) -> Result<Self> {
        let aws_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;

        Ok(Self {
            client: Client::new(&aws_config),
            bedrock_client: aws_sdk_bedrock::Client::new(&aws_config),
            region: region.to_string(),
            auth_mode: BedrockAuthMode::AgentCoreApiKey {
                credential_provider_name: credential_provider_name.to_string(),
            },
            sdk_config: aws_config,
        })
    }

    /// Get the current auth mode
    pub fn auth_mode(&self) -> &BedrockAuthMode {
        &self.auth_mode
    }

    /// List models from the Bedrock ListFoundationModels API.
    ///
    /// Filters to models that support the Converse API (ON_DEMAND inference)
    /// and are actively available.
    async fn list_models_from_api(&self) -> Result<Vec<ModelInfo>> {
        use aws_sdk_bedrock::types::InferenceType;

        let resp = self.bedrock_client
            .list_foundation_models()
            .by_inference_type(InferenceType::OnDemand)
            .send()
            .await
            .map_err(|e| LlmError::ApiError {
                message: format!("Failed to list Bedrock models: {e}"),
            })?;

        let mut models = Vec::new();
        for summary in resp.model_summaries() {
            let model_id: &str = summary.model_id();
            let model_name = summary.model_name().unwrap_or(model_id);
            let provider_name = summary.provider_name().unwrap_or("Unknown");

            // Skip models that don't support streaming (basic filter for Converse-compatible)
            let supports_streaming = summary.response_streaming_supported().unwrap_or(false);

            // Extract capabilities from input modalities
            let supports_vision = summary.input_modalities()
                .iter()
                .any(|m| m.as_str() == "IMAGE");

            // Estimate context window and max output from known providers
            let (context_window, max_output) = Self::estimate_model_limits(model_id);

            models.push(ModelInfo {
                id: model_id.to_string(),
                name: format!("{model_name} ({provider_name})"),
                description: format!("{provider_name} {model_name} via AWS Bedrock"),
                context_window,
                max_output_tokens: Some(max_output),
                supports_tools: supports_streaming, // Converse-capable models generally support tools
                supports_vision,
            });
        }

        Ok(models)
    }

    /// Estimate context window and max output tokens for known model families.
    /// The Bedrock ListFoundationModels API doesn't return these values directly.
    fn estimate_model_limits(model_id: &str) -> (u32, u32) {
        if model_id.contains("claude") {
            (200_000, 8192)
        } else if model_id.contains("nova-pro") {
            (300_000, 5120)
        } else if model_id.contains("nova-lite") {
            (300_000, 5120)
        } else if model_id.contains("nova-micro") {
            (128_000, 5120)
        } else if model_id.contains("llama") {
            (128_000, 4096)
        } else if model_id.contains("mistral-large") || model_id.contains("mixtral") {
            (128_000, 4096)
        } else if model_id.contains("mistral") {
            (32_000, 4096)
        } else if model_id.contains("titan") {
            (32_000, 4096)
        } else if model_id.contains("command") {
            (128_000, 4096)
        } else if model_id.contains("jamba") {
            (256_000, 4096)
        } else {
            (32_000, 4096) // conservative default
        }
    }

    /// Fallback model list when the API call fails (e.g. no credentials)
    fn fallback_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "anthropic.claude-sonnet-4-20250514-v1:0".to_string(),
                name: "Claude Sonnet 4 (Bedrock)".to_string(),
                description: "Claude Sonnet 4 via AWS Bedrock".to_string(),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-opus-4-20250514-v1:0".to_string(),
                name: "Claude Opus 4 (Bedrock)".to_string(),
                description: "Claude Opus 4 via AWS Bedrock".to_string(),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-3-5-haiku-20241022-v1:0".to_string(),
                name: "Claude 3.5 Haiku (Bedrock)".to_string(),
                description: "Claude 3.5 Haiku via AWS Bedrock".to_string(),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-pro-v1:0".to_string(),
                name: "Amazon Nova Pro".to_string(),
                description: "Amazon's Nova Pro model".to_string(),
                context_window: 300_000,
                max_output_tokens: Some(5120),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-lite-v1:0".to_string(),
                name: "Amazon Nova Lite".to_string(),
                description: "Amazon's Nova Lite model".to_string(),
                context_window: 300_000,
                max_output_tokens: Some(5120),
                supports_tools: true,
                supports_vision: true,
            },
        ]
    }

    /// Normalize model ID (strip provider prefix)
    #[allow(clippy::unused_self)]
    fn normalize_model(&self, model_id: &str) -> String {
        model_id
            .strip_prefix("bedrock/")
            .unwrap_or(model_id)
            .to_string()
    }

    /// Convert our messages to Bedrock format, extracting system prompt
    #[allow(clippy::unused_self)]
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> (Option<Vec<SystemContentBlock>>, Vec<BedrockMessage>) {
        let mut system_blocks = Vec::new();
        // Collect content blocks grouped by role, merging consecutive same-role messages
        // (Bedrock requires strictly alternating user/assistant roles)
        let mut pending_role: Option<ConversationRole> = None;
        let mut pending_blocks: Vec<ContentBlock> = Vec::new();
        let mut bedrock_messages = Vec::new();

        let flush = |role: ConversationRole, blocks: Vec<ContentBlock>, out: &mut Vec<BedrockMessage>| {
            if blocks.is_empty() {
                return;
            }
            #[allow(clippy::expect_used)]
            let mut builder = BedrockMessage::builder().role(role);
            for block in blocks {
                builder = builder.content(block);
            }
            out.push(builder.build().expect("valid message"));
        };

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    system_blocks.push(
                        SystemContentBlock::Text(msg.content.clone()),
                    );
                }
                MessageRole::Tool => {
                    // Tool results are sent as User role with ContentBlock::ToolResult
                    let target_role = ConversationRole::User;
                    if pending_role.as_ref() != Some(&target_role) {
                        if let Some(role) = pending_role.take() {
                            flush(role, std::mem::take(&mut pending_blocks), &mut bedrock_messages);
                        }
                        pending_role = Some(target_role);
                    }
                    let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                    #[allow(clippy::expect_used)]
                    let tool_result = ToolResultBlock::builder()
                        .tool_use_id(&tool_call_id)
                        .content(ToolResultContentBlock::Text(msg.content.clone()))
                        .build()
                        .expect("valid tool result");
                    pending_blocks.push(ContentBlock::ToolResult(tool_result));
                }
                MessageRole::User => {
                    let target_role = ConversationRole::User;
                    if pending_role.as_ref() != Some(&target_role) {
                        if let Some(role) = pending_role.take() {
                            flush(role, std::mem::take(&mut pending_blocks), &mut bedrock_messages);
                        }
                        pending_role = Some(target_role);
                    }
                    pending_blocks.push(ContentBlock::Text(msg.content.clone()));
                }
                MessageRole::Assistant => {
                    let target_role = ConversationRole::Assistant;
                    if pending_role.as_ref() != Some(&target_role) {
                        if let Some(role) = pending_role.take() {
                            flush(role, std::mem::take(&mut pending_blocks), &mut bedrock_messages);
                        }
                        pending_role = Some(target_role);
                    }
                    if !msg.content.is_empty() {
                        pending_blocks.push(ContentBlock::Text(msg.content.clone()));
                    }
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            let input_doc = serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                                .map(|v| Self::json_to_document(&v))
                                .unwrap_or_else(|_| aws_smithy_types::Document::Object(Default::default()));
                            #[allow(clippy::expect_used)]
                            let tool_use = ToolUseBlock::builder()
                                .tool_use_id(&tc.id)
                                .name(&tc.function.name)
                                .input(input_doc)
                                .build()
                                .expect("valid tool use");
                            pending_blocks.push(ContentBlock::ToolUse(tool_use));
                        }
                    }
                    if pending_blocks.is_empty() {
                        pending_blocks.push(ContentBlock::Text(String::new()));
                    }
                }
            }
        }
        // Flush any remaining pending blocks
        if let Some(role) = pending_role {
            flush(role, pending_blocks, &mut bedrock_messages);
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
            aws_smithy_types::Document::Number(n) => match n {
                aws_smithy_types::Number::PosInt(i) => i.to_string(),
                aws_smithy_types::Number::NegInt(i) => i.to_string(),
                aws_smithy_types::Number::Float(f) => {
                    if f.is_finite() { format!("{f}") } else { "null".to_string() }
                }
            },
            aws_smithy_types::Document::String(s) => {
                // Properly escape for JSON
                serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
            }
            aws_smithy_types::Document::Array(arr) => {
                let items: Vec<String> = arr.iter().map(Self::document_to_json_string).collect();
                format!("[{}]", items.join(","))
            }
            aws_smithy_types::Document::Object(obj) => {
                let items: Vec<String> = obj.iter()
                    .map(|(k, v)| {
                        let key = serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\""));
                        format!("{key}:{}", Self::document_to_json_string(v))
                    })
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
    }

    /// Convert tool definitions to Bedrock format
    #[allow(clippy::unused_self)]
    fn convert_tools(&self, tools: &[ToolDefinition]) -> ToolConfiguration {
        let bedrock_tools: Vec<Tool> = tools
            .iter()
            .map(|t| {
                let doc = Self::json_to_document(&t.parameters);

                #[allow(clippy::expect_used)] // builder is always valid when required fields are set
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

        #[allow(clippy::expect_used)] // builder is always valid when tools list is set
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
            auth_methods: vec![
                // Standard AWS credential chain (long-lived keys)
                AuthMethod {
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
                            id: "session_token".to_string(),
                            label: "Session Token".to_string(),
                            secret: true,
                            default: None,
                            placeholder: Some("Optional — for temporary credentials".to_string()),
                            required: false,
                            env_var: Some("AWS_SESSION_TOKEN".to_string()),
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
                        CredentialField {
                            id: "endpoint_url".to_string(),
                            label: "Endpoint URL".to_string(),
                            secret: false,
                            default: Some("https://bedrock-runtime.us-east-1.amazonaws.com".to_string()),
                            placeholder: Some("https://bedrock-runtime.{region}.amazonaws.com".to_string()),
                            required: false,
                            env_var: None,
                        },
                    ],
                    hint: Some("Also supports IAM roles, ~/.aws/credentials, and instance profiles".to_string()),
                    docs_url: Some("https://aws.amazon.com/bedrock/".to_string()),
                },
                // AWS Bedrock session key (temporary credentials from console)
                AuthMethod {
                    id: "aws_bearer_token".to_string(),
                    label: "AWS Bearer Token (Session Key)".to_string(),
                    fields: vec![
                        CredentialField {
                            id: "bearer_token".to_string(),
                            label: "Bearer Token".to_string(),
                            secret: true,
                            default: None,
                            placeholder: Some("Paste token from AWS console".to_string()),
                            required: true,
                            env_var: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
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
                        CredentialField {
                            id: "endpoint_url".to_string(),
                            label: "Endpoint URL".to_string(),
                            secret: false,
                            default: Some("https://bedrock-runtime.us-east-1.amazonaws.com".to_string()),
                            placeholder: Some("https://bedrock-runtime.{region}.amazonaws.com".to_string()),
                            required: false,
                            env_var: None,
                        },
                    ],
                    hint: Some(
                        "Use a session key from the AWS console. Generate one at: \
                         AWS Console → Bedrock → Session credentials. \
                         The console names this AWS_BEARER_TOKEN_BEDROCK."
                            .to_string(),
                    ),
                    docs_url: Some("https://aws.amazon.com/bedrock/".to_string()),
                },
                // AgentCore Identity - OAuth2 credential provider
                AuthMethod {
                    id: "agentcore_oauth2".to_string(),
                    label: "AgentCore Identity (OAuth2)".to_string(),
                    fields: vec![
                        CredentialField {
                            id: "credential_provider_name".to_string(),
                            label: "Credential Provider Name".to_string(),
                            secret: false,
                            default: None,
                            placeholder: Some("my-google-oauth-provider".to_string()),
                            required: true,
                            env_var: None,
                        },
                        CredentialField {
                            id: "auth_flow".to_string(),
                            label: "Auth Flow".to_string(),
                            secret: false,
                            default: Some("USER_FEDERATION".to_string()),
                            placeholder: Some("USER_FEDERATION or CLIENT_CREDENTIALS".to_string()),
                            required: true,
                            env_var: None,
                        },
                        CredentialField {
                            id: "oauth_scopes".to_string(),
                            label: "OAuth2 Scopes".to_string(),
                            secret: false,
                            default: None,
                            placeholder: Some("https://www.googleapis.com/auth/drive.readonly".to_string()),
                            required: false,
                            env_var: None,
                        },
                        CredentialField {
                            id: "oauth_vendor".to_string(),
                            label: "OAuth2 Vendor".to_string(),
                            secret: false,
                            default: Some("CustomOauth2".to_string()),
                            placeholder: Some("GoogleOauth2, GithubOauth2, SlackOauth2, ...".to_string()),
                            required: true,
                            env_var: None,
                        },
                        CredentialField {
                            id: "credentials_secret_arn".to_string(),
                            label: "Credentials Secret ARN".to_string(),
                            secret: true,
                            default: None,
                            placeholder: Some("arn:aws:secretsmanager:...".to_string()),
                            required: true,
                            env_var: None,
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
                    hint: Some(
                        "Uses Bedrock AgentCore Identity for OAuth2 tokens. \
                         Supports 23+ built-in providers (Google, GitHub, Slack, Salesforce, etc.). \
                         Create provider first: aws bedrock-agentcore-control create-oauth2-credential-provider. \
                         Requires IAM: bedrock-agentcore:GetResourceOauth2Token, secretsmanager:GetSecretValue"
                            .to_string(),
                    ),
                    docs_url: Some("https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/identity.html".to_string()),
                },
                // AgentCore Identity - API key credential provider
                AuthMethod {
                    id: "agentcore_api_key".to_string(),
                    label: "AgentCore Identity (API Key)".to_string(),
                    fields: vec![
                        CredentialField {
                            id: "credential_provider_name".to_string(),
                            label: "Credential Provider Name".to_string(),
                            secret: false,
                            default: None,
                            placeholder: Some("my-api-key-provider".to_string()),
                            required: true,
                            env_var: None,
                        },
                        CredentialField {
                            id: "api_key".to_string(),
                            label: "API Key".to_string(),
                            secret: true,
                            default: None,
                            placeholder: Some("sk-...".to_string()),
                            required: true,
                            env_var: None,
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
                    hint: Some(
                        "Uses Bedrock AgentCore Identity Token Vault for API key storage. \
                         Create provider first: aws bedrock-agentcore-control create-api-key-credential-provider. \
                         Requires IAM: bedrock-agentcore:GetApiKeyCredential"
                            .to_string(),
                    ),
                    docs_url: Some("https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/identity.html".to_string()),
                },
            ],
        })
    }

    async fn is_configured(&self) -> bool {
        // Check bearer token env var first (alternative auth for any mode)
        if std::env::var("AWS_BEARER_TOKEN_BEDROCK").is_ok() {
            return true;
        }
        // Probe the credential chain using the same config the client was built with
        if let Some(creds) = self.sdk_config.credentials_provider() {
            creds.provide_credentials().await.is_ok()
        } else {
            false
        }
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

        let response = req.send().await.map_err(|e| {
            // Include full error chain for better diagnostics
            let mut msg = format!("Bedrock API error: {e}");
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                msg.push_str(&format!(" — caused by: {cause}"));
                source = std::error::Error::source(cause);
            }
            LlmError::ApiError { message: msg }
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

        let usage = response.usage().map_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        }, |u| Usage {
            prompt_tokens: u.input_tokens() as u64,
            completion_tokens: u.output_tokens() as u64,
            total_tokens: u.total_tokens() as u64,
        });

        let finish_reason = response.stop_reason().as_str().to_string();

        Ok(ChatCompletionResponse {
            id: format!("bedrock-{}", uuid::Uuid::new_v4()),
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
                    content,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    tool_call_id: None,
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
            message: format!("Bedrock streaming error: {e}"),
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
                                    #[allow(clippy::unwrap_used)]
                                    let created_ts = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_secs();
                                    yield Ok(StreamingChunk {
                                        id: format!("stream-{}", uuid::Uuid::new_v4()),
                                        object: "chat.completion.chunk".to_string(),
                                        created: created_ts,
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
                                    });
                                }
                            }
                            ConverseStreamOutput::MessageStop(_) => {
                                #[allow(clippy::unwrap_used)]
                                let created_ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();
                                yield Ok(StreamingChunk {
                                    id: format!("stream-{}", uuid::Uuid::new_v4()),
                                    object: "chat.completion.chunk".to_string(),
                                    created: created_ts,
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
                            message: format!("Bedrock stream error: {e}"),
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
        // Try to list models from the Bedrock API
        match self.list_models_from_api().await {
            Ok(models) if !models.is_empty() => Ok(models),
            Ok(_) | Err(_) => {
                // Fallback to well-known models if API call fails
                // (e.g., credentials not configured, permissions insufficient)
                Ok(Self::fallback_models())
            }
        }
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn test_normalize_model() {
        assert_eq!(
            "anthropic.claude-sonnet-4-20250514-v1:0",
            "bedrock/anthropic.claude-sonnet-4-20250514-v1:0"
                .strip_prefix("bedrock/")
                .unwrap_or("anthropic.claude-sonnet-4-20250514-v1:0")
        );
    }

    #[test]
    fn test_auth_mode_default() {
        let mode = BedrockAuthMode::default();
        assert!(matches!(mode, BedrockAuthMode::AwsCredentials));
    }

    #[test]
    fn test_auth_flow_display() {
        assert_eq!(AgentCoreAuthFlow::UserFederation.to_string(), "USER_FEDERATION");
        assert_eq!(AgentCoreAuthFlow::ClientCredentials.to_string(), "CLIENT_CREDENTIALS");
    }

    #[test]
    fn test_oauth_vendor_ids() {
        assert_eq!(AgentCoreOAuthVendor::Google.vendor_id(), "GoogleOauth2");
        assert_eq!(AgentCoreOAuthVendor::GitHub.vendor_id(), "GithubOauth2");
        assert_eq!(AgentCoreOAuthVendor::Slack.vendor_id(), "SlackOauth2");
        assert_eq!(
            AgentCoreOAuthVendor::Custom {
                discovery_url: "https://example.com/.well-known/openid-configuration".to_string()
            }
            .vendor_id(),
            "CustomOauth2"
        );
    }

    #[test]
    fn test_agentcore_config_default() {
        let config = AgentCoreConfig::default();
        assert!(config.credential_provider_name.is_empty());
        assert!(matches!(config.auth_flow, AgentCoreAuthFlow::UserFederation));
        assert!(config.scopes.is_empty());
        assert!(config.credentials_secret_arn.is_none());
        assert!(config.vendor.is_none());
    }
}
