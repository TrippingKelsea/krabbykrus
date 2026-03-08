//! Agent execution engine for Krabbykrus
//!
//! This module provides the core agent functionality, including message processing,
//! tool execution, and LLM interaction.

use crate::error::{AgentError, Result};
use crate::message::{Message, MessageRole, SystemLevel};
use crate::session::{Session, SessionManager};
use crate::config::AgentInstance;
use krabbykrus_llm::{LlmProvider, ChatCompletionRequest, ChatCompletionResponse};
use krabbykrus_memory::MemoryManager;
use krabbykrus_tools::{ToolRegistry, ToolExecutionContext, ToolExecutionResult};
use krabbykrus_tools::message::ToolResult;
use krabbykrus_security::SecurityManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Agent execution engine
pub struct Agent {
    /// Agent configuration
    pub config: AgentInstance,
    /// LLM provider for this agent
    llm_provider: Arc<dyn LlmProvider>,
    /// Tool registry
    tool_registry: Arc<ToolRegistry>,
    /// Memory manager
    memory_manager: Arc<MemoryManager>,
    /// Security manager
    security_manager: Arc<SecurityManager>,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Credential accessor for tool credential injection
    credential_accessor: Option<Arc<dyn krabbykrus_tools::CredentialAccessor>>,
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
        credential_accessor: Option<Arc<dyn krabbykrus_tools::CredentialAccessor>>,
    ) -> Result<Self> {
        info!("Initializing agent '{}'", config.id);
        
        // Initialize agent workspace if it doesn't exist
        let default_workspace = dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("krabbykrus")
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
        
        // Get or create session
        let session = self.get_or_create_session(&session_id, &message).await?;
        
        // Store incoming message
        self.session_manager.add_message(&session_id, message.clone()).await?;
        
        // Update processing context
        let available_tools = self.get_available_tools(&session).await?;
        let mut context = self.update_processing_context(session_id.clone(), message, available_tools).await?;
        
        // Generate LLM request
        let llm_request = self.build_llm_request(&mut context).await?;
        
        // Call LLM
        let llm_response = self.llm_provider.chat_completion(llm_request).await
            .map_err(|e| AgentError::ModelError { message: e.to_string() })?;
        
        // Process LLM response and handle tool calls
        let (response_message, tool_results, token_usage) = self.process_llm_response(
            &session_id,
            &mut context,
            llm_response,
        ).await?;
        
        // Store response message
        self.session_manager.add_message(&session_id, response_message.clone()).await?;
        
        // Update session token stats
        let mut session = self.session_manager.get_session(&session_id).await?
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
            state.active_contexts.remove(&session_id);
        }
        
        Ok(AgentResponse {
            message: response_message,
            tool_results,
            tokens_used: token_usage,
            processing_time_ms,
        })
    }
    
    /// Get or create a session for the given session ID
    async fn get_or_create_session(&self, session_id: &str, message: &Message) -> Result<Session> {
        if let Some(session) = self.session_manager.get_session(session_id).await? {
            Ok(session)
        } else {
            // Create new session using message metadata if available
            let default_session_key = format!("session-{}", Uuid::new_v4());
            let session_key = message.metadata.source.as_ref()
                .unwrap_or(&default_session_key);
            
            self.session_manager.create_session(&self.config.id, session_key).await
        }
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
    
    /// Build LLM chat completion request
    async fn build_llm_request(&self, context: &mut ProcessingContext) -> Result<ChatCompletionRequest> {
        // Convert messages to LLM format
        let messages = context.messages.iter().map(|m| {
            krabbykrus_llm::Message {
                role: match m.metadata.role {
                    MessageRole::User => krabbykrus_llm::MessageRole::User,
                    MessageRole::Assistant => krabbykrus_llm::MessageRole::Assistant,
                    MessageRole::System => krabbykrus_llm::MessageRole::System,
                    MessageRole::Tool => krabbykrus_llm::MessageRole::Tool,
                },
                content: m.extract_text().unwrap_or_default(),
                tool_calls: None, // TODO: Handle tool calls
            }
        }).collect();
        
        // Get tool definitions if tools are available
        let tools = if !context.available_tools.is_empty() {
            let tool_defs = self.tool_registry.get_tool_definitions(&context.available_tools).await?;
            // Convert from krabbykrus_tools::ToolDefinition to krabbykrus_llm::ToolDefinition
            Some(tool_defs.into_iter().map(|td| {
                krabbykrus_llm::ToolDefinition {
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
                .map(|tc| !tc.is_empty())
                .unwrap_or(false);
            
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
                let assistant_message = krabbykrus_llm::Message {
                    role: krabbykrus_llm::MessageRole::Assistant,
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
            
            // Get next LLM response
            match self.llm_provider.chat_completion(next_llm_request).await {
                Ok(response) => {
                    // Accumulate token usage
                    cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                    cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                    cumulative_token_usage.total_tokens += response.usage.total_tokens;
                    
                    current_response = response;
                }
                Err(e) => {
                    error!("LLM error in tool execution loop: {}", e);
                    return Err(crate::error::KrabbykrusError::Agent(AgentError::ModelError { message: e.to_string() }));
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
                                    format!("Error: {}", message)
                                }
                                ToolResult::Json { data } => {
                                    serde_json::to_string_pretty(data).unwrap_or_else(|_| "Invalid JSON".to_string())
                                }
                                ToolResult::File { path, .. } => {
                                    format!("[File: {}]", path)
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
                                format!("Tool execution failed: {}", e),
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
                .join("krabbykrus")
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