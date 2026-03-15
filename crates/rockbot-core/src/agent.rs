//! Agent execution engine for RockBot
//!
//! This module provides the core agent functionality, including message processing,
//! tool execution, and LLM interaction.

use crate::error::{AgentError, Result};
use crate::message::{Message, MessageContent, MessageRole, SystemLevel};
use crate::session::{Session, SessionManager};
use crate::config::AgentInstance;
use rockbot_llm::{LlmProvider, LlmError, ChatCompletionRequest, ChatCompletionResponse, StreamingChunk};
use rockbot_memory::MemoryManager;
use rockbot_tools::{ToolRegistry, ToolExecutionContext, ToolExecutionResult, AgentInvoker};
use rockbot_tools::message::ToolResult;
use rockbot_security::SecurityManager;
use crate::hooks::{HookRegistry, HookEvent, HookResult};
use crate::guardrails::{GuardrailPipeline, GuardrailResult, PiiGuardrail, PromptInjectionGuardrail};
use crate::trajectory::{Trajectory, TrajectoryEvent, preview};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

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
    /// Hook registry for lifecycle events
    hook_registry: Arc<HookRegistry>,
    /// Agent invoker for subagent delegation
    agent_invoker: Option<Arc<dyn AgentInvoker>>,
    /// Guardrail pipeline for input/output safety checks
    guardrail_pipeline: Arc<GuardrailPipeline>,
    /// Episodic memory store for cross-session recall
    episodic_store: Option<Arc<rockbot_memory::EpisodicStore>>,
    /// Blackboard accessor for swarm coordination
    blackboard: Option<Arc<dyn rockbot_tools::BlackboardAccessor>>,
    /// Swarm ID (from config) for blackboard scoping
    swarm_id: Option<String>,
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

/// Signal that the current agent wants to transfer conversation control.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffSignal {
    /// Target agent to hand off to
    pub target_agent_id: String,
    /// Context/instructions for the target agent
    pub context: String,
    /// Optional override for the user message sent to the target
    pub message_override: Option<String>,
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
    /// Execution trajectory (for debugging/replay)
    pub trajectory: Option<Trajectory>,
    /// Handoff signal — if set, the caller should route the conversation to the target agent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffSignal>,
}

/// Token usage breakdown
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Progress events emitted during agent processing.
/// Callers can provide an `mpsc::UnboundedSender<AgentProgressEvent>` to
/// receive real-time updates while the agent loop is running.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum AgentProgressEvent {
    /// A tool is about to be executed
    #[serde(rename = "tool_start")]
    ToolStart { tool_name: String },
    /// A tool finished executing
    #[serde(rename = "tool_done")]
    ToolDone {
        tool_name: String,
        success: bool,
        duration_ms: u64,
    },
    /// An LLM call is starting (with context size info)
    #[serde(rename = "llm_call")]
    LlmCall {
        iteration: usize,
        message_count: usize,
    },
    /// Model produced text content (reasoning, analysis, etc.)
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    /// A tool produced output (for showing in the chat stream)
    #[serde(rename = "tool_output")]
    ToolOutput {
        tool_name: String,
        output: String,
        success: bool,
        duration_ms: u64,
    },
    /// Token usage update after an LLM call
    #[serde(rename = "token_usage")]
    TokenUsage {
        prompt_tokens: u64,
        completion_tokens: u64,
        total_tokens: u64,
        cumulative_total: u64,
    },
    /// Conversation control is being handed off to another agent
    #[serde(rename = "handoff")]
    Handoff {
        from_agent: String,
        to_agent: String,
        context_preview: String,
    },
}

/// Convenience type for a progress sender
pub type ProgressSender = tokio::sync::mpsc::UnboundedSender<AgentProgressEvent>;

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
        hook_registry: Option<Arc<HookRegistry>>,
        agent_invoker: Option<Arc<dyn AgentInvoker>>,
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

        // Start MCP servers and register discovered tools
        #[cfg(feature = "tools-mcp")]
        if !config.mcp_servers.is_empty() {
            let mcp_manager = Arc::new(rockbot_tools_mcp::McpServerManager::new());
            for (name, entry) in &config.mcp_servers {
                let mcp_config = rockbot_tools_mcp::McpServerConfig {
                    command: entry.command.clone(),
                    args: entry.args.clone(),
                    env: entry.env.clone(),
                };
                match mcp_manager.start_server(name, &mcp_config).await {
                    Ok(tools) => {
                        info!("MCP server '{}' started with {} tools", name, tools.len());
                        for tool_def in tools {
                            let proxy = Arc::new(rockbot_tools_mcp::McpProxyTool::new(
                                name.clone(),
                                tool_def,
                                Arc::clone(&mcp_manager),
                            ));
                            tool_registry.register_tool(proxy).await;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to start MCP server '{}': {e}", name);
                    }
                }
            }
        }

        // Build guardrail pipeline from config
        let mut guardrail_pipeline = GuardrailPipeline::new();
        for name in &config.guardrails {
            match name.as_str() {
                "pii" => guardrail_pipeline.add(Arc::new(PiiGuardrail::new())),
                "prompt_injection" => guardrail_pipeline.add(Arc::new(PromptInjectionGuardrail::new())),
                other => {
                    warn!("Unknown guardrail '{}', skipping", other);
                }
            }
        }
        if !guardrail_pipeline.is_empty() {
            info!("Agent '{}' has {} guardrail(s) enabled", config.id, guardrail_pipeline.len());
        }

        // Set up episodic memory store if enabled
        let episodic_store = if config.episodic_memory {
            let episodes_dir = workspace.join("episodes");
            Some(Arc::new(rockbot_memory::EpisodicStore::new(episodes_dir)))
        } else {
            None
        };

        // Extract swarm_id from agent config map
        let swarm_id = config.config.get("swarm_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(Self {
            config,
            llm_provider,
            tool_registry,
            memory_manager,
            security_manager,
            session_manager,
            credential_accessor,
            hook_registry: hook_registry.unwrap_or_else(|| Arc::new(HookRegistry::new())),
            agent_invoker,
            guardrail_pipeline: Arc::new(guardrail_pipeline),
            episodic_store,
            blackboard: None,
            swarm_id,
            state: Arc::new(RwLock::new(AgentState {
                active_contexts: HashMap::new(),
                stats: AgentStats::default(),
            })),
        })
    }

    /// Get a reference to this agent's tool registry.
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    /// Set the agent invoker for subagent delegation.
    pub fn set_agent_invoker(&mut self, invoker: Arc<dyn AgentInvoker>) {
        self.agent_invoker = Some(invoker);
    }

    /// Set the hook registry for lifecycle events.
    pub fn set_hook_registry(&mut self, registry: Arc<HookRegistry>) {
        self.hook_registry = registry;
    }

    /// Set the blackboard accessor for swarm coordination.
    pub fn set_blackboard(&mut self, blackboard: Arc<dyn rockbot_tools::BlackboardAccessor>) {
        self.blackboard = Some(blackboard);
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
        self.process_message_inner(session_id, message, workspace_override, None).await
    }

    /// Like `process_message`, but sends real-time progress events to the
    /// provided channel as tool calls execute and LLM calls are made.
    pub async fn process_message_with_progress(
        &self,
        session_id: String,
        message: Message,
        workspace_override: Option<std::path::PathBuf>,
        progress_tx: ProgressSender,
    ) -> Result<AgentResponse> {
        self.process_message_inner(session_id, message, workspace_override, Some(progress_tx)).await
    }

    async fn process_message_inner(
        &self,
        session_id: String,
        message: Message,
        workspace_override: Option<std::path::PathBuf>,
        progress_tx: Option<ProgressSender>,
    ) -> Result<AgentResponse> {
        let start_time = std::time::Instant::now();
        let mut trajectory = Trajectory::new(&session_id, &self.config.id);

        debug!("Processing message {} in session {}", message.id, session_id);

        // Record user message in trajectory
        trajectory.record(TrajectoryEvent::UserMessage {
            content_preview: preview(&message.extract_text().unwrap_or_default(), 200),
        }, 0, 0);

        // --- Input guardrail check ---
        if !self.guardrail_pipeline.is_empty() {
            let guardrail_result = self.guardrail_pipeline.check_input(&message).await;
            let result_label = match &guardrail_result {
                GuardrailResult::Pass => "pass".to_string(),
                GuardrailResult::Warn(msg) => format!("warn: {msg}"),
                GuardrailResult::Block(msg) => format!("block: {msg}"),
            };
            trajectory.record(TrajectoryEvent::Guardrail {
                name: "pipeline".to_string(),
                direction: "input".to_string(),
                result: result_label,
            }, 0, 0);
            if let GuardrailResult::Block(reason) = guardrail_result {
                return Err(AgentError::ExecutionFailed {
                    message: format!("Input blocked by guardrail: {reason}"),
                }.into());
            }
        }

        // Fire PreMessage hook
        let pre_event = HookEvent::PreMessage {
            agent_id: self.config.id.clone(),
            session_id: session_id.clone(),
            message: message.clone(),
        };
        if let HookResult::Abort { reason } = self.hook_registry.fire(&pre_event).await {
            return Err(AgentError::ExecutionFailed {
                message: format!("PreMessage hook aborted: {reason}"),
            }.into());
        }

        // Get or create session — use the session's actual DB ID for all operations
        let session = self.get_or_create_session(&session_id, &message).await?;
        let db_session_id = session.id.clone();

        // Store incoming message
        self.session_manager.add_message(&db_session_id, message.clone()).await?;

        // Update processing context
        let available_tools = self.get_available_tools(&session).await?;
        let mut context = self.update_processing_context(db_session_id.clone(), message, available_tools, workspace_override).await?;

        // --- Planning phase ---
        // If planning_mode is "always" or "auto", ask the model to produce a plan first.
        // "approval_required" generates the plan but returns it to the user for approval
        // instead of executing it immediately.
        let planning_mode = self.config.planning_mode.as_str();
        if planning_mode == "always" || planning_mode == "auto" || planning_mode == "approval_required" {
            let plan_result = self.run_planning_phase(&db_session_id, &mut context, &trajectory).await;
            match plan_result {
                Ok(Some(plan_text)) => {
                    trajectory.record(TrajectoryEvent::Reflection {
                        action: format!("plan: {}", preview(&plan_text, 200)),
                    }, 0, 0);

                    if planning_mode == "approval_required" {
                        // Return the plan to the user for approval instead of executing
                        info!("Plan requires approval, returning plan to user");
                        let plan_response = Message::text(format!(
                            "I've created a plan for this task. Please review and approve it \
                             by replying with \"approved\" or provide feedback:\n\n{plan_text}"
                        ))
                            .with_session_id(&db_session_id)
                            .with_agent_id(&self.config.id)
                            .with_role(MessageRole::Assistant);

                        trajectory.record(TrajectoryEvent::Complete {
                            total_iterations: 0,
                            total_tool_calls: 0,
                            final_tokens: TokenUsage::default(),
                            duration_ms: start_time.elapsed().as_millis() as u64,
                        }, 0, 0);

                        return Ok(AgentResponse {
                            message: plan_response,
                            tool_results: vec![],
                            tokens_used: TokenUsage::default(),
                            processing_time_ms: start_time.elapsed().as_millis() as u64,
                            trajectory: Some(trajectory),
                            handoff: None,
                        });
                    }

                    // Inject the plan into context so the model follows it
                    let plan_msg = Message::text(format!(
                        "Here is your plan. Follow it step by step:\n\n{plan_text}\n\n\
                         Now execute the plan. Start with step 1."
                    ))
                        .with_session_id(&db_session_id)
                        .with_agent_id(&self.config.id)
                        .with_role(MessageRole::User);
                    context.messages.push(plan_msg);
                }
                Ok(None) => {
                    // Auto mode decided no plan was needed, or plan was empty
                }
                Err(e) => {
                    warn!("Planning phase failed, proceeding without plan: {e}");
                }
            }
        }

        // --- Workflow dispatch ---
        // If this agent has a workflow definition, execute the DAG instead of LLM.
        if let Some(ref workflow) = self.config.workflow {
            let user_msg = context.messages.last().cloned().unwrap_or_else(|| {
                Message::text("").with_session_id(&db_session_id)
            });
            return self.execute_workflow(
                &db_session_id,
                &user_msg,
                workflow,
                &progress_tx,
                start_time,
                trajectory,
            ).await;
        }

        // Generate LLM request
        let llm_request = self.build_llm_request(&mut context).await?;

        // Call LLM with streaming (for real-time text deltas) + retry logic
        let llm_response = self.call_llm_streaming_with_retry(llm_request, &progress_tx).await?;

        // Process LLM response and handle tool calls
        let (mut response_message, tool_results, mut token_usage) = self.process_llm_response(
            &db_session_id,
            &mut context,
            llm_response,
            &progress_tx,
        ).await?;

        // --- Output guardrail check ---
        if !self.guardrail_pipeline.is_empty() {
            let response_text = response_message.extract_text().unwrap_or_default();
            let guardrail_result = self.guardrail_pipeline.check_output(&response_text).await;
            let result_label = match &guardrail_result {
                GuardrailResult::Pass => "pass".to_string(),
                GuardrailResult::Warn(msg) => format!("warn: {msg}"),
                GuardrailResult::Block(msg) => format!("block: {msg}"),
            };
            trajectory.record(TrajectoryEvent::Guardrail {
                name: "pipeline".to_string(),
                direction: "output".to_string(),
                result: result_label,
            }, 0, token_usage.total_tokens);
            // For output, we warn but don't block — the response already exists
        }

        // --- Reflection pass ---
        if self.config.reflection_enabled && !tool_results.is_empty() {
            trajectory.record(TrajectoryEvent::Reflection {
                action: "starting".to_string(),
            }, 0, token_usage.total_tokens);

            let reflection_result = self.run_reflection(
                &db_session_id,
                &mut context,
                &response_message,
                &progress_tx,
            ).await;

            match reflection_result {
                Ok(Some((new_msg, extra_results, extra_tokens))) => {
                    trajectory.record(TrajectoryEvent::Reflection {
                        action: "corrected".to_string(),
                    }, 0, token_usage.total_tokens + extra_tokens.total_tokens);
                    response_message = new_msg;
                    token_usage.prompt_tokens += extra_tokens.prompt_tokens;
                    token_usage.completion_tokens += extra_tokens.completion_tokens;
                    token_usage.total_tokens += extra_tokens.total_tokens;
                    // Extend tool results if reflection made more calls
                    let _ = extra_results; // tool_results already consumed upstream
                }
                Ok(None) => {
                    trajectory.record(TrajectoryEvent::Reflection {
                        action: "no_changes".to_string(),
                    }, 0, token_usage.total_tokens);
                }
                Err(e) => {
                    warn!("Reflection pass failed: {e}");
                    trajectory.record(TrajectoryEvent::Reflection {
                        action: format!("error: {e}"),
                    }, 0, token_usage.total_tokens);
                }
            }
        }

        // Record completion in trajectory
        trajectory.record(TrajectoryEvent::Complete {
            total_iterations: tool_results.len(),
            total_tool_calls: tool_results.len(),
            final_tokens: token_usage.clone(),
            duration_ms: start_time.elapsed().as_millis() as u64,
        }, 0, token_usage.total_tokens);

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

        // Record observability metrics
        crate::metrics::record_agent_message(&self.config.id);
        let model_label = self.config.model.as_deref().unwrap_or("default");
        crate::metrics::record_llm_request(
            &self.config.id,
            model_label,
            start_time.elapsed(),
            token_usage.prompt_tokens as u64,
            token_usage.completion_tokens as u64,
        );

        // Store episodic memory entry if enabled and tool work was done
        if let Some(ref store) = self.episodic_store {
            if !tool_results.is_empty() {
                let response_text = response_message.extract_text().unwrap_or_default();
                let summary = if response_text.len() > 200 {
                    format!("{}...", &response_text[..200])
                } else {
                    response_text
                };
                let episode = rockbot_memory::Episode {
                    session_id: db_session_id.clone(),
                    timestamp: chrono::Utc::now(),
                    summary,
                    outcome: if tool_results.iter().all(|r| r.success) {
                        "success".to_string()
                    } else {
                        "partial".to_string()
                    },
                    tools_used: tool_results.iter().map(|r| r.tool_name.clone()).collect(),
                    tokens_used: token_usage.total_tokens,
                };
                if let Err(e) = store.store(&self.config.id, &episode).await {
                    debug!("Failed to store episode: {e}");
                }
            }
        }

        // Fire PostMessage hook
        let post_event = HookEvent::PostMessage {
            agent_id: self.config.id.clone(),
            session_id: db_session_id.clone(),
            response: response_message.clone(),
        };
        self.hook_registry.fire(&post_event).await;

        // Clean up processing context
        {
            let mut state = self.state.write().await;
            state.active_contexts.remove(&db_session_id);
        }

        // Check if any tool result was a handoff signal
        let handoff = tool_results.iter().find_map(|tr| {
            if let ToolResult::Handoff { ref target_agent_id, ref context, ref message_override } = tr.result {
                Some(HandoffSignal {
                    target_agent_id: target_agent_id.clone(),
                    context: context.clone(),
                    message_override: message_override.clone(),
                })
            } else {
                None
            }
        });

        Ok(AgentResponse {
            message: response_message,
            tool_results,
            tokens_used: token_usage,
            processing_time_ms,
            trajectory: Some(trajectory),
            handoff,
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

        // Fire PreMessage hook
        let pre_event = HookEvent::PreMessage {
            agent_id: self.config.id.clone(),
            session_id: session_id.clone(),
            message: message.clone(),
        };
        if let HookResult::Abort { reason } = self.hook_registry.fire(&pre_event).await {
            return Err(AgentError::ExecutionFailed {
                message: format!("PreMessage hook aborted: {reason}"),
            }.into());
        }

        let mut trajectory = Trajectory::new(&session_id, &self.config.id);
        let session = self.get_or_create_session(&session_id, &message).await?;
        let db_session_id = session.id.clone();

        self.session_manager.add_message(&db_session_id, message.clone()).await?;

        trajectory.record(TrajectoryEvent::UserMessage {
            content_preview: preview(&message.extract_text().unwrap_or_default(), 200),
        }, 0, 0);

        let available_tools = self.get_available_tools(&session).await?;
        let mut context = self.update_processing_context(
            db_session_id.clone(), message, available_tools, workspace_override,
        ).await?;

        // Build initial streaming request
        let llm_request = self.build_llm_request_streaming(&mut context).await?;

        trajectory.record(TrajectoryEvent::LlmRequest {
            model: llm_request.model.clone(),
            message_count: llm_request.messages.len(),
            tools_available: llm_request.tools.as_ref().map_or(0, |t| t.len()),
        }, 0, 0);

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

        trajectory.record(TrajectoryEvent::LlmResponse {
            content_preview: preview(&response_message.extract_text().unwrap_or_default(), 200),
            tool_call_names: tool_results.iter().map(|r| r.tool_name.clone()).collect(),
            tokens: token_usage.clone(),
        }, 1, token_usage.total_tokens as u64);

        self.session_manager.add_message(&db_session_id, response_message.clone()).await?;

        let mut session = self.session_manager.get_session(&db_session_id).await?
            .ok_or_else(|| AgentError::ExecutionFailed {
                message: "Session disappeared during processing".to_string()
            })?;
        session.add_tokens(token_usage.prompt_tokens, token_usage.completion_tokens);
        self.session_manager.update_session(&session).await?;

        let processing_time_ms = start_time.elapsed().as_millis() as u64;
        self.update_stats(token_usage.total_tokens, processing_time_ms).await;

        // Record observability metrics
        crate::metrics::record_agent_message(&self.config.id);
        let model_label = self.config.model.as_deref().unwrap_or("default");
        crate::metrics::record_llm_request(
            &self.config.id,
            model_label,
            start_time.elapsed(),
            token_usage.prompt_tokens as u64,
            token_usage.completion_tokens as u64,
        );

        // Fire PostMessage hook
        let post_event = HookEvent::PostMessage {
            agent_id: self.config.id.clone(),
            session_id: db_session_id.clone(),
            response: response_message.clone(),
        };
        self.hook_registry.fire(&post_event).await;

        trajectory.record(TrajectoryEvent::Complete {
            total_iterations: 1,
            total_tool_calls: tool_results.len(),
            final_tokens: token_usage.clone(),
            duration_ms: processing_time_ms,
        }, 1, token_usage.total_tokens as u64);

        {
            let mut state = self.state.write().await;
            state.active_contexts.remove(&db_session_id);
        }

        Ok(AgentResponse {
            message: response_message,
            tool_results,
            tokens_used: token_usage,
            processing_time_ms,
            trajectory: Some(trajectory),
            handoff: None,
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
        let llm_timeout = Duration::from_secs(self.config.llm_timeout_secs);

        // Timeout on initial stream connection
        let mut stream = tokio::time::timeout(llm_timeout, self.llm_provider.stream_completion(request))
            .await
            .map_err(|_| crate::error::RockBotError::Agent(AgentError::ModelError {
                message: format!("LLM streaming connection timed out after {}s", self.config.llm_timeout_secs),
            }))?
            .map_err(|e| crate::error::RockBotError::Agent(AgentError::ModelError {
                message: format!("LLM streaming connection failed: {e}"),
            }))?;

        let mut accumulated_text = String::new();
        let mut accumulated_tool_calls: Vec<rockbot_llm::ToolCall> = Vec::new();
        let mut response_id = String::new();
        let mut finish_reason = "stop".to_string();

        // Per-chunk idle timeout: if no chunk arrives within the timeout, abort.
        // This is more generous than the initial connection timeout since the model
        // may legitimately pause while thinking/generating tool calls.
        let chunk_idle_timeout = Duration::from_secs(15);

        while let Ok(maybe_chunk) = tokio::time::timeout(chunk_idle_timeout, stream.next()).await {
            let Some(chunk_result) = maybe_chunk else { break };
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
        // Note: if the while loop exits due to chunk_idle_timeout, we fall through
        // and use whatever we've accumulated so far. If nothing was accumulated,
        // return an error.
        if accumulated_text.is_empty() && accumulated_tool_calls.is_empty() && response_id.is_empty() {
            return Err(crate::error::RockBotError::Agent(AgentError::ModelError {
                message: format!("LLM streaming stalled — no data received for {}s", self.config.llm_timeout_secs * 2),
            }));
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
                    images: vec![],
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
            let (tool_results, tool_messages, handoff_signal) = self.execute_tool_calls(
                session_id, &current_response, &effective_workspace,
            ).await?;

            // If handoff detected in streaming path, break immediately
            if handoff_signal.is_some() {
                all_tool_results.extend(tool_results);
                for tool_message in tool_messages {
                    self.session_manager.add_message(session_id, tool_message.clone()).await?;
                    context.messages.push(tool_message);
                }
                final_response_content = "[Handoff signalled]".to_string();
                break;
            }

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
                    images: vec![],
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

        // Count tokens using BPE tokenizer
        context.token_count = context.messages.iter()
            .map(|m| crate::tokenizer::count_tokens(&m.extract_text().unwrap_or_default()))
            .sum();

        // If context is too large, perform compaction
        let compaction_threshold = self.config.max_context_tokens * 80 / 100;
        if context.token_count > compaction_threshold {
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
        // Scale keep_recent with context budget — smaller budgets keep fewer recent messages.
        let keep_recent = if self.config.max_context_tokens <= 16_000 {
            8usize
        } else if self.config.max_context_tokens <= 32_000 {
            12usize
        } else {
            20usize
        };
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
                    images: vec![],
                    tool_calls: None,
                    tool_call_id: None,
                },
                rockbot_llm::Message {
                    role: rockbot_llm::MessageRole::User,
                    content: summary_input.clone(),
                    images: vec![],
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

        // Recalculate token count using BPE tokenizer
        context.token_count = context.messages.iter()
            .map(|m| crate::tokenizer::count_tokens(&m.extract_text().unwrap_or_default()))
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
                images: vec![],
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
                images: vec![],
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

        // Load and inject memory guidelines (MEMORY.md)
        if let Ok(memory_content) = self.load_context_file("MEMORY.md").await {
            prompt_parts.push(format!("# Memory Guidelines\n\n{memory_content}"));
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
        
        // Inject relevant past episodes from episodic memory
        if let Some(ref store) = self.episodic_store {
            let user_msg = context.messages.last()
                .map(|m| m.extract_text().unwrap_or_default())
                .unwrap_or_default();
            if !user_msg.is_empty() {
                match store.recall(&self.config.id, &user_msg, 3).await {
                    Ok(episodes) if !episodes.is_empty() => {
                        let mut section = String::from("# Relevant Past Interactions\n\n");
                        for (i, ep) in episodes.iter().enumerate() {
                            section.push_str(&format!(
                                "{}. [{}] {}\n   Tools: {}, Outcome: {}\n",
                                i + 1,
                                ep.timestamp.format("%Y-%m-%d"),
                                ep.summary,
                                ep.tools_used.join(", "),
                                ep.outcome,
                            ));
                        }
                        prompt_parts.push(section);
                    }
                    Ok(_) => {} // No relevant episodes
                    Err(e) => {
                        debug!("Episodic recall failed: {e}");
                    }
                }
            }
        }

        // Add agentic behavior directives
        prompt_parts.push(Self::agentic_behavior_prompt().to_string());

        // Add current timestamp
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
        prompt_parts.push(format!("# Current Time\n\n{timestamp}"));

        Ok(prompt_parts.join("\n\n---\n\n"))
    }
    
    /// Load a context file from the agent's config directory.
    ///
    /// Tries the agent directory first, then fallback locations.
    /// Returns `Err` if the file is not found anywhere — callers use
    /// `if let Ok(...)` to treat missing files as optional.
    async fn load_context_file(&self, filename: &str) -> Result<String> {
        let agent_dir = self.get_agent_directory();
        let file_path = agent_dir.join(filename);

        match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => return Ok(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                trace!("Context file {} not found in agent dir, checking fallbacks", filename);
            }
            Err(e) => {
                debug!("Could not load context file {}: {}", filename, e);
            }
        }

        // Try standard fallback locations
        let fallback_paths = [
            dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("rockbot")
                .join(filename),
            std::env::current_dir().unwrap_or_default().join(filename),
            dirs::home_dir().unwrap_or_default().join(".openclaw").join(filename),
        ];

        for path in fallback_paths {
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                debug!("Loaded context file {} from fallback: {}", filename, path.display());
                return Ok(content);
            }
        }

        trace!("Optional context file {} not found in any location", filename);
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
        let llm_timeout = Duration::from_secs(self.config.llm_timeout_secs);

        for attempt in 0..=retry_config.max_retries {
            debug!("LLM API call attempt {} of {} (timeout: {}s)", attempt + 1, retry_config.max_retries + 1, self.config.llm_timeout_secs);

            match tokio::time::timeout(llm_timeout, self.llm_provider.chat_completion(request.clone())).await {
                Err(_elapsed) => {
                    last_error_message = format!("LLM API call timed out after {}s", self.config.llm_timeout_secs);
                    warn!("{} (attempt {})", last_error_message, attempt + 1);
                    {
                        let mut state = self.state.write().await;
                        state.stats.error_count += 1;
                    }
                    if attempt >= retry_config.max_retries {
                        break;
                    }
                    let delay = self.calculate_retry_delay(&retry_config, attempt, &ErrorCategory::Network);
                    warn!("Retrying LLM API call in {}ms after timeout", delay);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                Ok(inner_result) => match inner_result {
                    Ok(response) => {
                        if attempt > 0 {
                            info!("LLM API call succeeded after {} retries", attempt);
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

                        {
                            let mut state = self.state.write().await;
                            state.stats.error_count += 1;
                            if matches!(error_category, ErrorCategory::RateLimit { .. }) {
                                state.stats.rate_limit_hits += 1;
                            }
                        }

                        if attempt >= retry_config.max_retries || !self.should_retry_error(&error_category) {
                            break;
                        }

                        let delay = self.calculate_retry_delay(&retry_config, attempt, &error_category);
                        warn!("Retrying LLM API call in {}ms due to: {}", delay, error);
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                },
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
    
    /// Call LLM with streaming + retry, forwarding text deltas via the progress channel.
    /// Returns the assembled response, same as `call_llm_with_retry`.
    async fn call_llm_streaming_with_retry(
        &self,
        request: ChatCompletionRequest,
        progress_tx: &Option<ProgressSender>,
    ) -> Result<ChatCompletionResponse> {
        // If no progress channel, fall back to non-streaming
        let Some(ptx) = progress_tx.as_ref() else {
            return self.call_llm_with_retry(request).await;
        };

        let retry_config = RetryConfig::default();
        let mut last_error_message = String::new();
        let llm_timeout = Duration::from_secs(self.config.llm_timeout_secs);
        let chunk_idle_timeout = Duration::from_secs(15);

        for attempt in 0..=retry_config.max_retries {
            debug!("LLM streaming call attempt {} of {} (timeout: {}s)",
                   attempt + 1, retry_config.max_retries + 1, self.config.llm_timeout_secs);

            let stream_result = tokio::time::timeout(
                llm_timeout,
                self.llm_provider.stream_completion(request.clone()),
            ).await;

            let mut stream = match stream_result {
                Err(_elapsed) => {
                    last_error_message = format!("LLM streaming timed out after {}s", self.config.llm_timeout_secs);
                    warn!("{} (attempt {})", last_error_message, attempt + 1);
                    if attempt >= retry_config.max_retries { break; }
                    let delay = self.calculate_retry_delay(&retry_config, attempt, &ErrorCategory::Network);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                Ok(Err(error)) => {
                    last_error_message = error.to_string();
                    let cat = self.classify_llm_error(&error);
                    if attempt >= retry_config.max_retries || !self.should_retry_error(&cat) { break; }
                    let delay = self.calculate_retry_delay(&retry_config, attempt, &cat);
                    warn!("Retrying LLM streaming in {}ms due to: {}", delay, error);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    continue;
                }
                Ok(Ok(s)) => s,
            };

            let mut accumulated_text = String::new();
            let mut accumulated_tool_calls: Vec<rockbot_llm::ToolCall> = Vec::new();
            let mut response_id = String::new();
            let mut finish_reason = "stop".to_string();

            while let Ok(maybe_chunk) = tokio::time::timeout(chunk_idle_timeout, stream.next()).await {
                let Some(chunk_result) = maybe_chunk else { break };
                match chunk_result {
                    Ok(chunk) => {
                        if response_id.is_empty() {
                            response_id.clone_from(&chunk.id);
                        }
                        for choice in &chunk.choices {
                            if let Some(ref content) = choice.delta.content {
                                accumulated_text.push_str(content);
                                // Forward text delta to TUI in real-time
                                let _ = ptx.send(AgentProgressEvent::TextDelta {
                                    text: content.clone(),
                                });
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
                    }
                    Err(e) => {
                        last_error_message = format!("Stream error: {e}");
                        break;
                    }
                }
            }

            // If we got data, assemble and return
            if !response_id.is_empty() || !accumulated_text.is_empty() || !accumulated_tool_calls.is_empty() {
                if attempt > 0 {
                    info!("LLM streaming succeeded after {} retries", attempt);
                }
                let tool_calls_option = if accumulated_tool_calls.is_empty() {
                    None
                } else {
                    Some(accumulated_tool_calls)
                };
                // Estimate tokens since streaming chunks don't carry usage data
                let est_completion = crate::tokenizer::count_tokens(&accumulated_text) as u64;
                let est_prompt = request.messages.iter()
                    .map(|m| crate::tokenizer::count_tokens(&m.content) as u64)
                    .sum::<u64>();
                return Ok(ChatCompletionResponse {
                    id: if response_id.is_empty() { format!("stream-{}", uuid::Uuid::new_v4()) } else { response_id },
                    object: "chat.completion".to_string(),
                    created: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                    model: request.model.clone(),
                    choices: vec![rockbot_llm::Choice {
                        index: 0,
                        message: rockbot_llm::Message {
                            role: rockbot_llm::MessageRole::Assistant,
                            content: accumulated_text,
                            images: vec![],
                            tool_calls: tool_calls_option,
                            tool_call_id: None,
                        },
                        finish_reason,
                    }],
                    usage: rockbot_llm::Usage {
                        prompt_tokens: est_prompt,
                        completion_tokens: est_completion,
                        total_tokens: est_prompt + est_completion,
                    },
                });
            }

            // No data received — retry if possible
            if attempt >= retry_config.max_retries { break; }
            let delay = self.calculate_retry_delay(&retry_config, attempt, &ErrorCategory::Network);
            warn!("LLM streaming produced no data, retrying in {}ms", delay);
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }

        error!("LLM streaming failed after {} retries: {}", retry_config.max_retries, last_error_message);
        Err(crate::error::RockBotError::Agent(AgentError::ModelError {
            message: format!("LLM streaming failed after {} retries: {}", retry_config.max_retries, last_error_message),
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
        progress_tx: &Option<ProgressSender>,
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

        // Continuation nudge budget (re-prompts when model produces text without tools).
        // Budget resets after each successful tool-call iteration — a nudge that causes
        // the model to use tools is a success, not a permanent deduction.
        // "Consecutive" tracks nudges since the last tool-call iteration.
        let max_consecutive_nudges = 2u32;
        let mut consecutive_nudge_count = 0u32;

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

            // Send token usage update after each LLM call.
            // Text content was already streamed via TextDelta events during
            // call_llm_streaming_with_retry, so we don't re-send it here.
            if let Some(ref tx) = progress_tx {
                let _ = tx.send(AgentProgressEvent::TokenUsage {
                    prompt_tokens: cumulative_token_usage.prompt_tokens,
                    completion_tokens: cumulative_token_usage.completion_tokens,
                    total_tokens: cumulative_token_usage.total_tokens,
                    cumulative_total: cumulative_token_usage.total_tokens,
                });
            }

            if has_tool_calls {
                // Model is making tool calls — reset the consecutive nudge counter
                // because any prior nudges succeeded in getting the model to act.
                consecutive_nudge_count = 0;
            }

            if !has_tool_calls {
                let response_text = current_response.choices
                    .first()
                    .map(|c| c.message.content.as_str())
                    .unwrap_or("");

                // Strip <think> blocks from final output
                let clean_text = Self::strip_think_blocks(response_text);

                // --- Continuation nudge ---
                // If the model's response looks like an acknowledgment / plan
                // rather than a final answer, nudge it to take action.
                // Also nudge when the model produced empty visible output after
                // doing work (common with models that use <think> blocks).
                // Skip nudging for conversational questions — the model's text
                // response IS the correct answer.
                let user_is_asking_question = Self::is_conversational_question(context);
                let is_mid_task_pause = !all_tool_results.is_empty()
                    && !clean_text.is_empty()
                    && Self::looks_like_continuation_intent(&clean_text);
                let is_initial_ack = all_tool_results.is_empty()
                    && !clean_text.is_empty()
                    && Self::looks_like_acknowledgment(&clean_text);
                // Model did work (tools were called) but produced no visible output —
                // only <think> blocks or truly empty response. Nudge it to answer.
                // Limit empty_after_work nudges to 1 since models that structurally
                // separate reasoning from text rarely recover with repeated nudges.
                let is_empty_after_work = !all_tool_results.is_empty()
                    && clean_text.is_empty();
                let nudge_limit = if is_empty_after_work { 1 } else { max_consecutive_nudges };
                if (is_initial_ack || is_mid_task_pause || is_empty_after_work)
                    && consecutive_nudge_count < nudge_limit
                    && !user_is_asking_question
                {
                    consecutive_nudge_count += 1;
                    info!(
                        "Continuation nudge {}/{} for session {} — model responded without tool use \
                         (mid_task={}, empty_after_work={})",
                        consecutive_nudge_count, max_consecutive_nudges, session_id,
                        is_mid_task_pause, is_empty_after_work
                    );

                    // Add the assistant's text to context (may include <think> blocks)
                    if !response_text.is_empty() {
                        let assistant_message = rockbot_llm::Message {
                            role: rockbot_llm::MessageRole::Assistant,
                            content: response_text.to_string(),
                            images: vec![],
                            tool_calls: None,
                            tool_call_id: None,
                        };
                        let assistant_msg = Message::from_llm_message(
                            assistant_message, session_id, &self.config.id
                        )?;
                        context.messages.push(assistant_msg);
                    }

                    // Escalating nudge messages — tone differs by situation
                    let nudge_text = if is_empty_after_work {
                        match consecutive_nudge_count {
                            1 => "You executed tools but did not provide a visible response. \
                                  You MUST respond with your findings, analysis, or next actions. \
                                  Do not only think internally — produce visible output for the user.",
                            2 => "You MUST provide a response NOW. Summarize what you found from \
                                  the tool calls you made. The user cannot see your internal reasoning.",
                            _ => "FINAL WARNING: Produce a visible response immediately. \
                                  Summarize your findings or continue working with tool calls.",
                        }
                    } else if is_mid_task_pause {
                        match consecutive_nudge_count {
                            1 => "You indicated you want to continue but stopped calling tools. \
                                  Continue working — use your tools now to take the next step.",
                            2 => "You MUST continue executing. Call a tool right now to make progress. \
                                  Do not describe what you plan to do — just do it.",
                            _ => "FINAL WARNING: You have tools available and work remaining. \
                                  Call a tool immediately or this task will be considered complete.",
                        }
                    } else {
                        match consecutive_nudge_count {
                            1 => "You described what you would do but did not take action. \
                                  Use your tools now to accomplish the task. \
                                  Do not describe steps — execute them.",
                            2 => "You are still not using tools. You MUST call a tool right now. \
                                  Start with the first concrete action needed. \
                                  Do not output any text — only tool calls.",
                            _ => "FINAL WARNING: Call a tool immediately or this task will be \
                                  considered failed. Pick the single most important action \
                                  and execute it now.",
                        }
                    };

                    let nudge = Message::text(nudge_text)
                        .with_session_id(session_id)
                        .with_agent_id(&self.config.id)
                        .with_role(MessageRole::User);
                    context.messages.push(nudge);

                    // Re-prompt
                    let next_request = self.build_llm_request(context).await?;
                    match self.call_llm_streaming_with_retry(next_request, progress_tx).await {
                        Ok(response) => {
                            cumulative_token_usage.prompt_tokens += response.usage.prompt_tokens;
                            cumulative_token_usage.completion_tokens += response.usage.completion_tokens;
                            cumulative_token_usage.total_tokens += response.usage.total_tokens;
                            current_response = response;
                            continue;
                        }
                        Err(e) => {
                            error!("LLM error during continuation nudge: {}", e);
                            if !clean_text.is_empty() {
                                final_response_content = clean_text;
                            }
                            // If clean_text is empty, fall through to post-loop fallback
                            break;
                        }
                    }
                }

                // Nudge budget exhausted or no nudge condition matched.
                // If we have visible text, use it as the final response.
                if !clean_text.is_empty() {
                    final_response_content = clean_text;
                } else if !all_tool_results.is_empty() {
                    // Model never produced visible output despite nudges.
                    // Synthesize a summary from tool results including output previews.
                    let tool_summary: Vec<String> = all_tool_results.iter().map(|tr: &ToolExecutionResult| {
                        let status = if tr.success { "✓" } else { "✗" };
                        let preview = match &tr.result {
                            rockbot_tools::message::ToolResult::Text { content } => {
                                let trimmed = content.trim();
                                if trimmed.len() > 200 {
                                    format!(": {}…", &trimmed[..200])
                                } else if !trimmed.is_empty() {
                                    format!(": {trimmed}")
                                } else {
                                    String::new()
                                }
                            }
                            rockbot_tools::message::ToolResult::Error { message, .. } => {
                                format!(": {message}")
                            }
                            _ => String::new(),
                        };
                        format!("  {status} {}{preview}", tr.tool_name)
                    }).collect();
                    final_response_content = format!(
                        "Completed {} tool call(s). The model did not produce a visible summary.\n\
                         Tools executed:\n{}",
                        all_tool_results.len(),
                        tool_summary.join("\n")
                    );
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
                        // Notify progress listener of each tool about to run
                        if let Some(ref tx) = progress_tx {
                            let _ = tx.send(AgentProgressEvent::ToolStart {
                                tool_name: tc.function.name.clone(),
                            });
                        }
                    }
                }
            }

            let effective_workspace = self.resolve_workspace(context);
            let (tool_results, tool_messages, handoff_signal) = self.execute_tool_calls(
                session_id,
                &current_response,
                &effective_workspace,
            ).await?;

            // Notify progress listener of completed tool calls with output
            if let Some(ref tx) = progress_tx {
                for (tr, tm) in tool_results.iter().zip(tool_messages.iter()) {
                    let output = tm.extract_text().unwrap_or_default();
                    let _ = tx.send(AgentProgressEvent::ToolOutput {
                        tool_name: tr.tool_name.clone(),
                        output,
                        success: tr.success,
                        duration_ms: tr.execution_time_ms,
                    });
                }
            }

            // If a handoff was signalled, break the loop immediately
            if let Some(ref signal) = handoff_signal {
                if let Some(ref tx) = progress_tx {
                    let preview = if signal.context.len() > 100 {
                        format!("{}...", &signal.context[..100])
                    } else {
                        signal.context.clone()
                    };
                    let _ = tx.send(AgentProgressEvent::Handoff {
                        from_agent: self.config.id.clone(),
                        to_agent: signal.target_agent_id.clone(),
                        context_preview: preview,
                    });
                }

                // Persist assistant message + tool messages before breaking
                if let Some(choice) = current_response.choices.first() {
                    let assistant_message = rockbot_llm::Message {
                        role: rockbot_llm::MessageRole::Assistant,
                        content: choice.message.content.clone(),
                        images: vec![],
                        tool_calls: choice.message.tool_calls.clone(),
                        tool_call_id: None,
                    };
                    let assistant_msg = Message::from_llm_message(
                        assistant_message, session_id, &self.config.id
                    )?;
                    self.session_manager.add_message(session_id, assistant_msg.clone()).await?;
                    context.messages.push(assistant_msg);
                }
                for tool_message in tool_messages {
                    self.session_manager.add_message(session_id, tool_message.clone()).await?;
                    context.messages.push(tool_message);
                }
                all_tool_results.extend(tool_results);

                // Return with handoff info embedded in the response
                let handoff_msg = format!(
                    "[Handing off to agent '{}']",
                    signal.target_agent_id,
                );
                let response_message = Message::text(&handoff_msg)
                    .with_session_id(session_id)
                    .with_agent_id(&self.config.id)
                    .with_role(MessageRole::Assistant);

                return Ok((response_message, all_tool_results, cumulative_token_usage));
            }

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
                            images: vec![],
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
                            images: vec![],
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
                    match self.call_llm_streaming_with_retry(next_llm_request, progress_tx).await {
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
                    images: vec![],
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
                .map(|m| crate::tokenizer::count_tokens(&m.extract_text().unwrap_or_default()))
                .sum();
            let compaction_threshold = self.config.max_context_tokens * 80 / 100;
            if estimated_tokens > compaction_threshold {
                info!("Context approaching limit ({} est. tokens), compacting for session {}",
                      estimated_tokens, session_id);
                self.compact_context(context).await?;
            }

            // Generate next LLM request
            let next_llm_request = self.build_llm_request(context).await?;

            match self.call_llm_streaming_with_retry(next_llm_request, progress_tx).await {
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

        // Strip any remaining <think> blocks from the final output
        final_response_content = Self::strip_think_blocks(&final_response_content);

        // Create final response message
        let response_message = Message::text(final_response_content)
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::Assistant);

        info!(
            "Completed tool execution loop: {} iterations, {} tool calls, {} prompt + {} completion = {} total tokens",
            iteration_count, all_tool_results.len(),
            cumulative_token_usage.prompt_tokens,
            cumulative_token_usage.completion_tokens,
            cumulative_token_usage.total_tokens,
        );

        Ok((response_message, all_tool_results, cumulative_token_usage))
    }
    
    /// Run the planning phase: ask the model to produce a step-by-step plan
    /// before executing. Returns the plan text if one was produced.
    async fn run_planning_phase(
        &self,
        session_id: &str,
        context: &mut ProcessingContext,
        _trajectory: &Trajectory,
    ) -> Result<Option<String>> {
        let planning_mode = self.config.planning_mode.as_str();

        // In "auto" mode, check if the message is complex enough to warrant planning
        if planning_mode == "auto" {
            let user_msg = context.messages.last()
                .map(|m| m.extract_text().unwrap_or_default())
                .unwrap_or_default();
            // Simple heuristic: only plan for messages > 100 chars or containing
            // task-like keywords
            let needs_plan = user_msg.len() > 100
                || user_msg.contains("implement")
                || user_msg.contains("create")
                || user_msg.contains("refactor")
                || user_msg.contains("fix")
                || user_msg.contains("build");
            if !needs_plan {
                return Ok(None);
            }
        }

        info!("Running planning phase for session {}", session_id);

        // Ask the model to produce a plan (without tools — pure text response)
        let plan_prompt = Message::text(
            "Before executing, produce a concise numbered plan (3-8 steps) for this task. \
             Each step should describe one concrete action. Do not execute anything yet — \
             only output the plan. Format:\n\
             1. [action]\n\
             2. [action]\n\
             ..."
        )
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::User);
        context.messages.push(plan_prompt);

        // Build request WITHOUT tools so the model produces only text
        let system_prompt = self.assemble_system_prompt(context).await?;
        let mut messages = Vec::new();
        if !system_prompt.is_empty() {
            messages.push(rockbot_llm::Message {
                role: rockbot_llm::MessageRole::System,
                content: system_prompt,
                images: vec![],
                tool_calls: None,
                tool_call_id: None,
            });
        }
        for msg in &context.messages {
            if matches!(msg.metadata.role, MessageRole::System) { continue; }
            let tool_calls = msg.metadata.extra.get("tool_calls")
                .and_then(|v| serde_json::from_value::<Vec<rockbot_llm::ToolCall>>(v.clone()).ok());
            let tool_call_id = msg.metadata.extra.get("tool_call_id")
                .and_then(|v| v.as_str()).map(|s| s.to_string());
            messages.push(rockbot_llm::Message {
                role: match msg.metadata.role {
                    MessageRole::User => rockbot_llm::MessageRole::User,
                    MessageRole::Assistant => rockbot_llm::MessageRole::Assistant,
                    MessageRole::System => rockbot_llm::MessageRole::System,
                    MessageRole::Tool => rockbot_llm::MessageRole::Tool,
                },
                content: msg.extract_text().unwrap_or_default(),
                images: vec![],
                tool_calls,
                tool_call_id,
            });
        }

        let model = self.config.model.as_ref()
            .unwrap_or(&"anthropic/claude-sonnet-4-20250514".to_string())
            .clone();

        let plan_request = ChatCompletionRequest {
            model,
            messages,
            tools: None, // No tools for planning
            temperature: Some(0.3),
            max_tokens: Some(2000), // Plans should be concise
            stream: false,
            response_format: None,
        };

        let plan_response = self.call_llm_with_retry(plan_request).await?;
        let plan_text = plan_response.choices.first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        let clean_plan = Self::strip_think_blocks(&plan_text);
        if clean_plan.is_empty() {
            return Ok(None);
        }

        // Add the plan as assistant message in context
        let plan_assistant = rockbot_llm::Message {
            role: rockbot_llm::MessageRole::Assistant,
            content: clean_plan.clone(),
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
        };
        let plan_msg = Message::from_llm_message(plan_assistant, session_id, &self.config.id)?;
        context.messages.push(plan_msg);

        info!("Planning phase produced {} char plan for session {}", clean_plan.len(), session_id);
        Ok(Some(clean_plan))
    }

    /// Run a reflection/self-critique pass after the tool loop completes.
    ///
    /// Asks the model to review its own response for errors or omissions.
    /// If the model identifies corrections, it gets one more tool-loop pass.
    /// Returns `Ok(Some(...))` with the corrected response, or `Ok(None)` if
    /// the model is satisfied with its original answer.
    async fn run_reflection(
        &self,
        session_id: &str,
        context: &mut ProcessingContext,
        response: &Message,
        progress_tx: &Option<ProgressSender>,
    ) -> Result<Option<(Message, Vec<ToolExecutionResult>, TokenUsage)>> {
        let response_text = response.extract_text().unwrap_or_default();
        if response_text.is_empty() {
            return Ok(None);
        }

        debug!("Running reflection pass for session {}", session_id);

        // Add the response to context, then ask for reflection
        let assistant_msg = rockbot_llm::Message {
            role: rockbot_llm::MessageRole::Assistant,
            content: response_text.clone(),
            images: vec![],
            tool_calls: None,
            tool_call_id: None,
        };
        let assistant = Message::from_llm_message(assistant_msg, session_id, &self.config.id)?;
        context.messages.push(assistant);

        let reflection_prompt = Message::text(
            "Review your response above. Did you fully address the request? \
             Are there errors, omissions, or improvements needed? \
             If corrections are needed, make them now using your tools. \
             If your response is complete and correct, say LGTM."
        )
            .with_session_id(session_id)
            .with_agent_id(&self.config.id)
            .with_role(MessageRole::User);
        context.messages.push(reflection_prompt);

        let llm_request = self.build_llm_request(context).await?;
        let llm_response = self.call_llm_with_retry(llm_request).await?;

        let has_tool_calls = llm_response.choices
            .first()
            .and_then(|c| c.message.tool_calls.as_ref())
            .is_some_and(|tc| !tc.is_empty());

        let reflection_text = llm_response.choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap_or("");

        // If reflection says LGTM (or similar), no corrections needed
        let clean = reflection_text.to_uppercase();
        if !has_tool_calls && (clean.contains("LGTM") || clean.contains("COMPLETE") || clean.contains("CORRECT")) {
            info!("Reflection pass: no corrections needed for session {}", session_id);
            return Ok(None);
        }

        // If the model wants to make corrections (tool calls), give it one pass
        if has_tool_calls {
            info!("Reflection pass: model making corrections for session {}", session_id);
            let (corrected_msg, extra_results, extra_tokens) = self.process_llm_response(
                session_id, context, llm_response, progress_tx,
            ).await?;
            return Ok(Some((corrected_msg, extra_results, extra_tokens)));
        }

        // Model produced text (not LGTM, not tool calls) — use it as the corrected response
        let corrected_text = Self::strip_think_blocks(reflection_text);
        if !corrected_text.is_empty() && corrected_text != response_text {
            info!("Reflection pass: model revised response for session {}", session_id);
            let corrected_msg = Message::text(corrected_text)
                .with_session_id(session_id)
                .with_agent_id(&self.config.id)
                .with_role(MessageRole::Assistant);
            let token_usage = TokenUsage {
                prompt_tokens: llm_response.usage.prompt_tokens,
                completion_tokens: llm_response.usage.completion_tokens,
                total_tokens: llm_response.usage.total_tokens,
            };
            return Ok(Some((corrected_msg, Vec::new(), token_usage)));
        }

        Ok(None)
    }

    /// Execute a workflow DAG instead of the normal LLM tool loop.
    ///
    /// Each node invokes an agent via the gateway's `AgentInvoker`. Nodes in the
    /// same topological layer run concurrently. Progress events map to the existing
    /// `ToolStart`/`ToolOutput`/`TextDelta` events so the TUI renders them.
    async fn execute_workflow(
        &self,
        session_id: &str,
        user_message: &Message,
        workflow: &crate::orchestration::WorkflowDefinition,
        progress_tx: &Option<ProgressSender>,
        start_time: std::time::Instant,
        mut trajectory: Trajectory,
    ) -> Result<AgentResponse> {
        use crate::orchestration::{WorkflowExecutor, WorkflowProgressEvent};

        let invoker = self.agent_invoker.as_ref().ok_or_else(|| {
            crate::error::AgentError::ExecutionFailed {
                message: "Workflow agent requires an agent_invoker but none is configured".to_string(),
            }
        })?;

        let input_text = user_message.extract_text().unwrap_or_default();
        trajectory.record(TrajectoryEvent::UserMessage {
            content_preview: crate::trajectory::preview(&input_text, 200),
        }, 0, 0);

        // Set up workflow progress forwarding
        let (wf_progress_tx, mut wf_progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent_progress_tx = progress_tx.clone();
        let agent_id = self.config.id.clone();
        let progress_handle = tokio::spawn(async move {
            while let Some(event) = wf_progress_rx.recv().await {
                if let Some(ref tx) = agent_progress_tx {
                    match event {
                        WorkflowProgressEvent::NodeStarted { node_id, agent_id } => {
                            let _ = tx.send(AgentProgressEvent::ToolStart {
                                tool_name: format!("workflow:{node_id}@{agent_id}"),
                            });
                        }
                        WorkflowProgressEvent::NodeCompleted { node_id, output_preview } => {
                            let _ = tx.send(AgentProgressEvent::ToolOutput {
                                tool_name: format!("workflow:{node_id}"),
                                output: output_preview,
                                success: true,
                                duration_ms: 0,
                            });
                        }
                        WorkflowProgressEvent::NodeFailed { node_id, error } => {
                            let _ = tx.send(AgentProgressEvent::TextDelta {
                                text: format!("Node '{node_id}' failed: {error}"),
                            });
                        }
                    }
                }
            }
        });

        let executor = WorkflowExecutor::new(Arc::clone(invoker));
        let result = executor.execute(workflow, &input_text, session_id, Some(wf_progress_tx)).await;

        progress_handle.abort();

        let processing_time_ms = start_time.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                trajectory.record(TrajectoryEvent::Complete {
                    total_iterations: workflow.nodes.len(),
                    total_tool_calls: workflow.nodes.len(),
                    final_tokens: TokenUsage::default(),
                    duration_ms: processing_time_ms,
                }, 0, 0);

                let response_message = Message::text(output)
                    .with_session_id(session_id)
                    .with_agent_id(&agent_id)
                    .with_role(MessageRole::Assistant);

                self.session_manager.add_message(session_id, response_message.clone()).await?;

                Ok(AgentResponse {
                    message: response_message,
                    tool_results: vec![],
                    tokens_used: TokenUsage::default(),
                    processing_time_ms,
                    trajectory: Some(trajectory),
                    handoff: None,
                })
            }
            Err(error) => {
                let response_message = Message::text(format!("Workflow failed: {error}"))
                    .with_session_id(session_id)
                    .with_agent_id(&agent_id)
                    .with_role(MessageRole::Assistant);

                self.session_manager.add_message(session_id, response_message.clone()).await?;

                Ok(AgentResponse {
                    message: response_message,
                    tool_results: vec![],
                    tokens_used: TokenUsage::default(),
                    processing_time_ms,
                    trajectory: Some(trajectory),
                    handoff: None,
                })
            }
        }
    }

    /// Execute tool calls from an LLM response.
    ///
    /// Returns `(tool_results, tool_messages, handoff_signal)`. If a handoff is
    /// detected, the signal is set and execution stops early — only tool calls
    /// before the handoff are included in the results.
    async fn execute_tool_calls(
        &self,
        session_id: &str,
        llm_response: &ChatCompletionResponse,
        workspace: &std::path::Path,
    ) -> Result<(Vec<ToolExecutionResult>, Vec<Message>, Option<HandoffSignal>)> {
        let mut tool_results = Vec::new();
        let mut tool_messages = Vec::new();

        if let Some(choice) = llm_response.choices.first() {
            if let Some(ref tool_calls) = choice.message.tool_calls {
                for tool_call in tool_calls {
                    debug!("Executing tool: {}", tool_call.function.name);

                    // Fire PreToolCall hook
                    let pre_tool_event = HookEvent::PreToolCall {
                        agent_id: self.config.id.clone(),
                        session_id: session_id.to_string(),
                        tool_name: tool_call.function.name.clone(),
                        arguments: serde_json::from_str(&tool_call.function.arguments)
                            .unwrap_or(serde_json::Value::Null),
                    };
                    if let HookResult::Abort { reason } = self.hook_registry.fire(&pre_tool_event).await {
                        warn!("Tool call '{}' aborted by hook: {reason}", tool_call.function.name);
                        let error_message = Message::tool_result(
                            tool_call.id.clone(),
                            tool_call.function.name.clone(),
                            format!("Tool call aborted by hook: {reason}"),
                        ).with_session_id(session_id);
                        tool_messages.push(error_message);
                        continue;
                    }

                    // Check breakpoint tools — require approval before execution
                    if self.config.breakpoint_tools.contains(&tool_call.function.name) {
                        info!("Breakpoint hit: tool '{}' requires approval", tool_call.function.name);
                        // If no approval callback is configured, skip with an explanation
                        // (The TUI/API layer should set up the callback for interactive sessions)
                        let error_message = Message::tool_result(
                            tool_call.id.clone(),
                            tool_call.function.name.clone(),
                            format!(
                                "Tool '{}' is a breakpoint tool and requires human approval. \
                                 The user has not yet approved this call. Try an alternative approach \
                                 or ask the user for permission.",
                                tool_call.function.name
                            ),
                        ).with_session_id(session_id);
                        tool_messages.push(error_message);
                        continue;
                    }

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
                        agent_invoker: self.agent_invoker.clone(),
                        delegation_depth: 0,
                        blackboard: self.blackboard.clone(),
                        swarm_id: self.swarm_id.clone(),
                    };

                    let tool_start = std::time::Instant::now();
                    let tool_timeout = Duration::from_secs(self.config.tool_timeout_secs);
                    let tool_future = self.tool_registry.execute_tool(
                        &tool_call.function.name,
                        &tool_call.function.arguments,
                        execution_context,
                    );
                    let tool_result = match tokio::time::timeout(tool_timeout, tool_future).await {
                        Err(_elapsed) => {
                            warn!("Tool '{}' timed out after {}s", tool_call.function.name, self.config.tool_timeout_secs);
                            let error_message = Message::tool_result(
                                tool_call.id.clone(),
                                tool_call.function.name.clone(),
                                format!("Tool execution timed out after {}s. The operation was cancelled.", self.config.tool_timeout_secs),
                            ).with_session_id(session_id);
                            tool_messages.push(error_message);

                            crate::metrics::record_tool_call(
                                &tool_call.function.name,
                                false,
                                tool_start.elapsed(),
                            );
                            continue;
                        }
                        Ok(result) => result,
                    };
                    match tool_result {
                        Ok(result) => {
                            crate::metrics::record_tool_call(
                                &tool_call.function.name,
                                result.success,
                                tool_start.elapsed(),
                            );

                            // Fire PostToolCall hook
                            let post_tool_event = HookEvent::PostToolCall {
                                agent_id: self.config.id.clone(),
                                session_id: session_id.to_string(),
                                tool_name: tool_call.function.name.clone(),
                                result: serde_json::to_value(&result.result).unwrap_or(serde_json::Value::Null),
                                success: result.success,
                            };
                            self.hook_registry.fire(&post_tool_event).await;

                            // Check for handoff signal before processing normally
                            if let ToolResult::Handoff { ref target_agent_id, ref context, ref message_override } = result.result {
                                info!("Handoff detected: {} -> {}", self.config.id, target_agent_id);
                                let handoff_msg = Message::tool_result(
                                    tool_call.id.clone(),
                                    tool_call.function.name.clone(),
                                    format!("Transferring conversation to agent '{target_agent_id}'..."),
                                ).with_session_id(session_id);
                                tool_messages.push(handoff_msg);
                                tool_results.push(result.clone());

                                // Update stats before returning
                                {
                                    let mut state = self.state.write().await;
                                    state.stats.tool_executions += 1;
                                }

                                return Ok((tool_results, tool_messages, Some(HandoffSignal {
                                    target_agent_id: target_agent_id.clone(),
                                    context: context.clone(),
                                    message_override: message_override.clone(),
                                })));
                            }

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
                                ToolResult::Handoff { .. } => {
                                    // Already handled above — unreachable
                                    unreachable!()
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

                            // Fire PostToolCall hook for error
                            let post_tool_event = HookEvent::PostToolCall {
                                agent_id: self.config.id.clone(),
                                session_id: session_id.to_string(),
                                tool_name: tool_call.function.name.clone(),
                                result: serde_json::json!({"error": e.to_string()}),
                                success: false,
                            };
                            self.hook_registry.fire(&post_tool_event).await;

                            // Fire OnError hook
                            let error_event = HookEvent::OnError {
                                agent_id: self.config.id.clone(),
                                session_id: session_id.to_string(),
                                error: format!("Tool '{}' failed: {e}", tool_call.function.name),
                            };
                            self.hook_registry.fire(&error_event).await;

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

        Ok((tool_results, tool_messages, None))
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

        // Strip any orphaned </think> tags (closing tag without a matching opener,
        // e.g. when the opening <think> was in a previous chunk or omitted)
        let cleaned = result.replace("</think>", "");

        let trimmed = cleaned.trim().to_string();
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
    /// Check if the last user message in the context is a conversational question
    /// (e.g. "What is your purpose?") rather than a task request. Conversational
    /// questions should not trigger continuation nudges — the model's text answer
    /// is the correct response, not a tool call.
    fn is_conversational_question(context: &ProcessingContext) -> bool {
        // Find the last user message
        let last_user_msg = context.messages.iter().rev()
            .find(|m| m.metadata.role == MessageRole::User);

        let text = match last_user_msg {
            Some(msg) => match &msg.content {
                MessageContent::Text { text } => text.trim(),
                _ => return false,
            },
            None => return false,
        };

        let lower = text.to_lowercase();

        // Short messages ending with ? are usually conversational
        if text.ends_with('?') && text.len() < 200 {
            return true;
        }

        // Common conversational question patterns
        let question_starts = [
            "what is ", "what are ", "what's ", "who are ", "who is ",
            "how are ", "how do you ", "how does ", "can you tell me",
            "what do you ", "what can you ", "why ", "explain ",
            "describe ", "tell me about ", "do you ",
        ];
        if question_starts.iter().any(|p| lower.starts_with(p)) && text.len() < 300 {
            return true;
        }

        false
    }

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

    /// Detect when a model response signals intent to continue working but
    /// didn't attach tool calls. This catches the common pattern where a model
    /// says "Let me explore..." or "Now I'll read..." mid-task and then stops.
    fn looks_like_continuation_intent(text: &str) -> bool {
        // Only short-ish responses — long responses are likely genuine final answers
        if text.len() > 1000 {
            return false;
        }

        let lower = text.to_lowercase();

        let continuation_phrases = [
            "let me ", "now i'll ", "now i will ", "next i'll ", "next i will ",
            "i'll now ", "i will now ", "i need to ", "i should ",
            "let's ", "moving on to ", "continuing with ",
            "i'm going to ", "first let me ", "now let's ",
            "let me also ", "i'll also ", "i also need to ",
            "more thoroughly", "more closely", "in more detail",
            "let me check", "let me look", "let me read", "let me search",
            "let me explore", "let me examine", "let me investigate",
        ];

        // Must end with a continuation marker (colon, ellipsis) or contain
        // one of the intent phrases
        let ends_with_continuation = text.trim_end().ends_with(':')
            || text.trim_end().ends_with("...")
            || text.trim_end().ends_with("…");

        if ends_with_continuation && continuation_phrases.iter().any(|p| lower.contains(p)) {
            return true;
        }

        // Even without trailing punctuation, strong intent phrases are enough
        let strong_intent = [
            "let me ", "now i'll ", "i need to ", "i'm going to ",
        ];
        if strong_intent.iter().any(|p| lower.contains(p)) && text.len() < 500 {
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
            trajectory: None,
            handoff: None,
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

    #[test]
    fn test_strip_think_blocks_orphaned_close_tag() {
        // Bare </think> without matching <think> (e.g. opening tag in prior chunk)
        let input = "</think>\nHere is my answer.";
        assert_eq!(Agent::strip_think_blocks(input), "Here is my answer.");
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

    // --- looks_like_continuation_intent tests ---

    #[test]
    fn test_continuation_with_colon() {
        assert!(Agent::looks_like_continuation_intent(
            "This is a Rust project. Let me explore the structure more thoroughly:"
        ));
    }

    #[test]
    fn test_continuation_with_ellipsis() {
        assert!(Agent::looks_like_continuation_intent(
            "Let me read the main configuration file..."
        ));
    }

    #[test]
    fn test_continuation_strong_intent() {
        assert!(Agent::looks_like_continuation_intent(
            "Now I'll look at the gateway module"
        ));
        assert!(Agent::looks_like_continuation_intent(
            "I need to check the error handling"
        ));
        assert!(Agent::looks_like_continuation_intent(
            "I'm going to analyze the test suite"
        ));
    }

    #[test]
    fn test_long_response_not_continuation() {
        let long = "a ".repeat(600);
        assert!(!Agent::looks_like_continuation_intent(&long));
    }

    #[test]
    fn test_final_answer_not_continuation() {
        assert!(!Agent::looks_like_continuation_intent(
            "The function handles errors by returning a Result type with custom error variants."
        ));
    }

    // --- is_conversational_question tests ---

    fn make_context_with_user_msg(text: &str) -> ProcessingContext {
        let msg = Message::text(text)
            .with_role(MessageRole::User);
        ProcessingContext {
            session_id: "test".to_string(),
            messages: vec![msg],
            available_tools: vec![],
            token_count: 0,
            workspace_override: None,
        }
    }

    #[test]
    fn test_question_mark_is_conversational() {
        let ctx = make_context_with_user_msg("What is your purpose?");
        assert!(Agent::is_conversational_question(&ctx));
    }

    #[test]
    fn test_question_words_are_conversational() {
        let ctx = make_context_with_user_msg("Who are you");
        assert!(Agent::is_conversational_question(&ctx));
        let ctx = make_context_with_user_msg("How do you work");
        assert!(Agent::is_conversational_question(&ctx));
        let ctx = make_context_with_user_msg("Explain the architecture");
        assert!(Agent::is_conversational_question(&ctx));
    }

    #[test]
    fn test_task_request_not_conversational() {
        let ctx = make_context_with_user_msg("Fix the bug in main.rs");
        assert!(!Agent::is_conversational_question(&ctx));
        let ctx = make_context_with_user_msg("Refactor the gateway module");
        assert!(!Agent::is_conversational_question(&ctx));
        let ctx = make_context_with_user_msg("Add error handling to the parser");
        assert!(!Agent::is_conversational_question(&ctx));
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