//! Agent execution engine for RockBot
//!
//! This module provides the core agent functionality, including message processing,
//! tool execution, and LLM interaction.

use crate::error::{AgentError, Result};
use crate::message::{Message, MessageRole, SystemLevel};
use crate::session::{Session, SessionManager};
use crate::config::AgentInstance;
use rockbot_llm::{LlmProvider, LlmError, ChatCompletionRequest, ChatCompletionResponse};
use rockbot_memory::MemoryManager;
use rockbot_tools::{ToolRegistry, ToolExecutionContext, ToolExecutionResult};
use rockbot_tools::message::ToolResult;
use rockbot_security::SecurityManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};


/// Agent execution engine
pub struct Agent {
    /// Agent configuration
    pub config: AgentInstance,
    /// LLM provider for this agent
    llm_provider: Arc<dyn LlmProvider>,
    /// Tool registry
    tool_registry: Arc<ToolRegistry>,
    /// Memory manager
    #[allow(dead_code)]
    memory_manager: Arc<MemoryManager>,
    /// Security manager
    security_manager: Arc<SecurityManager>,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Credential accessor for tool credential injection
    credential_accessor: Option<Arc<dyn rockbot_tools::CredentialAccessor>>,
    /// Agent state
    state: Arc<RwLock<AgentState>>,
}

/// Internal agent state
#[derive(Debug)]
struct AgentState {
    /// Active processing contexts
    active_contexts: HashMap<String, ProcessingContext>,
    /// Agent statistics
    stats: AgentStats,
}

/// Processing context for a message/conversation
#[derive(Debug, Clone)]
struct ProcessingContext {
    /// Session ID
    session_id: String,
    /// Current conversation messages
    messages: Vec<Message>,
    /// Available tools for this context
    available_tools: Vec<String>,
    /// Context size in tokens (estimate)
    token_count: usize,
}

/// Agent execution statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentStats {
    /// Total messages processed
    pub messages_processed: u64,
    /// Total tool executions
    pub tool_executions: u64,
    /// Total tokens used
    pub total_tokens: u64,
    /// Average response time in milliseconds
    pub avg_response_time_ms: u64,
    /// Error count
    pub error_count: u64,
    /// Total retry attempts
    pub retry_attempts: u64,
    /// Rate limit hits
    pub rate_limit_hits: u64,
}

/// Retry configuration for LLM calls
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Base delay in milliseconds
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds
    pub max_delay_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
    /// Jitter factor (0.0-1.0)
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 1000,     // Start with 1 second
            max_delay_ms: 30000,     // Cap at 30 seconds
            backoff_multiplier: 2.0, // Double each time
            jitter_factor: 0.1,      // 10% jitter to prevent thundering herd
        }
    }
}

/// Error classification for retry decisions
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorCategory {
    /// Temporary errors that should be retried
    Retryable,
    /// Rate limiting errors with potential backoff info
    RateLimit { retry_after_ms: Option<u64> },
    /// Authentication/authorization errors
    Auth,
    /// Client errors that shouldn't be retried
    Client,
    /// Server errors that might be retryable
    Server,
    /// Network/connection errors
    Network,
}

/// Agent response to a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// Response message
    pub message: Message,
    /// Tool results (if any)
    pub tool_results: Vec<ToolExecutionResult>,
    /// Token usage for this response
    pub tokens_used: TokenUsage,
    /// Processing time in milliseconds
    pub processing_time_ms: u64,
}

/// Token usage breakdown
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl Agent {
    /// Create a new agent with the given configuration
    pub async fn new(
        config: AgentInstance,
        llm_provider: Arc<dyn LlmProvider>,
        tool_registry: Arc<ToolRegistry>,
        memory_manager: Arc<MemoryManager>,
        security_manager: Arc<SecurityManager>,
        session_manager: Arc<SessionManager>,
        credential_accessor: Option<Arc<dyn rockbot_tools::CredentialAccessor>>,
    ) -> Result<Self> {
        info!("Initializing agent '{}'", config.id);
        
        // Initialize agent workspace if it doesn't exist
        let default_workspace = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("rockbot")
            .join("agents")
            .join(&config.id);
        let workspace = config.workspace.as_ref().unwrap_or(&default_workspace);
        
        tokio::fs::create_dir_all(workspace).await?;
        
        Ok(Self {
            config,
            llm_provider,
            tool_registry,
            memory_manager,
            security_manager,
            session_manager,
            credential_accessor,
            state: Arc::new(RwLock::new(AgentState {
                active_contexts: HashMap::new(),
                stats: AgentStats::default(),
            })),
        })
    }
    
    /// Process an incoming message and generate a response
    pub async fn process_message(
        &self,
        session_id: String,
        message: Message,
    ) -> Result<AgentResponse> {
        let start_time = std::time::Instant::now();

        debug!("Processing message {} in session {}", message.id, session_id);

        // Get or create session — use the session's actual DB ID for all operations
        // (the passed-in session_id may be a compound key like "agent:session_key",
        // but the DB session has its own UUID-based id)
        let session = self.get_or_create_session(&session_id, &message).await?;
        let db_session_id = session.id.clone();

        // Store incoming message
        self.session_manager.add_message(&db_session_id, message.clone()).await?;

        // Update processing context
        let available_tools = self.get_available_tools(&session).await?;
        let mut context = self.update_processing_context(db_session_id.clone(), message, available_tools).await?;

        // Generate LLM request
        let llm_request = self.build_llm_request(&mut context).await?;

        // Call LLM with retry logic
        let llm_response = self.call_llm_with_retry(llm_request).await?;

        // Process LLM response and handle tool calls
        let (response_message, tool_results, token_usage) = self.process_llm_response(
            &db_session_id,
            &mut context,
            llm_response,
        ).await?;

        // Store response message
        self.session_manager.add_message(&db_session_id, response_message.clone()).await?;

        // Update session token stats
        let mut session = self.session_manager.get_session(&db_session_id).await?
            .ok_or_else(|| AgentError::ExecutionFailed {
                message: "Session disappeared during processing".to_string()
            })?;
        session.add_tokens(token_usage.prompt_tokens, token_usage.completion_tokens);
        self.session_manager.update_session(&session).await?;

        // Update agent statistics
        let processing_time_ms = start_time.elapsed().as_millis() as u64;
        self.update_stats(token_usage.total_tokens, processing_time_ms).await;

        // Clean up processing context
        {
            let mut state = self.state.write().await;
            state.active_contexts.remove(&db_session_id);
        }

        Ok(AgentResponse {
            message: response_message,
            tool_results,
            tokens_used: token_usage,
            processing_time_ms,
        })
    }
    
    /// Get or create a session for the given session ID
    ///
    /// The `session_id` may be a compound key like "agent_id:session_key" from the gateway.
    /// We first try exact ID lookup, then try finding by session_key, before creating a new one.
    async fn get_or_create_session(&self, session_id: &str, message: &Message) -> Result<Session> {
        // Try exact ID match first
        if let Some(session) = self.session_manager.get_session(session_id).await? {
            return Ok(session);
        }

        // Try looking up by session_key (handles compound keys like "agent:session_key")
        let session_key = session_id.split_once(':')
            .map_or(session_id, |(_, key)| key);
        if let Some(session) = self.session_manager.find_by_session_key(session_key).await? {
            return Ok(session);
        }

        // Create new session
        let key = message.metadata.source.as_deref()
            .unwrap_or(session_key);
        self.session_manager.create_session(&self.config.id, key).await
    }
    
    /// Get available tools for the current session
    async fn get_available_tools(&self, session: &Session) -> Result<Vec<String>> {
        // Get security context for this session
        let security_context = self.security_manager
            .get_session_context(&session.id)
            .await?;
        
        // Get tools that are allowed by security policy
        let available_tools = self.tool_registry
            .get_available_tools(&security_context.capabilities)
            .await?;
        
        Ok(available_tools.into_iter().map(|t| t.name().to_string()).collect())
    }
    
    /// Update processing context with new message
    async fn update_processing_context(
        &self,
        session_id: String,
        message: Message,
        available_tools: Vec<String>,
    ) -> Result<ProcessingContext> {
        let mut state = self.state.write().await;
        
        let context = state.active_contexts.entry(session_id.clone()).or_insert_with(|| {
            ProcessingContext {
                session_id: session_id.clone(),
                messages: Vec::new(),
                available_tools: available_tools.clone(),
                token_count: 0,
            }
        });
        
        // Add new message to context
        context.messages.push(message);
        context.available_tools = available_tools;
        
        // Estimate token count (rough approximation)
        context.token_count = context.messages.iter()
            .map(|m| m.extract_text().unwrap_or_default().len() / 4) // ~4 chars per token
            .sum();
        
        // If context is too large, perform compaction
        if context.token_count > 100000 { // Rough token limit
            self.compact_context(context).await?;
        }
        
        Ok(context.clone())
    }
    
    /// Compact conversation context by summarizing old messages
    async fn compact_context(&self, context: &mut ProcessingContext) -> Result<()> {
        debug!("Compacting context for session {}", context.session_id);
        
        // For now, simple strategy: keep recent messages and create a summary
        if context.messages.len() > 20 {
            let to_summarize = context.messages.drain(0..context.messages.len() - 15).collect::<Vec<_>>();
            
            // Create summary message
            let summary_text = format!(
                "[Previous conversation summary: {} messages exchanged]",
                to_summarize.len()
            );
            
            let summary_message = Message::system(summary_text, SystemLevel::Info)
                .with_session_id(&context.session_id);
            
            context.messages.insert(0, summary_message);
            
            // Recalculate token count
            context.token_count = context.messages.iter()
                .map(|m| m.extract_text().unwrap_or_default().len() / 4)
                .sum();
        }
        
        Ok(())
    }
    
    /// Build LLM chat completion request with system prompt assembly
    async fn build_llm_request(&self, context: &mut ProcessingContext) -> Result<ChatCompletionRequest> {
        // Assemble system prompt with context injection
        let system_prompt = self.assemble_system_prompt(context).await?;
        
        // Convert messages to LLM format, injecting system message if needed
        let mut messages = Vec::new();
        
        // Add system message first if we have a system prompt
        if !system_prompt.is_empty() {
            messages.push(rockbot_llm::Message {
                role: rockbot_llm::MessageRole::System,
                content: system_prompt,
                tool_calls: None,
            });
        }
        
        // Add conversation messages
        for message in &context.messages {
            // Skip system messages from conversation (they're handled above)
            if matches!(message.metadata.role, MessageRole::System) {
                continue;
            }
            
            messages.push(rockbot_llm::Message {
                role: match message.metadata.role {
                    MessageRole::User => rockbot_llm::MessageRole::User,
                    MessageRole::Assistant => rockbot_llm::MessageRole::Assistant,
                    MessageRole::System => rockbot_llm::MessageRole::System,
                    MessageRole::Tool => rockbot_llm::MessageRole::Tool,
                },
                content: message.extract_text().unwrap_or_default(),
                tool_calls: None, // TODO: Handle tool calls
            });
        }
        
        // Get tool definitions if tools are available
        let tools = if !context.available_tools.is_empty() {
            let tool_defs = self.tool_registry.get_tool_definitions(&context.available_tools).await?;
            // Convert from rockbot_tools::ToolDefinition to rockbot_llm::ToolDefinition
            Some(tool_defs.into_iter().map(|td| {
                rockbot_llm::ToolDefinition {
                    name: td.name,
                    description: td.description,
                    parameters: td.parameters,
                }
            }).collect())
        } else {
            None
        };
        
        let model = self.config.model.as_ref()
            .unwrap_or(&"anthropic/claude-sonnet-4-20250514".to_string())
            .clone();
        
        Ok(ChatCompletionRequest {
            model,
            messages,
            tools,
            temperature: Some(0.7),
            max_tokens: Some(4000),
            stream: false,
        })
    }
    
    /// Assemble system prompt with context injection
    async fn assemble_system_prompt(&self, context: &ProcessingContext) -> Result<String> {
        let mut prompt_parts = Vec::new();
        
        // Load and inject identity context (SOUL.md)
        if let Ok(soul_content) = self.load_context_file("SOUL.md").await {
            prompt_parts.push(format!("# Agent Identity\n\n{soul_content}"));
        }
        
        // Load and inject operational context (AGENTS.md)
        if let Ok(agents_content) = self.load_context_file("AGENTS.md").await {
            prompt_parts.push(format!("# Operational Guidelines\n\n{agents_content}"));
        }
        
        // Inject skills/tools information
        if !context.available_tools.is_empty() {
            let skills_section = self.build_skills_section(&context.available_tools).await?;
            prompt_parts.push(skills_section);
        }
        
        // Add session and agent context
        let context_section = format!(
            "# Current Context\n\n- Agent ID: {}\n- Session ID: {}\n- Available tools: {}\n- Workspace: {}",
            self.config.id,
            context.session_id,
            context.available_tools.join(", "),
            self.get_workspace_path().display()
        );
        prompt_parts.push(context_section);
        
        // Add current timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        prompt_parts.push(format!("# Current Time\n\n{timestamp}"));
        
        Ok(prompt_parts.join("\n\n---\n\n"))
    }
    
    /// Load a context file from the agent's workspace
    async fn load_context_file(&self, filename: &str) -> Result<String> {
        let workspace_path = self.get_workspace_path();
        let file_path = workspace_path.join(filename);
        
        match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => Ok(content),
            Err(e) => {
                debug!("Could not load context file {}: {}", filename, e);
                // Try loading from default locations
                self.load_context_file_fallback(filename).await
            }
        }
    }
    
    /// Try loading context files from fallback locations
    async fn load_context_file_fallback(&self, filename: &str) -> Result<String> {
        // Try standard locations for agent context files
        let fallback_paths = [
            // Check if it exists in the global workspace
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("rockbot")
                .join(filename),
            // Check current directory (for development/testing)
            std::env::current_dir().unwrap_or_default().join(filename),
            // Check home directory
            dirs::home_dir().unwrap_or_default().join(".openclaw").join(filename),
        ];
        
        for path in fallback_paths {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                debug!("Loaded context file {} from fallback location: {}", filename, path.display());
                return Ok(content);
            }
        }
        
        debug!("No fallback location found for context file: {}", filename);
        Err(crate::error::RockBotError::Agent(AgentError::ExecutionFailed {
            message: format!("Context file {filename} not found"),
        }))
    }
    
    /// Build the skills/tools section of the system prompt
    async fn build_skills_section(&self, available_tools: &[String]) -> Result<String> {
        if available_tools.is_empty() {
            return Ok("# Available Tools\n\nNo tools available in this session.".to_string());
        }
        
        let mut skills_parts = vec!["# Available Tools".to_string()];
        
        // Get tool definitions
        let tool_definitions = self.tool_registry.get_tool_definitions(available_tools).await?;
        
        for tool in tool_definitions {
            let tool_description = format!(
                "## {}\n\n{}\n\n**Parameters:** {}", 
                tool.name, 
                tool.description,
                serde_json::to_string_pretty(&tool.parameters).unwrap_or_else(|_| "{}".to_string())
            );
            skills_parts.push(tool_description);
        }
        
        skills_parts.push(
            "Use these tools when appropriate to help the user. Always explain what you're doing when using tools.".to_string()
        );
        
        Ok(skills_parts.join("\n\n"))
    }
    
    /// Call LLM with retry logic and exponential backoff
    async fn call_llm_with_retry(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let retry_config = RetryConfig::default();
        let mut last_error_message = String::new();
        
        for attempt in 0..=retry_config.max_retries {
            debug!("LLM API call attempt {} of {}", attempt + 1, retry_config.max_retries + 1);
            
            match self.llm_provider.chat_completion(request.clone()).await {
                Ok(response) => {
                    if attempt > 0 {
                        info!("LLM API call succeeded after {} retries", attempt);
                        // Update stats
                        {
                            let mut state = self.state.write().await;
                            state.stats.retry_attempts += attempt as u64;
                        }
                    }
                    return Ok(response);
                }
                Err(error) => {
                    last_error_message = error.to_string();
                    let error_category = self.classify_llm_error(&error);
                    
                    debug!("LLM API call failed (attempt {}): {} - Category: {:?}", 
                           attempt + 1, error, error_category);
                    
                    // Update stats
                    {
                        let mut state = self.state.write().await;
                        state.stats.error_count += 1;
                        if matches!(error_category, ErrorCategory::RateLimit { .. }) {
                            state.stats.rate_limit_hits += 1;
                        }
                    }
                    
                    // Check if we should retry
                    if attempt >= retry_config.max_retries || !self.should_retry_error(&error_category) {
                        break;
                    }
                    
                    // Calculate delay with exponential backoff and jitter
                    let delay = self.calculate_retry_delay(&retry_config, attempt, &error_category);
                    
                    warn!("Retrying LLM API call in {}ms due to: {}", delay, error);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
        
        // All retries exhausted
        if last_error_message.is_empty() {
            last_error_message = "Unknown error during LLM API call".to_string();
        }
        
        error!("LLM API call failed after {} retries: {}", retry_config.max_retries, last_error_message);
        Err(crate::error::RockBotError::Agent(AgentError::ModelError { 
            message: format!("LLM API call failed after {} retries: {}", retry_config.max_retries, last_error_message)
        }))
    }
    
    /// Classify an LLM error for retry decisions
    #[allow(clippy::unused_self)]
    fn classify_llm_error(&self, error: &LlmError) -> ErrorCategory {
        match error {
            LlmError::RateLimitExceeded => ErrorCategory::RateLimit { retry_after_ms: None },
            LlmError::AuthenticationFailed => ErrorCategory::Auth,
            LlmError::ModelNotFound { .. } => ErrorCategory::Client,
            LlmError::Request(req_error) => {
                // Classify reqwest errors
                if req_error.is_timeout() || req_error.is_connect() {
                    ErrorCategory::Network
                } else if req_error.is_status() {
                    if let Some(status) = req_error.status() {
                        match status.as_u16() {
                            429 => ErrorCategory::RateLimit { retry_after_ms: None },
                            401 | 403 => ErrorCategory::Auth,
                            400..=499 => ErrorCategory::Client,
                            500..=599 => ErrorCategory::Server,
                            _ => ErrorCategory::Network,
                        }
                    } else {
                        ErrorCategory::Network
                    }
                } else {
                    ErrorCategory::Network
                }
            }
            LlmError::ApiError { message } => {
                // Parse message for specific error types
                let msg_lower = message.to_lowercase();
                if msg_lower.contains("rate limit") || msg_lower.contains("too many requests") {
                    ErrorCategory::RateLimit { retry_after_ms: None }
                } else if msg_lower.contains("auth") || msg_lower.contains("unauthorized") || msg_lower.contains("forbidden") {
                    ErrorCategory::Auth
                } else if msg_lower.contains("invalid") || msg_lower.contains("bad request") {
                    ErrorCategory::Client
                } else {
                    ErrorCategory::Server
                }
            }
            LlmError::Json(_) => ErrorCategory::Client,
        }
    }
    
    /// Determine if an error should be retried
    #[allow(clippy::unused_self)]
    fn should_retry_error(&self, error_category: &ErrorCategory) -> bool {
        match error_category {
            ErrorCategory::Retryable => true,
            ErrorCategory::RateLimit { .. } => true,
            ErrorCategory::Server => true,
            ErrorCategory::Network => true,
            ErrorCategory::Auth => false,
            ErrorCategory::Client => false,
        }
    }
    
    /// Calculate retry delay with exponential backoff and jitter
    #[allow(clippy::unused_self)]
    fn calculate_retry_delay(&self, config: &RetryConfig, attempt: u32, error_category: &ErrorCategory) -> u64 {
        let base_delay = match error_category {
            ErrorCategory::RateLimit { retry_after_ms: Some(delay) } => *delay,
            ErrorCategory::RateLimit { .. } => {
                // Use longer backoff for rate limits
                config.base_delay_ms * 2
            }
            _ => config.base_delay_ms,
        };
        
        // Calculate exponential backoff
        let exponential_delay = (base_delay as f64) * config.backoff_multiplier.powi(attempt as i32);
        
        // Cap at max delay
        let capped_delay = exponential_delay.min(config.max_delay_ms as f64);
        
        // Add jitter to prevent thundering herd
        let jitter_range = capped_delay * config.jitter_factor;
        let jitter = (rand::random::<f64>() - 0.5) * 2.0 * jitter_range;
        
        let final_delay = (capped_delay + jitter).max(100.0); // Minimum 100ms
        
        final_delay as u64
    }
    
    /// Process LLM response and handle any tool calls with multi-turn execution
    async fn process_llm_response(
        &self,
        session_id: &str,
        context: &mut ProcessingContext,
        initial_llm_response: ChatCompletionResponse,
    ) -> Result<(Message, Vec<ToolExecutionResult>, TokenUsage)> {
        let mut all_tool_results = Vec::new();
        let mut cumulative_token_usage = TokenUsage {
            prompt_tokens: initial_llm_response.usage.prompt_tokens,
            completion_tokens: initial_llm_response.usage.completion_tokens,
            total_tokens: initial_llm_response.usage.total_tokens,
        };
        
        let mut current_response = initial_llm_response;
        let mut final_response_content = String::new();
        let mut iteration_count = 0;
        
        // Maximum tool execution iterations to prevent infinite loops
        let max_tool_iterations = self.config.max_tool_calls.unwrap_or(10);
        
        loop {
            iteration_count += 1;
            debug!("Tool execution iteration {} for session {}", iteration_count, session_id);
            
            // Check for iteration limit
            if iteration_count > max_tool_iterations {
                warn!("Maximum tool execution iterations ({}) reached for session {}", 
                      max_tool_iterations, session_id);
                break;
            }
            
            let has_tool_calls = current_response.choices
                .first()
                .and_then(|c| c.message.tool_calls.as_ref())
                .is_some_and(|tc| !tc.is_empty());
            
            if !has_tool_calls {
                // No more tool calls - extract final response and finish
                if let Some(choice) = current_response.choices.first() {
                    if !choice.message.content.is_empty() {
                        final_response_content = choice.message.content.clone();
                    }
                }
                break;
            }
            
            // Execute tool calls and prepare for next iteration
            let (tool_results, tool_messages) = self.execute_tool_calls(
                session_id,
                &current_response,
            ).await?;
            
            all_tool_results.extend(tool_results);
            
            // Add assistant's tool call message to conversation
            if let Some(choice) = current_response.choices.first() {
                let assistant_message = rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::Assistant,
                    content: choice.message.content.clone(),
                    tool_calls: choice.message.tool_calls.clone(),
                };
                context.messages.push(Message::from_llm_message(assistant_message, session_id, &self.config.id)?);
            }
            
            // Add tool result messages to conversation
            for tool_message in tool_messages {
                context.messages.push(tool_message);
            }
            
            // Generate next LLM request with updated conversation including tool results
            let next_llm_request = self.build_llm_request(context).await?;
            
            // Get next LLM response with retry logic
            match self.call_llm_with_retry(next_llm_request).await {
                Ok(response) => {
                    // Accumulate token usage
                    cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                    cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                    cumulative_token_usage.total_tokens += response.usage.total_tokens;
                    
                    current_response = response;
                }
                Err(e) => {
                    error!("LLM error in tool execution loop: {}", e);
                    return Err(e);
                }
            }
        }
        
        // If we didn't get a final response, use the last assistant content
        if final_response_content.is_empty() && !all_tool_results.is_empty() {
            final_response_content = format!("Executed {} tool(s) successfully.", all_tool_results.len());
        }
        
        // Create final response message
        let response_message = Message::text(final_response_content)
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::Assistant);
        
        info!("Completed tool execution loop with {} iterations, {} tool calls", 
              iteration_count, all_tool_results.len());
        
        Ok((response_message, all_tool_results, cumulative_token_usage))
    }
    
    /// Execute tool calls from an LLM response
    async fn execute_tool_calls(
        &self,
        session_id: &str,
        llm_response: &ChatCompletionResponse,
    ) -> Result<(Vec<ToolExecutionResult>, Vec<Message>)> {
        let mut tool_results = Vec::new();
        let mut tool_messages = Vec::new();
        
        if let Some(choice) = llm_response.choices.first() {
            if let Some(ref tool_calls) = choice.message.tool_calls {
                for tool_call in tool_calls {
                    debug!("Executing tool: {}", tool_call.function.name);
                    
                    let execution_context = ToolExecutionContext {
                        session_id: session_id.to_string(),
                        agent_id: self.config.id.clone(),
                        workspace_path: self.get_workspace_path(),
                        security_context: self.security_manager
                            .get_session_context(session_id)
                            .await?,
                        credential_accessor: self.credential_accessor.clone(),
                    };
                    
                    match self.tool_registry
                        .execute_tool(
                            &tool_call.function.name,
                            &tool_call.function.arguments,
                            execution_context,
                        )
                        .await
                    {
                        Ok(result) => {
                            tool_results.push(result.clone());
                            
                            // Create tool result message for conversation
                            let tool_content = match &result.result {
                                ToolResult::Text { content } => content.clone(),
                                ToolResult::Error { message, .. } => {
                                    format!("Error: {message}")
                                }
                                ToolResult::Json { data } => {
                                    serde_json::to_string_pretty(data).unwrap_or_else(|_| "Invalid JSON".to_string())
                                }
                                ToolResult::File { path, .. } => {
                                    format!("[File: {path}]")
                                }
                            };
                            
                            let tool_message = Message::tool_result(
                                tool_call.id.clone(),
                                tool_call.function.name.clone(),
                                tool_content,
                            ).with_session_id(session_id);
                            
                            tool_messages.push(tool_message);
                        }
                        Err(e) => {
                            error!("Tool execution failed: {}", e);
                            
                            // Create error message for conversation
                            let error_message = Message::tool_result(
                                tool_call.id.clone(),
                                tool_call.function.name.clone(),
                                format!("Tool execution failed: {e}"),
                            ).with_session_id(session_id);
                            
                            tool_messages.push(error_message);
                        }
                    }
                }
                
                // Update stats
                {
                    let mut state = self.state.write().await;
                    state.stats.tool_executions += tool_calls.len() as u64;
                }
            }
        }
        
        Ok((tool_results, tool_messages))
    }
    
    /// Get the workspace path for this agent
    fn get_workspace_path(&self) -> std::path::PathBuf {
        self.config.workspace.as_ref()
            .unwrap_or(&dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("rockbot")
                .join("agents")
                .join(&self.config.id))
            .clone()
    }
    
    /// Update agent statistics
    async fn update_stats(&self, tokens_used: u64, processing_time_ms: u64) {
        let mut state = self.state.write().await;
        let stats = &mut state.stats;
        
        stats.messages_processed += 1;
        stats.total_tokens += tokens_used;
        
        // Update rolling average of response time
        if stats.messages_processed == 1 {
            stats.avg_response_time_ms = processing_time_ms;
        } else {
            stats.avg_response_time_ms = 
                (stats.avg_response_time_ms * (stats.messages_processed - 1) + processing_time_ms) 
                / stats.messages_processed;
        }
    }
    
    /// Get agent statistics
    pub async fn get_stats(&self) -> AgentStats {
        let state = self.state.read().await;
        state.stats.clone()
    }
    
    /// Get agent ID
    pub fn id(&self) -> &str {
        &self.config.id
    }
    
    /// Check if agent is healthy
    pub async fn health_check(&self) -> Result<AgentHealthStatus> {
        // Check LLM provider
        let llm_healthy = match self.llm_provider.list_models().await {
            Ok(_) => true,
            Err(e) => {
                warn!("LLM provider health check failed: {}", e);
                false
            }
        };
        
        // Check workspace accessibility
        let workspace_healthy = tokio::fs::metadata(self.get_workspace_path()).await.is_ok();
        
        // Check active contexts
        let state = self.state.read().await;
        let active_contexts = state.active_contexts.len();
        
        Ok(AgentHealthStatus {
            agent_id: self.config.id.clone(),
            llm_healthy,
            workspace_healthy,
            active_contexts,
            stats: state.stats.clone(),
        })
    }
}

/// Agent health status
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentHealthStatus {
    pub agent_id: String,
    pub llm_healthy: bool,
    pub workspace_healthy: bool,
    pub active_contexts: usize,
    pub stats: AgentStats,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    
    #[test]
    fn test_agent_stats_default() {
        let stats = AgentStats::default();
        assert_eq!(stats.messages_processed, 0);
        assert_eq!(stats.tool_executions, 0);
        assert_eq!(stats.total_tokens, 0);
    }
    
    #[test]
    fn test_agent_response_serialization() {
        let response = AgentResponse {
            message: crate::message::Message::text("Hello"),
            tool_results: Vec::new(),
            tokens_used: TokenUsage::default(),
            processing_time_ms: 100,
        };
        
        // Should be serializable
        let json = serde_json::to_string(&response);
        assert!(json.is_ok());
    }
    
    // Integration tests with real components require full infrastructure setup
    // and are better suited for integration test crate
}