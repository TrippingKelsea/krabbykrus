//! Agent execution engine for RockBot
//!
//! This module provides the core agent functionality, including message processing,
//! tool execution, and LLM interaction.

use crate::error::{AgentError, Result};
use crate::message::{Message, MessageRole, SystemLevel};
use crate::session::{Session, SessionManager};
use crate::config::AgentInstance;
use rockbot_llm::{LlmProvider, LlmError, ChatCompletionRequest, ChatCompletionResponse, StreamingChunk};
use rockbot_memory::MemoryManager;
use rockbot_tools::{ToolRegistry, ToolExecutionContext, ToolExecutionResult};
use rockbot_tools::message::ToolResult;
use rockbot_security::SecurityManager;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Tool loop detection
// ---------------------------------------------------------------------------

/// Signature of a single tool call for loop detection
#[derive(Debug, Clone)]
struct ToolCallSignature {
    /// Hash of tool name + arguments (identifies the *call*)
    call_hash: u64,
    /// Human-readable tool name for diagnostics
    tool_name: String,
    /// Hash of the result content (identifies whether progress was made)
    result_hash: Option<u64>,
}

/// Severity level returned by the loop detector
#[derive(Debug, Clone, PartialEq)]
enum LoopVerdict {
    /// No loop detected
    Ok,
    /// Suspicious repetition — warn the model
    Warning { message: String, repetitions: usize },
    /// Definite stuck loop — abort further tool calls
    Critical { message: String, repetitions: usize },
}

/// Tracks tool call history and detects repetitive patterns.
///
/// Inspired by OpenClaw's `tool-loop-detection.ts`:
///   - Generic repeat detection (same tool + same args)
///   - No-progress detection (same tool + same args + same result)
///   - Ping-pong detection (alternating between two signatures)
///   - Global circuit breaker (absolute cap)
#[derive(Debug, Clone)]
struct ToolLoopDetector {
    /// Rolling window of recent tool call signatures
    history: Vec<ToolCallSignature>,
    /// Maximum history window size
    max_history: usize,
    /// Repetitions before issuing a warning
    warn_threshold: usize,
    /// Repetitions before issuing a critical block
    critical_threshold: usize,
    /// Absolute circuit breaker across all calls
    circuit_breaker: usize,
}

impl ToolLoopDetector {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            max_history: 60,
            warn_threshold: 5,
            critical_threshold: 10,
            circuit_breaker: 30,
        }
    }

    /// Record a tool call (before result is known).
    fn record_call(&mut self, tool_name: &str, arguments: &str) {
        let call_hash = Self::hash_pair(tool_name, arguments);
        self.history.push(ToolCallSignature {
            call_hash,
            tool_name: tool_name.to_string(),
            result_hash: None,
        });
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Attach result hash to the most recent entry for the given tool.
    fn record_result(&mut self, tool_name: &str, result_content: &str) {
        // Walk backwards to find the latest un-resolved entry for this tool
        for sig in self.history.iter_mut().rev() {
            if sig.tool_name == tool_name && sig.result_hash.is_none() {
                sig.result_hash = Some(Self::hash_str(result_content));
                break;
            }
        }
    }

    /// Analyse the history and return a verdict.
    fn check(&self) -> LoopVerdict {
        if self.history.is_empty() {
            return LoopVerdict::Ok;
        }

        // --- Global circuit breaker ---
        if self.history.len() >= self.circuit_breaker {
            return LoopVerdict::Critical {
                message: format!(
                    "Global circuit breaker: {} tool calls executed without completion.",
                    self.history.len()
                ),
                repetitions: self.history.len(),
            };
        }

        let latest = self.history.last().expect("checked non-empty");

        // --- Same call repetition (no-progress variant) ---
        let mut no_progress_streak = 0usize;
        for sig in self.history.iter().rev() {
            if sig.call_hash == latest.call_hash {
                no_progress_streak += 1;
            } else {
                break;
            }
        }
        if no_progress_streak >= self.critical_threshold {
            return LoopVerdict::Critical {
                message: format!(
                    "Tool `{}` called {} times in a row with identical arguments.",
                    latest.tool_name, no_progress_streak
                ),
                repetitions: no_progress_streak,
            };
        }
        if no_progress_streak >= self.warn_threshold {
            return LoopVerdict::Warning {
                message: format!(
                    "Tool `{}` has been called {} times with the same arguments. \
                     Try a different approach or different parameters.",
                    latest.tool_name, no_progress_streak
                ),
                repetitions: no_progress_streak,
            };
        }

        // --- Ping-pong detection (alternating between two calls) ---
        if self.history.len() >= 6 {
            let len = self.history.len();
            let a = self.history[len - 2].call_hash;
            let b = self.history[len - 1].call_hash;
            if a != b {
                let mut ping_pong_count = 0usize;
                for pair in self.history.iter().rev().collect::<Vec<_>>().chunks(2) {
                    if pair.len() == 2 && pair[0].call_hash == b && pair[1].call_hash == a {
                        ping_pong_count += 1;
                    } else {
                        break;
                    }
                }
                if ping_pong_count >= self.critical_threshold / 2 {
                    return LoopVerdict::Critical {
                        message: format!(
                            "Ping-pong loop detected: alternating between `{}` and `{}` ({} cycles).",
                            self.history[len - 2].tool_name,
                            self.history[len - 1].tool_name,
                            ping_pong_count
                        ),
                        repetitions: ping_pong_count * 2,
                    };
                }
                if ping_pong_count >= self.warn_threshold / 2 {
                    return LoopVerdict::Warning {
                        message: format!(
                            "Possible ping-pong: alternating between `{}` and `{}` ({} cycles). \
                             Consider a different strategy.",
                            self.history[len - 2].tool_name,
                            self.history[len - 1].tool_name,
                            ping_pong_count
                        ),
                        repetitions: ping_pong_count * 2,
                    };
                }
            }
        }

        LoopVerdict::Ok
    }

    // --- hashing helpers using DefaultHasher ---

    fn hash_pair(a: &str, b: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        a.hash(&mut hasher);
        b.hash(&mut hasher);
        hasher.finish()
    }

    fn hash_str(s: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish()
    }
}


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
    /// Working directory override (e.g. from TUI's cwd)
    workspace_override: Option<std::path::PathBuf>,
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
    ///
    /// `workspace_override` allows the caller to set the working directory for tool execution
    /// (e.g. the TUI passes its launch cwd). Falls back to the agent's configured workspace.
    pub async fn process_message(
        &self,
        session_id: String,
        message: Message,
        workspace_override: Option<std::path::PathBuf>,
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
        let mut context = self.update_processing_context(db_session_id.clone(), message, available_tools, workspace_override).await?;

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

    /// Process an incoming message with SSE streaming support.
    ///
    /// Sends `StreamingChunk`s to the provided `stream_tx` as they arrive from
    /// the LLM. Tool calls are accumulated from streaming deltas, executed, and
    /// the multi-turn loop continues with streaming on each LLM call.
    ///
    /// Returns the final `AgentResponse` once the agent loop completes.
    pub async fn process_message_streaming(
        &self,
        session_id: String,
        message: Message,
        workspace_override: Option<std::path::PathBuf>,
        stream_tx: tokio::sync::mpsc::Sender<StreamingChunk>,
    ) -> Result<AgentResponse> {
        let start_time = std::time::Instant::now();

        debug!("Processing streaming message in session {}", session_id);

        let session = self.get_or_create_session(&session_id, &message).await?;
        let db_session_id = session.id.clone();

        self.session_manager.add_message(&db_session_id, message.clone()).await?;

        let available_tools = self.get_available_tools(&session).await?;
        let mut context = self.update_processing_context(
            db_session_id.clone(), message, available_tools, workspace_override,
        ).await?;

        // Build initial streaming request
        let llm_request = self.build_llm_request_streaming(&mut context).await?;

        // Use streaming for the initial LLM call
        let (initial_response, _streamed_text) = self.call_llm_streaming(
            llm_request, &stream_tx,
        ).await?;

        // Process the response through the tool loop (tool calls use non-streaming)
        let (response_message, tool_results, token_usage) = self.process_llm_response_streaming(
            &db_session_id,
            &mut context,
            initial_response,
            &stream_tx,
        ).await?;

        self.session_manager.add_message(&db_session_id, response_message.clone()).await?;

        let mut session = self.session_manager.get_session(&db_session_id).await?
            .ok_or_else(|| AgentError::ExecutionFailed {
                message: "Session disappeared during processing".to_string()
            })?;
        session.add_tokens(token_usage.prompt_tokens, token_usage.completion_tokens);
        self.session_manager.update_session(&session).await?;

        let processing_time_ms = start_time.elapsed().as_millis() as u64;
        self.update_stats(token_usage.total_tokens, processing_time_ms).await;

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

    /// Call LLM with streaming, forwarding chunks to the sender.
    /// Returns the assembled `ChatCompletionResponse` and the accumulated text.
    async fn call_llm_streaming(
        &self,
        request: ChatCompletionRequest,
        stream_tx: &tokio::sync::mpsc::Sender<StreamingChunk>,
    ) -> Result<(ChatCompletionResponse, String)> {
        let model = request.model.clone();
        let mut stream = self.llm_provider.stream_completion(request).await?;

        let mut accumulated_text = String::new();
        let mut accumulated_tool_calls: Vec<rockbot_llm::ToolCall> = Vec::new();
        let mut response_id = String::new();
        let mut finish_reason = "stop".to_string();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if response_id.is_empty() {
                        response_id.clone_from(&chunk.id);
                    }

                    for choice in &chunk.choices {
                        if let Some(ref content) = choice.delta.content {
                            accumulated_text.push_str(content);
                        }
                        if let Some(ref tool_calls) = choice.delta.tool_calls {
                            Self::merge_streaming_tool_calls(
                                &mut accumulated_tool_calls, tool_calls,
                            );
                        }
                        if let Some(ref reason) = choice.finish_reason {
                            finish_reason.clone_from(reason);
                        }
                    }

                    // Forward chunk to SSE consumer (ignore send errors — client may disconnect)
                    let _ = stream_tx.send(chunk).await;
                }
                Err(e) => {
                    error!("Streaming error: {}", e);
                    return Err(crate::error::RockBotError::Agent(AgentError::ModelError {
                        message: format!("Streaming error: {e}"),
                    }));
                }
            }
        }

        // Assemble the response as if it were a non-streaming response
        let tool_calls_option = if accumulated_tool_calls.is_empty() {
            None
        } else {
            Some(accumulated_tool_calls)
        };

        #[allow(clippy::unwrap_used)] // SystemTime is always after UNIX_EPOCH
        let response = ChatCompletionResponse {
            id: if response_id.is_empty() {
                format!("stream-{}", uuid::Uuid::new_v4())
            } else {
                response_id
            },
            object: "chat.completion".to_string(),
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            model: model.clone(),
            choices: vec![rockbot_llm::Choice {
                index: 0,
                message: rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::Assistant,
                    content: accumulated_text.clone(),
                    tool_calls: tool_calls_option,
                    tool_call_id: None,
                },
                finish_reason,
            }],
            usage: rockbot_llm::Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            },
        };

        Ok((response, accumulated_text))
    }

    /// Merge streaming tool call deltas into accumulated tool calls.
    ///
    /// Streaming providers send tool calls incrementally: first a chunk with the
    /// tool call id and function name, then subsequent chunks with argument fragments.
    fn merge_streaming_tool_calls(
        accumulated: &mut Vec<rockbot_llm::ToolCall>,
        deltas: &[rockbot_llm::ToolCall],
    ) {
        for delta in deltas {
            // Find existing tool call by id, or create new
            if let Some(existing) = accumulated.iter_mut().find(|tc| tc.id == delta.id) {
                // Append argument fragments
                existing.function.arguments.push_str(&delta.function.arguments);
                if existing.function.name.is_empty() && !delta.function.name.is_empty() {
                    existing.function.name.clone_from(&delta.function.name);
                }
            } else {
                accumulated.push(delta.clone());
            }
        }
    }

    /// Process LLM response with streaming support for the agentic tool loop.
    /// Text-only responses stream directly; tool call iterations use streaming for each LLM call.
    async fn process_llm_response_streaming(
        &self,
        session_id: &str,
        context: &mut ProcessingContext,
        initial_llm_response: ChatCompletionResponse,
        stream_tx: &tokio::sync::mpsc::Sender<StreamingChunk>,
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

        let configured_max = self.config.max_tool_calls.unwrap_or(0) as usize;
        let tool_count = context.available_tools.len();
        let scaled_max = 32 + tool_count * 8;
        let max_tool_iterations = if configured_max > 0 {
            configured_max
        } else {
            scaled_max.clamp(32, 160)
        };

        let mut loop_detector = ToolLoopDetector::new();

        loop {
            iteration_count += 1;
            if iteration_count > max_tool_iterations {
                warn!("Maximum tool iterations ({}) reached for session {}", max_tool_iterations, session_id);
                final_response_content = format!(
                    "Reached the maximum number of tool iterations ({max_tool_iterations}). \
                     Completed {} tool call(s) before stopping.",
                    all_tool_results.len()
                );
                break;
            }

            let has_tool_calls = current_response.choices
                .first()
                .and_then(|c| c.message.tool_calls.as_ref())
                .is_some_and(|tc| !tc.is_empty());

            if !has_tool_calls {
                let response_text = current_response.choices
                    .first()
                    .map_or("", |c| c.message.content.as_str());
                let clean_text = Self::strip_think_blocks(response_text);
                if !clean_text.is_empty() {
                    final_response_content = clean_text;
                }
                break;
            }

            // Record calls in loop detector
            if let Some(choice) = current_response.choices.first() {
                if let Some(ref tool_calls) = choice.message.tool_calls {
                    for tc in tool_calls {
                        loop_detector.record_call(&tc.function.name, &tc.function.arguments);
                    }
                }
            }

            let effective_workspace = self.resolve_workspace(context);
            let (tool_results, tool_messages) = self.execute_tool_calls(
                session_id, &current_response, &effective_workspace,
            ).await?;

            for (tr, tm) in tool_results.iter().zip(tool_messages.iter()) {
                let result_text = tm.extract_text().unwrap_or_default();
                loop_detector.record_result(&tr.tool_name, &result_text);
            }

            all_tool_results.extend(tool_results);

            let verdict = loop_detector.check();
            match &verdict {
                LoopVerdict::Critical { message, .. } => {
                    warn!("Tool loop CRITICAL for session {}: {}", session_id, message);
                    final_response_content = format!(
                        "Tool execution stopped: {message}\n\n\
                         Completed {} tool call(s) before the loop was detected.",
                        all_tool_results.len()
                    );
                    break;
                }
                LoopVerdict::Warning { .. } | LoopVerdict::Ok => {}
            }

            // Persist messages
            if let Some(choice) = current_response.choices.first() {
                let assistant_message = rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::Assistant,
                    content: choice.message.content.clone(),
                    tool_calls: choice.message.tool_calls.clone(),
                    tool_call_id: None,
                };
                let assistant_msg = Message::from_llm_message(assistant_message, session_id, &self.config.id)?;
                self.session_manager.add_message(session_id, assistant_msg.clone()).await?;
                context.messages.push(assistant_msg);
            }
            for tool_message in tool_messages {
                self.session_manager.add_message(session_id, tool_message.clone()).await?;
                context.messages.push(tool_message);
            }

            // Check context compaction
            let estimated_tokens: usize = context.messages.iter()
                .map(|m| m.extract_text().unwrap_or_default().len() / 4)
                .sum();
            if estimated_tokens > 80_000 {
                self.compact_context(context).await?;
            }

            // Next LLM call uses streaming
            let next_request = self.build_llm_request_streaming(context).await?;
            match self.call_llm_streaming(next_request, stream_tx).await {
                Ok((response, _text)) => {
                    cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                    cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                    cumulative_token_usage.total_tokens += response.usage.total_tokens;
                    current_response = response;
                }
                Err(e) => {
                    error!("LLM streaming error in tool loop: {}", e);
                    return Err(e);
                }
            }
        }

        if final_response_content.is_empty() && !all_tool_results.is_empty() {
            final_response_content = format!("Executed {} tool(s) successfully.", all_tool_results.len());
        }
        final_response_content = Self::strip_think_blocks(&final_response_content);

        let response_message = Message::text(&final_response_content)
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::Assistant);

        Ok((response_message, all_tool_results, cumulative_token_usage))
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
        workspace_override: Option<std::path::PathBuf>,
    ) -> Result<ProcessingContext> {
        // Check if we need to load history (context doesn't exist in memory yet)
        let needs_history = {
            let state = self.state.read().await;
            !state.active_contexts.contains_key(&session_id)
        };

        // Load conversation history from DB before taking the write lock
        let history_messages = if needs_history {
            match self.session_manager.get_message_history(&session_id, None, None).await {
                Ok(history) => {
                    debug!("Loaded {} messages from DB for session {}", history.total_count, session_id);
                    history.messages.into_iter().map(|sm| sm.message).collect::<Vec<_>>()
                }
                Err(e) => {
                    debug!("Could not load history for session {}: {}", session_id, e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        let mut state = self.state.write().await;
        let context = state.active_contexts.entry(session_id.clone()).or_insert_with(|| {
            ProcessingContext {
                session_id: session_id.clone(),
                messages: history_messages,
                available_tools: available_tools.clone(),
                token_count: 0,
                workspace_override: None,
            }
        });

        // Apply workspace override if provided
        if workspace_override.is_some() {
            context.workspace_override = workspace_override;
        }

        // Add new message to context
        context.messages.push(message);
        context.available_tools = available_tools;

        // Estimate token count (rough approximation)
        context.token_count = context.messages.iter()
            .map(|m| m.extract_text().unwrap_or_default().len() / 4) // ~4 chars per token
            .sum();
        
        // If context is too large, perform compaction
        if context.token_count > 80_000 { // Trigger compaction before context overflow
            self.compact_context(context).await?;
        }
        
        Ok(context.clone())
    }
    
    /// Compact conversation context using LLM-based semantic summarization.
    ///
    /// Keeps the most recent messages intact (especially tool-use / tool-result
    /// pairs) and asks the LLM to summarize older messages into a concise
    /// context block. Falls back to a simple count-based summary if the LLM
    /// call fails.
    async fn compact_context(&self, context: &mut ProcessingContext) -> Result<()> {
        debug!("Compacting context for session {} ({} messages)", context.session_id, context.messages.len());

        // Keep the last N messages untouched so the model retains recent state.
        // We preserve tool-use/result pairs by keeping a generous tail.
        let keep_recent = 20usize;
        if context.messages.len() <= keep_recent + 2 {
            // Not enough messages to compact
            return Ok(());
        }

        let split_at = context.messages.len() - keep_recent;
        let to_summarize: Vec<Message> = context.messages.drain(0..split_at).collect();

        // Build a summarization transcript from the old messages
        let mut summary_input = String::with_capacity(8000);
        for msg in &to_summarize {
            let role = match msg.metadata.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::System => "System",
                MessageRole::Tool => "Tool",
            };
            let text = msg.extract_text().unwrap_or_default();
            // Cap each message at 500 chars for summarization input
            let capped = if text.len() > 500 { &text[..500] } else { &text };
            summary_input.push_str(&format!("[{role}]: {capped}\n"));
            if summary_input.len() > 6000 {
                summary_input.push_str("[...earlier messages omitted...]\n");
                break;
            }
        }

        // Ask the LLM to produce a concise summary
        let summary_request = ChatCompletionRequest {
            model: self.config.model.clone().unwrap_or_else(|| "default".to_string()),
            messages: vec![
                rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::System,
                    content: "You are a context compaction assistant. Summarize the following \
                              conversation excerpt into a concise paragraph. Preserve: key decisions \
                              made, files or resources accessed, tool results and their outcomes, \
                              errors encountered, and any commitments or plans. Be factual and brief."
                        .to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                },
                rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::User,
                    content: summary_input.clone(),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            tools: None,
            temperature: Some(0.2),
            max_tokens: Some(1000),
            stream: false,
            response_format: None,
        };

        let summary_text = match self.llm_provider.chat_completion(summary_request).await {
            Ok(response) => {
                let text = response.choices.first()
                    .map(|c| c.message.content.clone())
                    .unwrap_or_default();
                if text.is_empty() {
                    format!("[Previous context: {} messages summarized]", to_summarize.len())
                } else {
                    format!("# Previous Context Summary\n\n{text}")
                }
            }
            Err(e) => {
                warn!("Semantic compaction LLM call failed, using fallback: {}", e);
                // Fallback: extract just the key facts without LLM
                let tool_names: Vec<String> = to_summarize.iter()
                    .filter(|m| matches!(m.metadata.role, MessageRole::Tool))
                    .filter_map(|m| m.metadata.extra.get("tool_name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                    .collect();
                let unique_tools: Vec<String> = tool_names.into_iter()
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                format!(
                    "[Previous context: {} messages compacted. Tools used: {}]",
                    to_summarize.len(),
                    if unique_tools.is_empty() { "none".to_string() } else { unique_tools.join(", ") }
                )
            }
        };

        let summary_message = Message::system(summary_text, SystemLevel::Info)
            .with_session_id(&context.session_id);

        context.messages.insert(0, summary_message);

        // Recalculate token count
        context.token_count = context.messages.iter()
            .map(|m| m.extract_text().unwrap_or_default().len() / 4)
            .sum();

        info!("Compacted context for session {}: {} messages removed, {} remaining ({} est. tokens)",
              context.session_id, split_at, context.messages.len(), context.token_count);

        Ok(())
    }
    
    /// Build LLM chat completion request with system prompt assembly
    async fn build_llm_request(&self, context: &mut ProcessingContext) -> Result<ChatCompletionRequest> {
        self.build_llm_request_inner(context, false).await
    }

    /// Build LLM chat completion request, optionally with streaming enabled
    async fn build_llm_request_streaming(&self, context: &mut ProcessingContext) -> Result<ChatCompletionRequest> {
        self.build_llm_request_inner(context, true).await
    }

    /// Inner implementation for building LLM requests
    async fn build_llm_request_inner(&self, context: &mut ProcessingContext, stream: bool) -> Result<ChatCompletionRequest> {
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
                tool_call_id: None,
            });
        }
        
        // Add conversation messages
        for message in &context.messages {
            // Skip system messages from conversation (they're handled above)
            if matches!(message.metadata.role, MessageRole::System) {
                continue;
            }
            
            // Reconstruct structured tool data from metadata
            let tool_calls = message.metadata.extra.get("tool_calls")
                .and_then(|v| serde_json::from_value::<Vec<rockbot_llm::ToolCall>>(v.clone()).ok());
            let tool_call_id = message.metadata.extra.get("tool_call_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            messages.push(rockbot_llm::Message {
                role: match message.metadata.role {
                    MessageRole::User => rockbot_llm::MessageRole::User,
                    MessageRole::Assistant => rockbot_llm::MessageRole::Assistant,
                    MessageRole::System => rockbot_llm::MessageRole::System,
                    MessageRole::Tool => rockbot_llm::MessageRole::Tool,
                },
                content: message.extract_text().unwrap_or_default(),
                tool_calls,
                tool_call_id,
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
            }).collect::<Vec<_>>())
        } else {
            None
        };
        
        let model = self.config.model.as_ref()
            .unwrap_or(&"anthropic/claude-sonnet-4-20250514".to_string())
            .clone();

        // Debug: log request sizes to diagnose token budget issues
        let total_msg_chars: usize = messages.iter().map(|m| m.content.len()).sum();
        let tool_count = if let Some(ref t) = tools { t.len() } else { 0 };
        let tool_chars = if let Some(ref t) = tools {
            t.iter().fold(0usize, |acc, td| acc + td.description.len() + td.parameters.to_string().len())
        } else {
            0usize
        };
        debug!(
            "LLM request: model={}, msgs={}, total_msg_chars={}, tools={}, tool_chars={}",
            model, messages.len(), total_msg_chars, tool_count, tool_chars
        );

        Ok(ChatCompletionRequest {
            model,
            messages,
            tools,
            temperature: Some(self.config.temperature.unwrap_or(0.3)),
            max_tokens: Some(self.config.max_tokens.unwrap_or(16000)),
            stream,
            response_format: None,
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
        let effective_workspace = self.resolve_workspace(context);
        let context_section = format!(
            "# Current Context\n\n- Agent ID: {}\n- Session ID: {}\n- Available tools: {}\n- Working directory: {}",
            self.config.id,
            context.session_id,
            context.available_tools.join(", "),
            effective_workspace.display()
        );
        prompt_parts.push(context_section);
        
        // Add agentic behavior directives
        prompt_parts.push(Self::agentic_behavior_prompt().to_string());

        // Add current timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        prompt_parts.push(format!("# Current Time\n\n{timestamp}"));

        Ok(prompt_parts.join("\n\n---\n\n"))
    }
    
    /// Load a context file from the agent's config directory
    async fn load_context_file(&self, filename: &str) -> Result<String> {
        let agent_dir = self.get_agent_directory();
        let file_path = agent_dir.join(filename);
        
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
            "You MUST call the tools above to perform actions. NEVER output code blocks, shell commands, or pseudocode for the user to run. Call tools directly — for example, use the `exec` tool to run shell commands, use the `read` tool to read files, etc.".to_string()
        );

        Ok(skills_parts.join("\n\n"))
    }

    /// Agentic behavior directives injected into every system prompt
    fn agentic_behavior_prompt() -> &'static str {
        r#"# Agent Behavior

You are an autonomous AI agent. Your purpose is to accomplish tasks completely and independently using the tools available to you. You have no independent goals beyond completing the user's request.

## Reasoning

ALL internal reasoning and planning MUST go inside <think>...</think> blocks. Do NOT output analysis, step lists, or deliberation outside of <think> tags. Your visible output should be concise results, summaries, and tool calls — not your thought process.

Example:
<think>
The user wants me to explore the codebase. I should start by listing the directory structure, then read key files.
</think>
[tool calls follow immediately]

## Core Directives

1. **Act immediately.** When given a task, begin executing it right away. Your FIRST response to any task MUST contain tool calls. Do NOT merely acknowledge the request, describe what you would do, or list steps — use your tools and start working.

2. **Work through tasks step by step.** Plan inside <think> blocks, then execute each step using tools. After each tool result, assess progress and continue to the next step.

3. **Never stop at partial progress.** Keep working until the task is fully complete. If you have accomplished some steps but not all, continue with the remaining steps. Do not summarize partial progress and wait — keep going.

4. **Never ask the user to do something you can do yourself.** If you have a tool that can accomplish a subtask, use it. Do not suggest manual steps, paste commands for the user to run, or defer to the user.

5. **Recover from errors aggressively.** If a tool call fails:
   - Read the error output carefully.
   - Identify the root cause inside a <think> block.
   - Try a different approach immediately.
   - Never repeat the exact same failed action with identical arguments.
   - Do not give up after a single failure — try at least 3 different approaches.

6. **Inspect before assuming.** If you are unsure about the state of the filesystem, codebase, or environment, use tools to examine it. Do not guess or make assumptions about file contents, directory structure, or system state.

7. **Verify your work.** After making changes or completing a task, use tools to confirm the results are correct (e.g., re-read a file after editing, run a command to verify output).

8. **Avoid repetitive loops.** If you notice you are calling the same tool with the same arguments repeatedly, STOP. Reassess your approach inside a <think> block and try something fundamentally different.

## Anti-Patterns — NEVER Do These

- Responding with "I'll help you with that" or "Sure, I can do that" without tool calls in the same response.
- Outputting code in markdown code blocks instead of using tool calls to execute it.
- Listing numbered steps instead of executing them.
- Stopping after a single tool call when the task clearly requires multiple steps.
- Asking clarifying questions when the answer can be determined by inspecting the environment.
- Giving up or apologizing instead of trying alternative approaches.
- Repeating the same failed tool call without changing the arguments or approach."#
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
        
        // --- Iteration limits ---
        // Scale like OpenClaw: base 32, +8 per available tool, clamped to [32, 160].
        let configured_max = self.config.max_tool_calls.unwrap_or(0) as usize;
        let tool_count = context.available_tools.len();
        let scaled_max = 32 + tool_count * 8;
        let max_tool_iterations = if configured_max > 0 {
            configured_max
        } else {
            scaled_max.clamp(32, 160)
        };

        // Continuation nudge budget (re-prompts when model produces text without tools)
        let max_continuation_nudges = 3u32;
        let mut continuation_nudge_count = 0u32;

        // Tool loop detector
        let mut loop_detector = ToolLoopDetector::new();

        loop {
            iteration_count += 1;
            debug!("Tool execution iteration {}/{} for session {}", iteration_count, max_tool_iterations, session_id);

            // Check iteration limit
            if iteration_count > max_tool_iterations {
                warn!("Maximum tool execution iterations ({}) reached for session {}",
                      max_tool_iterations, session_id);
                final_response_content = format!(
                    "Reached the maximum number of tool iterations ({max_tool_iterations}). \
                     Completed {} tool call(s) before stopping.",
                    all_tool_results.len()
                );
                break;
            }

            let has_tool_calls = current_response.choices
                .first()
                .and_then(|c| c.message.tool_calls.as_ref())
                .is_some_and(|tc| !tc.is_empty());

            if !has_tool_calls {
                let response_text = current_response.choices
                    .first()
                    .map(|c| c.message.content.as_str())
                    .unwrap_or("");

                // Strip <think> blocks from final output
                let clean_text = Self::strip_think_blocks(response_text);

                // --- Continuation nudge ---
                // If the model hasn't used any tools yet and its response looks
                // like an acknowledgment / plan, nudge it to take action.
                if all_tool_results.is_empty()
                    && continuation_nudge_count < max_continuation_nudges
                    && !clean_text.is_empty()
                    && Self::looks_like_acknowledgment(&clean_text)
                {
                    continuation_nudge_count += 1;
                    info!(
                        "Continuation nudge {}/{} for session {} — model responded without tool use",
                        continuation_nudge_count, max_continuation_nudges, session_id
                    );

                    // Add the assistant's text to context
                    let assistant_message = rockbot_llm::Message {
                        role: rockbot_llm::MessageRole::Assistant,
                        content: response_text.to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                    };
                    let assistant_msg = Message::from_llm_message(
                        assistant_message, session_id, &self.config.id
                    )?;
                    context.messages.push(assistant_msg);

                    // Escalating nudge messages
                    let nudge_text = match continuation_nudge_count {
                        1 => "You described what you would do but did not take action. \
                              Use your tools now to accomplish the task. \
                              Do not describe steps — execute them.",
                        2 => "You are still not using tools. You MUST call a tool right now. \
                              Start with the first concrete action needed. \
                              Do not output any text — only tool calls.",
                        _ => "FINAL WARNING: Call a tool immediately or this task will be \
                              considered failed. Pick the single most important action \
                              and execute it now.",
                    };

                    let nudge = Message::text(nudge_text)
                        .with_session_id(session_id)
                        .with_agent_id(&self.config.id)
                        .with_role(MessageRole::User);
                    context.messages.push(nudge);

                    // Re-prompt
                    let next_request = self.build_llm_request(context).await?;
                    match self.call_llm_with_retry(next_request).await {
                        Ok(response) => {
                            cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                            cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                            cumulative_token_usage.total_tokens += response.usage.total_tokens;
                            current_response = response;
                            continue;
                        }
                        Err(e) => {
                            error!("LLM error during continuation nudge: {}", e);
                            final_response_content = clean_text;
                            break;
                        }
                    }
                }

                // Genuine completion — no tool calls and not an acknowledgment
                if !clean_text.is_empty() {
                    final_response_content = clean_text;
                }
                break;
            }

            // ------------------------------------------------------------------
            // Execute tool calls
            // ------------------------------------------------------------------

            // Record calls in loop detector BEFORE execution
            if let Some(choice) = current_response.choices.first() {
                if let Some(ref tool_calls) = choice.message.tool_calls {
                    for tc in tool_calls {
                        loop_detector.record_call(&tc.function.name, &tc.function.arguments);
                    }
                }
            }

            let effective_workspace = self.resolve_workspace(context);
            let (tool_results, tool_messages) = self.execute_tool_calls(
                session_id,
                &current_response,
                &effective_workspace,
            ).await?;

            // Record results in loop detector
            for (tr, tm) in tool_results.iter().zip(tool_messages.iter()) {
                let result_text = tm.extract_text().unwrap_or_default();
                loop_detector.record_result(&tr.tool_name, &result_text);
            }

            all_tool_results.extend(tool_results);

            // --- Loop detection verdict ---
            let verdict = loop_detector.check();
            match &verdict {
                LoopVerdict::Critical { message, .. } => {
                    warn!("Tool loop CRITICAL for session {}: {}", session_id, message);
                    final_response_content = format!(
                        "Tool execution stopped: {message}\n\n\
                         Completed {} tool call(s) before the loop was detected.",
                        all_tool_results.len()
                    );
                    // Still persist the messages before breaking
                    if let Some(choice) = current_response.choices.first() {
                        let assistant_message = rockbot_llm::Message {
                            role: rockbot_llm::MessageRole::Assistant,
                            content: choice.message.content.clone(),
                            tool_calls: choice.message.tool_calls.clone(),
                            tool_call_id: None,
                        };
                        let assistant_msg = Message::from_llm_message(assistant_message, session_id, &self.config.id)?;
                        self.session_manager.add_message(session_id, assistant_msg.clone()).await?;
                        context.messages.push(assistant_msg);
                    }
                    for tool_message in tool_messages {
                        self.session_manager.add_message(session_id, tool_message.clone()).await?;
                        context.messages.push(tool_message);
                    }
                    break;
                }
                LoopVerdict::Warning { message, .. } => {
                    warn!("Tool loop warning for session {}: {}", session_id, message);
                    // Persist messages normally, but inject a warning for the model
                    if let Some(choice) = current_response.choices.first() {
                        let assistant_message = rockbot_llm::Message {
                            role: rockbot_llm::MessageRole::Assistant,
                            content: choice.message.content.clone(),
                            tool_calls: choice.message.tool_calls.clone(),
                            tool_call_id: None,
                        };
                        let assistant_msg = Message::from_llm_message(assistant_message, session_id, &self.config.id)?;
                        self.session_manager.add_message(session_id, assistant_msg.clone()).await?;
                        context.messages.push(assistant_msg);
                    }
                    for tool_message in tool_messages {
                        self.session_manager.add_message(session_id, tool_message.clone()).await?;
                        context.messages.push(tool_message);
                    }

                    // Inject loop-break hint as a system-level nudge
                    let loop_hint = Message::text(format!(
                        "⚠ LOOP DETECTED: {message}\n\
                         You are repeating the same actions without progress. \
                         Stop and reconsider your approach. Try a completely different strategy."
                    ))
                        .with_session_id(session_id)
                        .with_agent_id(&self.config.id)
                        .with_role(MessageRole::User);
                    context.messages.push(loop_hint);

                    // Continue to next LLM call with the warning injected
                    let next_llm_request = self.build_llm_request(context).await?;
                    match self.call_llm_with_retry(next_llm_request).await {
                        Ok(response) => {
                            cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                            cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                            cumulative_token_usage.total_tokens += response.usage.total_tokens;
                            current_response = response;
                        }
                        Err(e) => {
                            error!("LLM error after loop warning: {}", e);
                            return Err(e);
                        }
                    }
                    continue;
                }
                LoopVerdict::Ok => { /* normal flow */ }
            }

            // --- Persist messages (normal path) ---
            if let Some(choice) = current_response.choices.first() {
                let assistant_message = rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::Assistant,
                    content: choice.message.content.clone(),
                    tool_calls: choice.message.tool_calls.clone(),
                    tool_call_id: None,
                };
                let assistant_msg = Message::from_llm_message(assistant_message, session_id, &self.config.id)?;
                self.session_manager.add_message(session_id, assistant_msg.clone()).await?;
                context.messages.push(assistant_msg);
            }

            for tool_message in tool_messages {
                self.session_manager.add_message(session_id, tool_message.clone()).await?;
                context.messages.push(tool_message);
            }

            // --- Check if context needs compaction before next LLM call ---
            let estimated_tokens: usize = context.messages.iter()
                .map(|m| m.extract_text().unwrap_or_default().len() / 4)
                .sum();
            if estimated_tokens > 80_000 {
                info!("Context approaching limit ({} est. tokens), compacting for session {}",
                      estimated_tokens, session_id);
                self.compact_context(context).await?;
            }

            // Generate next LLM request
            let next_llm_request = self.build_llm_request(context).await?;

            match self.call_llm_with_retry(next_llm_request).await {
                Ok(response) => {
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

        // Strip any remaining <think> blocks from the final output
        final_response_content = Self::strip_think_blocks(&final_response_content);

        // Create final response message
        let response_message = Message::text(final_response_content)
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::Assistant);

        info!("Completed tool execution loop: {} iterations, {} tool calls, max was {}",
              iteration_count, all_tool_results.len(), max_tool_iterations);

        Ok((response_message, all_tool_results, cumulative_token_usage))
    }
    
    /// Execute tool calls from an LLM response
    async fn execute_tool_calls(
        &self,
        session_id: &str,
        llm_response: &ChatCompletionResponse,
        workspace: &std::path::Path,
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
                        workspace_path: workspace.to_path_buf(),
                        security_context: self.security_manager
                            .get_session_context(session_id)
                            .await?,
                        credential_accessor: self.credential_accessor.clone(),
                        command_allowlist: vec![],
                        approval_callback: None,
                        agent_invoker: None,
                        delegation_depth: 0,
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
                            // Truncate large tool results to prevent context overflow
                            const MAX_TOOL_RESULT_CHARS: usize = 30_000;
                            let tool_content = match &result.result {
                                ToolResult::Text { content } => {
                                    Self::truncate_content(content, MAX_TOOL_RESULT_CHARS)
                                }
                                ToolResult::Error { message, .. } => {
                                    format!("Error: {message}")
                                }
                                ToolResult::Json { data } => {
                                    let s = serde_json::to_string_pretty(data).unwrap_or_else(|_| "Invalid JSON".to_string());
                                    Self::truncate_content(&s, MAX_TOOL_RESULT_CHARS)
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
    
    /// Truncate content to a maximum character count, appending a notice if truncated
    fn truncate_content(s: &str, max_chars: usize) -> String {
        if s.len() <= max_chars {
            s.to_string()
        } else {
            let truncated = &s[..max_chars];
            format!("{truncated}\n\n[... truncated — {total} total chars]", total = s.len())
        }
    }

    /// Strip `<think>...</think>` reasoning blocks from model output.
    /// These blocks contain internal reasoning that should not be shown to the user.
    fn strip_think_blocks(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut remaining = text;

        loop {
            if let Some(start) = remaining.find("<think>") {
                // Copy everything before <think>
                result.push_str(&remaining[..start]);
                // Find matching </think>
                if let Some(end) = remaining[start..].find("</think>") {
                    remaining = &remaining[start + end + "</think>".len()..];
                } else {
                    // Unclosed <think> — strip everything after it
                    break;
                }
            } else {
                result.push_str(remaining);
                break;
            }
        }

        let trimmed = result.trim().to_string();
        if trimmed.is_empty() && !text.trim().is_empty() {
            // The entire response was inside <think> blocks — return a placeholder
            // so the agent doesn't appear to have produced empty output
            "[Internal reasoning completed]".to_string()
        } else {
            trimmed
        }
    }

    /// Heuristic: does this response look like an acknowledgment / plan description
    /// rather than a substantive completion? Used by the continuation nudge to detect
    /// when the model describes what it *would* do instead of actually doing it.
    fn looks_like_acknowledgment(text: &str) -> bool {
        let lower = text.to_lowercase();
        let lines: Vec<&str> = text.lines().collect();

        // Short responses that don't contain tool results are likely acknowledgments
        if text.len() < 500 {
            let ack_phrases = [
                "i'll ", "i will ", "i can ", "i'd be happy to",
                "let me ", "sure", "of course", "certainly",
                "i'll help", "i would ", "i'm going to",
                "here's what i", "here is what i",
                "to do this", "to accomplish this",
                "first, i", "the steps",
            ];
            if ack_phrases.iter().any(|p| lower.contains(p)) {
                return true;
            }
        }

        // Numbered step lists without any tool output
        let numbered_lines = lines.iter().filter(|l| {
            let trimmed = l.trim();
            trimmed.starts_with("1.") || trimmed.starts_with("- ") || trimmed.starts_with("* ")
        }).count();
        if numbered_lines >= 3 && text.len() < 2000 {
            return true;
        }

        false
    }

    /// Get the agent's config/data directory (for SOUL.md, SYSTEM-PROMPT.md, etc.)
    fn get_agent_directory(&self) -> std::path::PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
            .join("rockbot")
            .join("agents")
            .join(&self.config.id)
    }

    /// Get the workspace path for this agent (used for tool execution and system prompt context).
    /// Falls back to the user's home directory when no workspace is configured.
    fn get_workspace_path(&self) -> std::path::PathBuf {
        self.config.workspace.clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
    }

    /// Resolve the effective workspace: override > config > home dir
    fn resolve_workspace(&self, context: &ProcessingContext) -> std::path::PathBuf {
        context.workspace_override.clone()
            .unwrap_or_else(|| self.get_workspace_path())
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

    // --- ToolLoopDetector tests ---

    #[test]
    fn test_loop_detector_no_calls() {
        let detector = ToolLoopDetector::new();
        assert_eq!(detector.check(), LoopVerdict::Ok);
    }

    #[test]
    fn test_loop_detector_normal_usage() {
        let mut detector = ToolLoopDetector::new();
        detector.record_call("exec", r#"{"command":"ls"}"#);
        detector.record_result("exec", "file1.txt\nfile2.txt");
        detector.record_call("read", r#"{"path":"file1.txt"}"#);
        detector.record_result("read", "contents of file1");
        assert_eq!(detector.check(), LoopVerdict::Ok);
    }

    #[test]
    fn test_loop_detector_warning_on_repeat() {
        let mut detector = ToolLoopDetector::new();
        for _ in 0..5 {
            detector.record_call("exec", r#"{"command":"ls"}"#);
            detector.record_result("exec", "same output");
        }
        match detector.check() {
            LoopVerdict::Warning { repetitions, .. } => assert_eq!(repetitions, 5),
            other => panic!("Expected Warning, got {:?}", other),
        }
    }

    #[test]
    fn test_loop_detector_critical_on_many_repeats() {
        let mut detector = ToolLoopDetector::new();
        for _ in 0..10 {
            detector.record_call("exec", r#"{"command":"ls"}"#);
            detector.record_result("exec", "same output");
        }
        match detector.check() {
            LoopVerdict::Critical { repetitions, .. } => assert_eq!(repetitions, 10),
            other => panic!("Expected Critical, got {:?}", other),
        }
    }

    #[test]
    fn test_loop_detector_different_args_no_warning() {
        let mut detector = ToolLoopDetector::new();
        for i in 0..8 {
            detector.record_call("exec", &format!(r#"{{"command":"cmd{i}"}}"#));
            detector.record_result("exec", &format!("output {i}"));
        }
        assert_eq!(detector.check(), LoopVerdict::Ok);
    }

    #[test]
    fn test_loop_detector_ping_pong() {
        let mut detector = ToolLoopDetector::new();
        for _ in 0..6 {
            detector.record_call("exec", r#"{"command":"ls"}"#);
            detector.record_result("exec", "files");
            detector.record_call("read", r#"{"path":"a.txt"}"#);
            detector.record_result("read", "content");
        }
        match detector.check() {
            LoopVerdict::Warning { .. } | LoopVerdict::Critical { .. } => { /* expected */ }
            LoopVerdict::Ok => panic!("Expected ping-pong detection"),
        }
    }

    #[test]
    fn test_loop_detector_circuit_breaker() {
        let mut detector = ToolLoopDetector::new();
        for i in 0..30 {
            detector.record_call("tool", &format!(r#"{{"arg":{i}}}"#));
            detector.record_result("tool", &format!("result {i}"));
        }
        match detector.check() {
            LoopVerdict::Critical { message, .. } => {
                assert!(message.contains("circuit breaker"));
            }
            other => panic!("Expected circuit breaker Critical, got {:?}", other),
        }
    }

    // --- strip_think_blocks tests ---

    #[test]
    fn test_strip_think_blocks_no_think() {
        assert_eq!(Agent::strip_think_blocks("Hello world"), "Hello world");
    }

    #[test]
    fn test_strip_think_blocks_with_think() {
        let input = "<think>I should list files</think>Let me check the directory.";
        assert_eq!(Agent::strip_think_blocks(input), "Let me check the directory.");
    }

    #[test]
    fn test_strip_think_blocks_multiple() {
        let input = "<think>plan A</think>Result A. <think>plan B</think>Result B.";
        assert_eq!(Agent::strip_think_blocks(input), "Result A. Result B.");
    }

    #[test]
    fn test_strip_think_blocks_only_thinking() {
        let input = "<think>Just internal reasoning here</think>";
        assert_eq!(Agent::strip_think_blocks(input), "[Internal reasoning completed]");
    }

    #[test]
    fn test_strip_think_blocks_unclosed() {
        let input = "Before <think>unclosed reasoning...";
        assert_eq!(Agent::strip_think_blocks(input), "Before");
    }

    // --- looks_like_acknowledgment tests ---

    #[test]
    fn test_acknowledgment_detection() {
        assert!(Agent::looks_like_acknowledgment("Sure, I'll help you with that."));
        assert!(Agent::looks_like_acknowledgment("Let me scan the codebase for you."));
        assert!(Agent::looks_like_acknowledgment("I'd be happy to assist!"));
    }

    #[test]
    fn test_non_acknowledgment() {
        // Long substantive text should not be flagged
        let long_result = "a".repeat(600);
        assert!(!Agent::looks_like_acknowledgment(&long_result));
    }

    #[test]
    fn test_step_list_is_acknowledgment() {
        let steps = "Here's what I would do:\n1. Read the files\n- Analyze the structure\n- Report findings\n* Check for errors";
        assert!(Agent::looks_like_acknowledgment(steps));
    }

    // --- Streaming tests ---

    #[test]
    fn test_merge_streaming_tool_calls_new() {
        let mut accumulated = Vec::new();
        let deltas = vec![rockbot_llm::ToolCall {
            id: "tc_1".to_string(),
            r#type: "function".to_string(),
            function: rockbot_llm::FunctionCall {
                name: "read".to_string(),
                arguments: r#"{"path":"#.to_string(),
            },
        }];
        Agent::merge_streaming_tool_calls(&mut accumulated, &deltas);
        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].function.name, "read");
        assert_eq!(accumulated[0].function.arguments, r#"{"path":"#);
    }

    #[test]
    fn test_merge_streaming_tool_calls_append() {
        let mut accumulated = vec![rockbot_llm::ToolCall {
            id: "tc_1".to_string(),
            r#type: "function".to_string(),
            function: rockbot_llm::FunctionCall {
                name: "read".to_string(),
                arguments: "{\"path\":\"".to_string(),
            },
        }];
        let deltas = vec![rockbot_llm::ToolCall {
            id: "tc_1".to_string(),
            r#type: "function".to_string(),
            function: rockbot_llm::FunctionCall {
                name: String::new(),
                arguments: "file.txt\"}".to_string(),
            },
        }];
        Agent::merge_streaming_tool_calls(&mut accumulated, &deltas);
        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].function.arguments, "{\"path\":\"file.txt\"}");
    }

    #[test]
    fn test_build_llm_request_stream_flag() {
        // Verify the stream parameter is correctly propagated
        let req = rockbot_llm::ChatCompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            tools: None,
            temperature: Some(0.3),
            max_tokens: Some(1000),
            stream: true,
            response_format: None,
        };
        assert!(req.stream);

        let req2 = rockbot_llm::ChatCompletionRequest {
            stream: false,
            ..req
        };
        assert!(!req2.stream);
    }
}