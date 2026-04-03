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
//! use rockbot_llm_bedrock::BedrockProvider;
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

use async_trait::async_trait;
use aws_sdk_bedrockruntime::config::ProvideCredentials;
use aws_sdk_bedrockruntime::{
    types::{
        ContentBlock, ContentBlockDelta, ContentBlockStart, ConversationRole, ConverseOutput,
        ConverseStreamOutput, Message as BedrockMessage, SystemContentBlock, Tool,
        ToolConfiguration, ToolInputSchema, ToolResultBlock, ToolResultContentBlock,
        ToolSpecification, ToolUseBlock,
    },
    Client,
};
use rockbot_llm::{
    AuthMethod, ChatCompletionRequest, ChatCompletionResponse, Choice, CompletionStream,
    CredentialCategory, CredentialField, CredentialSchema, FunctionCall, LlmError, LlmProvider,
    Message, MessageRole, ModelInfo, ProviderCapabilities, ResponseFormat, Result, StreamingChoice,
    StreamingChunk, StreamingDelta, ToolCall, ToolDefinition, Usage,
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
    /// Build runtime and bedrock clients from an AWS SdkConfig, applying
    /// connect/read timeouts so that misconfigured models or network issues
    /// cannot hang the process indefinitely.
    fn build_clients(config: &aws_config::SdkConfig) -> (Client, aws_sdk_bedrock::Client) {
        use std::time::Duration;

        let timeout = aws_sdk_bedrockruntime::config::timeout::TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(90))
            .operation_timeout(Duration::from_secs(120))
            .build();

        let runtime_config = aws_sdk_bedrockruntime::config::Builder::from(config)
            .timeout_config(timeout.clone())
            .build();

        let control_timeout = aws_sdk_bedrock::config::timeout::TimeoutConfig::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .operation_timeout(Duration::from_secs(45))
            .build();

        let control_config = aws_sdk_bedrock::config::Builder::from(config)
            .timeout_config(control_timeout)
            .build();

        (
            Client::from_conf(runtime_config),
            aws_sdk_bedrock::Client::from_conf(control_config),
        )
    }

    /// Create a new Bedrock provider with the specified region using standard AWS credentials
    pub async fn new(region: &str) -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()))
            .load()
            .await;

        let (client, bedrock_client) = Self::build_clients(&config);

        Ok(Self {
            client,
            bedrock_client,
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
            .region()
            .map_or_else(|| "us-east-1".to_string(), std::string::ToString::to_string);

        let (client, bedrock_client) = Self::build_clients(&config);

        Ok(Self {
            client,
            bedrock_client,
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

        let (client, bedrock_client) = Self::build_clients(&aws_config);

        Ok(Self {
            client,
            bedrock_client,
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

        let (client, bedrock_client) = Self::build_clients(&aws_config);

        Ok(Self {
            client,
            bedrock_client,
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

    /// List models from the Bedrock APIs.
    ///
    /// This merges foundation models with system/application inference profiles
    /// so the UI can offer both native model IDs and Bedrock-defined execution
    /// targets.
    async fn list_models_from_api(&self) -> Result<Vec<ModelInfo>> {
        use aws_sdk_bedrock::types::{InferenceProfileStatus, InferenceType};

        let resp = self
            .bedrock_client
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
            let supports_vision = summary
                .input_modalities()
                .iter()
                .any(|m| m.as_str() == "IMAGE");

            // Estimate context window and max output from known providers
            let (context_window, max_output) = Self::estimate_model_limits(model_id);

            models.push(ModelInfo {
                id: model_id.to_string(),
                name: format!("{model_name} ({provider_name})"),
                description: format!("{provider_name} {model_name} via AWS Bedrock"),
                kind: Some("foundation_model".to_string()),
                context_window,
                max_output_tokens: Some(max_output),
                supports_tools: supports_streaming, // Converse-capable models generally support tools
                supports_vision,
            });
        }

        let mut next_token = None;
        loop {
            let mut request = self.bedrock_client.list_inference_profiles();
            if let Some(token) = next_token.clone() {
                request = request.next_token(token);
            }
            let response = request.send().await.map_err(|e| LlmError::ApiError {
                message: format!("Failed to list Bedrock inference profiles: {e}"),
            })?;

            for summary in response.inference_profile_summaries() {
                if *summary.status() != InferenceProfileStatus::Active {
                    continue;
                }

                let profile_id = summary.inference_profile_id();
                let profile_name = summary.inference_profile_name();
                let description = summary.description().unwrap_or("");
                let model_targets: Vec<&str> = summary
                    .models()
                    .iter()
                    .filter_map(|model| model.model_arn())
                    .collect();
                let backing_model_hint = model_targets
                    .first()
                    .copied()
                    .unwrap_or(profile_id)
                    .rsplit('/')
                    .next()
                    .unwrap_or(profile_id);
                let (context_window, max_output) = Self::estimate_model_limits(backing_model_hint);
                let supports_vision = model_targets
                    .iter()
                    .any(|arn| arn.contains("claude") || arn.contains("nova"));
                let profile_kind = format!(
                    "inference_profile:{}",
                    summary.r#type().as_str().to_ascii_lowercase()
                );
                let model_summary = if model_targets.is_empty() {
                    "No backing models reported".to_string()
                } else {
                    format!("Targets: {}", model_targets.join(", "))
                };
                let detail = if description.is_empty() {
                    model_summary
                } else {
                    format!("{description} | {model_summary}")
                };

                models.push(ModelInfo {
                    id: profile_id.to_string(),
                    name: format!("{} [Profile]", profile_name),
                    description: detail,
                    kind: Some(profile_kind),
                    context_window,
                    max_output_tokens: Some(max_output),
                    supports_tools: true,
                    supports_vision,
                });
            }

            next_token = response.next_token().map(str::to_string);
            if next_token.is_none() {
                break;
            }
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
                kind: Some("foundation_model".to_string()),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-opus-4-20250514-v1:0".to_string(),
                name: "Claude Opus 4 (Bedrock)".to_string(),
                description: "Claude Opus 4 via AWS Bedrock".to_string(),
                kind: Some("foundation_model".to_string()),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "anthropic.claude-3-5-haiku-20241022-v1:0".to_string(),
                name: "Claude 3.5 Haiku (Bedrock)".to_string(),
                description: "Claude 3.5 Haiku via AWS Bedrock".to_string(),
                kind: Some("foundation_model".to_string()),
                context_window: 200_000,
                max_output_tokens: Some(8192),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-pro-v1:0".to_string(),
                name: "Amazon Nova Pro".to_string(),
                description: "Amazon's Nova Pro model".to_string(),
                kind: Some("foundation_model".to_string()),
                context_window: 300_000,
                max_output_tokens: Some(5120),
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                id: "amazon.nova-lite-v1:0".to_string(),
                name: "Amazon Nova Lite".to_string(),
                description: "Amazon's Nova Lite model".to_string(),
                kind: Some("foundation_model".to_string()),
                context_window: 300_000,
                max_output_tokens: Some(5120),
                supports_tools: true,
                supports_vision: true,
            },
        ]
    }

    /// Normalize model ID: strip "bedrock/" prefix, then prepend a cross-region
    /// inference profile prefix for raw foundation model IDs that require it.
    /// Already-qualified inference profile IDs pass through unchanged.
    fn normalize_model(&self, model_id: &str) -> String {
        let stripped = model_id.strip_prefix("bedrock/").unwrap_or(model_id);

        if Self::has_inference_profile_prefix(stripped) {
            return stripped.to_string();
        }

        if Self::requires_inference_profile(stripped) {
            let prefix = Self::region_to_inference_prefix(&self.region);
            return format!("{prefix}{stripped}");
        }

        stripped.to_string()
    }

    fn has_inference_profile_prefix(model_id: &str) -> bool {
        model_id.starts_with("global.")
            || model_id.starts_with("us.")
            || model_id.starts_with("eu.")
            || model_id.starts_with("ap.")
    }

    /// Returns true when a model requires a cross-region inference profile.
    fn requires_inference_profile(model_id: &str) -> bool {
        model_id.contains("claude-3")
            || model_id.contains("claude-4")
            || model_id.contains("claude-opus-4")
            || model_id.contains("claude-sonnet-4")
            || model_id.starts_with("amazon.nova-")
    }

    /// Map an AWS region to the corresponding inference profile prefix.
    fn region_to_inference_prefix(region: &str) -> &'static str {
        if region.starts_with("us-") {
            "us."
        } else if region.starts_with("eu-") {
            "eu."
        } else if region.starts_with("ap-") {
            "ap."
        } else {
            "us." // conservative default
        }
    }

    /// Convert our messages to Bedrock format, extracting system prompt
    #[allow(clippy::unused_self)]
    fn convert_messages(
        &self,
        messages: &[Message],
    ) -> (Option<Vec<SystemContentBlock>>, Vec<BedrockMessage>) {
        #[derive(Clone, Copy, Eq, PartialEq)]
        enum PendingMessageKind {
            UserText,
            ToolResult,
            Assistant,
        }

        let mut system_blocks = Vec::new();
        // Collect content blocks grouped by Bedrock turn semantics.
        // Tool results must remain isolated from ordinary user text even though both
        // map to `ConversationRole::User`, otherwise Bedrock/Anthropic rejects the
        // request because toolResult blocks no longer align with the immediately
        // preceding assistant toolUse turn.
        let mut pending_kind: Option<PendingMessageKind> = None;
        let mut pending_blocks: Vec<ContentBlock> = Vec::new();
        let mut bedrock_messages = Vec::new();

        let flush =
            |kind: PendingMessageKind, blocks: Vec<ContentBlock>, out: &mut Vec<BedrockMessage>| {
                if blocks.is_empty() {
                    return;
                }
                let role = match kind {
                    PendingMessageKind::UserText | PendingMessageKind::ToolResult => {
                        ConversationRole::User
                    }
                    PendingMessageKind::Assistant => ConversationRole::Assistant,
                };
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
                    system_blocks.push(SystemContentBlock::Text(msg.content.clone()));
                }
                MessageRole::Tool => {
                    let target_kind = PendingMessageKind::ToolResult;
                    if pending_kind.as_ref() != Some(&target_kind) {
                        if let Some(kind) = pending_kind.take() {
                            flush(
                                kind,
                                std::mem::take(&mut pending_blocks),
                                &mut bedrock_messages,
                            );
                        }
                        pending_kind = Some(target_kind);
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
                    let target_kind = PendingMessageKind::UserText;
                    if pending_kind.as_ref() != Some(&target_kind) {
                        if let Some(kind) = pending_kind.take() {
                            flush(
                                kind,
                                std::mem::take(&mut pending_blocks),
                                &mut bedrock_messages,
                            );
                        }
                        pending_kind = Some(target_kind);
                    }
                    pending_blocks.push(ContentBlock::Text(msg.content.clone()));
                }
                MessageRole::Assistant => {
                    let target_kind = PendingMessageKind::Assistant;
                    if pending_kind.as_ref() != Some(&target_kind) {
                        if let Some(kind) = pending_kind.take() {
                            flush(
                                kind,
                                std::mem::take(&mut pending_blocks),
                                &mut bedrock_messages,
                            );
                        }
                        pending_kind = Some(target_kind);
                    }
                    if !msg.content.is_empty() {
                        pending_blocks.push(ContentBlock::Text(msg.content.clone()));
                    }
                    if let Some(ref tool_calls) = msg.tool_calls {
                        for tc in tool_calls {
                            let input_doc =
                                serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                                    .map(|v| Self::json_to_document(&v))
                                    .unwrap_or_else(|_| {
                                        aws_smithy_types::Document::Object(Default::default())
                                    });
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
        if let Some(kind) = pending_kind {
            flush(kind, pending_blocks, &mut bedrock_messages);
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
                    let number = if i < 0 {
                        aws_smithy_types::Number::NegInt(i)
                    } else {
                        aws_smithy_types::Number::PosInt(i as u64)
                    };
                    aws_smithy_types::Document::Number(number)
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
            serde_json::Value::Object(obj) => aws_smithy_types::Document::Object(
                obj.iter()
                    .map(|(k, v)| (k.clone(), Self::json_to_document(v)))
                    .collect(),
            ),
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
                    if f.is_finite() {
                        format!("{f}")
                    } else {
                        "null".to_string()
                    }
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
                let items: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| {
                        let key = serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\""));
                        format!("{key}:{}", Self::document_to_json_string(v))
                    })
                    .collect();
                format!("{{{}}}", items.join(","))
            }
        }
    }

    fn append_json_mode_hint(
        system: &mut Option<Vec<SystemContentBlock>>,
        response_format: Option<&ResponseFormat>,
    ) {
        let Some(response_format) = response_format else {
            return;
        };

        let json_hint = match response_format {
            ResponseFormat::Text => None,
            ResponseFormat::JsonObject => Some(
                "IMPORTANT: You MUST respond with valid JSON only. No markdown, no explanation, no text outside the JSON object.".to_string(),
            ),
            ResponseFormat::JsonSchema { schema } => Some(format!(
                "IMPORTANT: You MUST respond with valid JSON conforming to this schema:\n{}\nNo markdown, no explanation, no text outside the JSON object.",
                serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string())
            )),
        };

        if let Some(hint) = json_hint {
            let hint_block = SystemContentBlock::Text(hint);
            match system {
                Some(blocks) => blocks.push(hint_block),
                None => *system = Some(vec![hint_block]),
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

                #[allow(clippy::expect_used)]
                // builder is always valid when required fields are set
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
        let (mut system, messages) = self.convert_messages(&request.messages);
        Self::append_json_mode_hint(&mut system, request.response_format.as_ref());

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
        let mut reasoning_text = String::new();
        let mut tool_calls = Vec::new();

        if let Some(ConverseOutput::Message(msg)) = response.output() {
            for block in msg.content() {
                match block {
                    ContentBlock::Text(text) => {
                        content.push_str(text);
                    }
                    ContentBlock::ReasoningContent(reasoning) => {
                        if let Ok(rt) = reasoning.as_reasoning_text() {
                            reasoning_text.push_str(rt.text());
                        }
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

        // If the model produced no visible text but did produce reasoning content
        // (common with reasoning models like Kimi K2.5), wrap the reasoning in
        // <think> tags so downstream strip_think_blocks can handle it properly.
        if content.trim().is_empty() && !reasoning_text.is_empty() {
            content = format!("<think>{reasoning_text}</think>");
        }

        let usage = response.usage().map_or(
            Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
            |u| Usage {
                prompt_tokens: u.input_tokens() as u64,
                completion_tokens: u.output_tokens() as u64,
                total_tokens: u.total_tokens() as u64,
            },
        );

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
                    images: vec![],
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

    async fn stream_completion(&self, request: ChatCompletionRequest) -> Result<CompletionStream> {
        let model = self.normalize_model(&request.model);
        let (mut system, messages) = self.convert_messages(&request.messages);
        Self::append_json_mode_hint(&mut system, request.response_format.as_ref());

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
            let mut in_reasoning_block = false;

            // Track in-progress tool use blocks from ContentBlockStart/Delta
            // Each entry: (content_block_index, tool_use_id, tool_name, accumulated_input)
            let mut pending_tool_uses: Vec<(i32, String, String, String)> = Vec::new();
            let mut tool_call_index: usize = 0;

            loop {
                match receiver.recv().await {
                    Ok(Some(event)) => {
                        match event {
                            ConverseStreamOutput::ContentBlockStart(start_event) => {
                                if let Some(ContentBlockStart::ToolUse(tool_start)) = start_event.start() {
                                    pending_tool_uses.push((
                                        start_event.content_block_index(),
                                        tool_start.tool_use_id().to_string(),
                                        tool_start.name().to_string(),
                                        String::new(),
                                    ));
                                    // Emit initial tool call chunk with name and id
                                    let tc_idx = tool_call_index;
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
                                                tool_calls: Some(vec![ToolCall {
                                                    id: tool_start.tool_use_id().to_string(),
                                                    r#type: "function".to_string(),
                                                    function: FunctionCall {
                                                        name: tool_start.name().to_string(),
                                                        arguments: String::new(),
                                                    },
                                                }]),
                                            },
                                            finish_reason: None,
                                        }],
                                    });
                                    tool_call_index = tc_idx + 1;
                                }
                            }
                            ConverseStreamOutput::ContentBlockDelta(delta_event) => {
                                match delta_event.delta() {
                                    Some(ContentBlockDelta::Text(text)) => {
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
                                    Some(ContentBlockDelta::ToolUse(tool_delta)) => {
                                        let cb_index = delta_event.content_block_index();
                                        // Find the pending tool use by block index and append input
                                        if let Some(pending) = pending_tool_uses.iter_mut()
                                            .find(|(idx, _, _, _)| *idx == cb_index)
                                        {
                                            pending.3.push_str(tool_delta.input());
                                            // Emit argument fragment for streaming merge
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
                                                        tool_calls: Some(vec![ToolCall {
                                                            id: pending.1.clone(),
                                                            r#type: "function".to_string(),
                                                            function: FunctionCall {
                                                                name: String::new(),
                                                                arguments: tool_delta.input().to_string(),
                                                            },
                                                        }]),
                                                    },
                                                    finish_reason: None,
                                                }],
                                            });
                                        }
                                    }
                                    Some(ContentBlockDelta::ReasoningContent(rc)) => {
                                        // Stream reasoning text in real-time wrapped in
                                        // <think> tags so the TUI shows thinking progress.
                                        if let Ok(text) = rc.as_text() {
                                            let mut emit_text = String::new();
                                            if !in_reasoning_block {
                                                emit_text.push_str("<think>");
                                                in_reasoning_block = true;
                                            }
                                            emit_text.push_str(text);
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
                                                        content: Some(emit_text),
                                                        tool_calls: None,
                                                    },
                                                    finish_reason: None,
                                                }],
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            ConverseStreamOutput::MessageStop(_) => {
                                // Close any open reasoning block
                                if in_reasoning_block {
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
                                                content: Some("</think>".to_string()),
                                                tool_calls: None,
                                            },
                                            finish_reason: None,
                                        }],
                                    });
                                }

                                let finish = if pending_tool_uses.is_empty() {
                                    "stop"
                                } else {
                                    "tool_use"
                                };
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
                                        finish_reason: Some(finish.to_string()),
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
            message: "Use Bedrock embedding models directly via AWS SDK for embeddings".to_string(),
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
    fn test_requires_inference_profile() {
        // Claude 3 and 4 models require inference profiles
        assert!(BedrockProvider::requires_inference_profile(
            "anthropic.claude-3-5-haiku-20241022-v1:0"
        ));
        assert!(BedrockProvider::requires_inference_profile(
            "anthropic.claude-sonnet-4-20250514-v1:0"
        ));
        assert!(BedrockProvider::requires_inference_profile(
            "anthropic.claude-opus-4-20250514-v1:0"
        ));
        // Amazon Nova models require inference profiles
        assert!(BedrockProvider::requires_inference_profile(
            "amazon.nova-pro-v1:0"
        ));
        assert!(BedrockProvider::requires_inference_profile(
            "amazon.nova-lite-v1:0"
        ));
        // Older models do not require inference profiles
        assert!(!BedrockProvider::requires_inference_profile(
            "anthropic.claude-v2"
        ));
        assert!(!BedrockProvider::requires_inference_profile(
            "anthropic.claude-instant-v1"
        ));
        assert!(!BedrockProvider::requires_inference_profile(
            "meta.llama3-8b-instruct-v1:0"
        ));
    }

    #[test]
    fn test_region_to_inference_prefix() {
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("us-east-1"),
            "us."
        );
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("us-west-2"),
            "us."
        );
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("eu-west-1"),
            "eu."
        );
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("eu-central-1"),
            "eu."
        );
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("ap-southeast-1"),
            "ap."
        );
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("ap-northeast-1"),
            "ap."
        );
        // Unknown region falls back to "us."
        assert_eq!(
            BedrockProvider::region_to_inference_prefix("sa-east-1"),
            "us."
        );
    }

    #[test]
    fn test_normalize_model_pass_through_prefixed() {
        assert!(BedrockProvider::has_inference_profile_prefix(
            "us.anthropic.claude-sonnet-4-20250514-v1:0"
        ));
        assert!(BedrockProvider::has_inference_profile_prefix(
            "eu.anthropic.claude-3-5-haiku-20241022-v1:0"
        ));
        assert!(BedrockProvider::has_inference_profile_prefix(
            "ap.amazon.nova-pro-v1:0"
        ));
        assert!(BedrockProvider::has_inference_profile_prefix(
            "global.anthropic.claude-opus-4-6-v1"
        ));
        assert!(!BedrockProvider::has_inference_profile_prefix(
            "anthropic.claude-opus-4-6-v1"
        ));
    }

    #[test]
    fn test_auth_mode_default() {
        let mode = BedrockAuthMode::default();
        assert!(matches!(mode, BedrockAuthMode::AwsCredentials));
    }

    #[test]
    fn test_auth_flow_display() {
        assert_eq!(
            AgentCoreAuthFlow::UserFederation.to_string(),
            "USER_FEDERATION"
        );
        assert_eq!(
            AgentCoreAuthFlow::ClientCredentials.to_string(),
            "CLIENT_CREDENTIALS"
        );
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
        assert!(matches!(
            config.auth_flow,
            AgentCoreAuthFlow::UserFederation
        ));
        assert!(config.scopes.is_empty());
        assert!(config.credentials_secret_arn.is_none());
        assert!(config.vendor.is_none());
    }

    #[test]
    fn test_append_json_mode_hint_adds_hint_for_streaming_parity() {
        let mut system = None;
        BedrockProvider::append_json_mode_hint(&mut system, Some(&ResponseFormat::JsonObject));
        let blocks = system.expect("json hint should be added");
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            SystemContentBlock::Text(text) => {
                assert!(text.contains("valid JSON only"));
            }
            other => panic!("expected text system block, got {other:?}"),
        }
    }
}
