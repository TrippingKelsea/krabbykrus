//! Built-in tools for RockBot
//!
//! This crate provides the tool system for RockBot agents, including:
//!
//! - **Tool Registry**: Profile-based tool loading and capability filtering
//! - **Built-in Tools**: read, write, edit, exec
//! - **Credential Injection**: Secure access to credentials from the vault
//!
//! # Credential Injection
//!
//! Tools can request credentials from the vault via the `CredentialAccessor` trait:
//!
//! ```ignore
//! async fn make_api_call(context: &ToolExecutionContext, url: &str) -> Result<String> {
//!     if let Some(accessor) = &context.credential_accessor {
//!         let result = accessor.get_credential("saggyclaw://myapi/token", &context.agent_id).await?;
//!         match result {
//!             CredentialResult::Granted { secret, credential_type } => {
//!                 // Use the credential to make the API call
//!             }
//!             CredentialResult::Denied { reason } => {
//!                 return Err(ToolError::SecurityError { message: reason });
//!             }
//!             _ => { /* handle other cases */ }
//!         }
//!     }
//!     Ok("response".to_string())
//! }
//! ```

use crate::message::ToolResult;
pub use rockbot_credentials_schema::CredentialSchema;
use rockbot_security::{Capabilities, SecurityContext};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

pub mod builtin;

/// Result type for tool operations
pub type Result<T> = std::result::Result<T, ToolError>;

/// Tool execution errors
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Tool '{name}' not found")]
    NotFound { name: String },
    
    #[error("Invalid parameters: {message}")]
    InvalidParameters { message: String },
    
    #[error("Execution failed: {message}")]
    ExecutionFailed { message: String },
    
    #[error("Security error: {message}")]
    SecurityError { message: String },
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Tool registry manages available tools
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
    config: ToolConfig,
}

/// Tool configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolConfig {
    pub profile: String,
    pub deny: Vec<String>,
    pub configs: HashMap<String, HashMap<String, serde_json::Value>>,
}

/// Result of a human-in-the-loop approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ApprovalResult {
    /// Tool execution is approved
    Approved,
    /// Tool execution is denied
    Denied { reason: String },
    /// Approval is pending (async flow — client must re-submit with approval)
    Pending { request_id: String },
}

/// Callback for requesting human approval before tool execution
pub type ApprovalCallback = Arc<
    dyn Fn(String, String, serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ApprovalResult> + Send>>
        + Send
        + Sync,
>;

/// Tool execution context
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_path: PathBuf,
    pub security_context: SecurityContext,
    /// Optional credential accessor for tools that need API credentials
    pub credential_accessor: Option<Arc<dyn CredentialAccessor>>,
    /// Pre-approved commands that skip HIL (e.g. ["ls", "cat", "git"])
    pub command_allowlist: Vec<String>,
    /// Optional callback for requesting human approval
    pub approval_callback: Option<ApprovalCallback>,
    /// Optional agent invoker for subagent delegation
    pub agent_invoker: Option<Arc<dyn AgentInvoker>>,
    /// Current delegation depth (0 = top-level, incremented for subagent calls)
    pub delegation_depth: u32,
    /// Optional blackboard accessor for swarm coordination
    pub blackboard: Option<Arc<dyn BlackboardAccessor>>,
    /// Swarm ID this agent belongs to (if any)
    pub swarm_id: Option<String>,
}

impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("session_id", &self.session_id)
            .field("agent_id", &self.agent_id)
            .field("workspace_path", &self.workspace_path)
            .field("security_context", &self.security_context)
            .field("has_credential_accessor", &self.credential_accessor.is_some())
            .field("command_allowlist", &self.command_allowlist)
            .field("has_approval_callback", &self.approval_callback.is_some())
            .field("has_agent_invoker", &self.agent_invoker.is_some())
            .field("delegation_depth", &self.delegation_depth)
            .field("has_blackboard", &self.blackboard.is_some())
            .field("swarm_id", &self.swarm_id)
            .finish()
    }
}

/// Agent invoker trait for delegating work to other agents (subagent pattern).
///
/// Implemented by the Gateway which has access to all registered agents.
#[async_trait::async_trait]
pub trait AgentInvoker: Send + Sync {
    /// Invoke another agent with a message, returning the response text.
    /// `depth` tracks delegation depth to prevent infinite recursion.
    async fn invoke_agent(
        &self,
        agent_id: &str,
        message: &str,
        session_id: &str,
        depth: u32,
    ) -> Result<String>;
}

/// Shared blackboard for swarm-style coordination between agents.
///
/// Agents in the same swarm can read/write key-value pairs on a shared
/// blackboard identified by `swarm_id`. This enables asynchronous
/// state passing between handoff steps.
#[async_trait::async_trait]
pub trait BlackboardAccessor: Send + Sync {
    /// Read a single key from the swarm's blackboard.
    async fn read(&self, swarm_id: &str, key: &str) -> Option<serde_json::Value>;
    /// Write a key-value pair to the swarm's blackboard.
    async fn write(&self, swarm_id: &str, key: &str, value: serde_json::Value);
    /// Read all entries from the swarm's blackboard.
    async fn read_all(&self, swarm_id: &str) -> HashMap<String, serde_json::Value>;
}

/// Credential accessor trait for tools to request credentials from the vault
#[async_trait::async_trait]
pub trait CredentialAccessor: Send + Sync {
    /// Get a credential for the given path (e.g., "saggyclaw://homeassistant/api")
    /// Returns the decrypted secret bytes if access is allowed
    async fn get_credential(&self, path: &str, agent_id: &str) -> Result<CredentialResult>;
    
    /// Check if a credential exists without retrieving it
    async fn has_credential(&self, path: &str) -> Result<bool>;
}

/// Result of a credential request
#[derive(Debug, Clone)]
pub enum CredentialResult {
    /// Credential was granted
    Granted {
        /// The decrypted secret
        secret: Vec<u8>,
        /// How to apply the credential (Bearer, Basic, ApiKey, etc.)
        credential_type: CredentialApplicationType,
    },
    /// Access denied
    Denied {
        reason: String,
    },
    /// Requires human approval (HIL)
    PendingApproval {
        request_id: String,
        message: String,
    },
    /// Credential not found
    NotFound {
        path: String,
    },
}

/// How to apply a credential to a request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialApplicationType {
    /// Bearer token in Authorization header
    BearerToken,
    /// Basic auth
    BasicAuth { username: String },
    /// API key in custom header
    ApiKey { header_name: String },
    /// Raw secret (tool handles application)
    Raw,
}

/// Tool execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub tool_name: String,
    pub result: ToolResult,
    pub execution_time_ms: u64,
    pub success: bool,
}

/// Tool definition for LLM integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    /// Whether this tool requires human approval before execution
    #[serde(default)]
    pub requires_approval: bool,
}

/// Core tool trait
pub trait Tool: Send + Sync {
    /// Tool name
    fn name(&self) -> &str;
    
    /// Tool description
    fn description(&self) -> &str;
    
    /// Tool parameters schema (JSON Schema)
    fn parameters(&self) -> serde_json::Value;
    
    /// Required capabilities
    fn required_capabilities(&self) -> Capabilities;
    
    /// Execute the tool
    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>>;

    /// Credential schema describing what this tool needs to authenticate.
    fn credential_schema(&self) -> Option<CredentialSchema> {
        None
    }

    /// Whether this tool requires human-in-the-loop approval before execution.
    /// Tools like process_execute and file_write should return true.
    fn requires_approval(&self) -> bool {
        false
    }
}

/// Tool provider registry — collects tool plugins and their credential schemas.
///
/// This is separate from `ToolRegistry` (which manages agent-accessible tools).
/// `ToolProviderRegistry` tracks external service integrations that need
/// credentials configured via the UI.
pub struct ToolProviderRegistry {
    providers: HashMap<String, Arc<dyn Tool>>,
}

impl ToolProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a tool provider
    pub fn register(&mut self, provider: Arc<dyn Tool>) {
        let id = provider.name().to_string();
        self.providers.insert(id, provider);
    }

    /// List registered provider IDs
    pub fn list(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Collect credential schemas from all registered tool providers
    pub fn credential_schemas(&self) -> Vec<CredentialSchema> {
        self.providers
            .values()
            .filter_map(|p| p.credential_schema())
            .collect()
    }
}

impl Default for ToolProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Agent-as-Tool: wraps an entire agent as a callable tool for other agents
// ---------------------------------------------------------------------------

/// A tool that delegates execution to another agent via `AgentInvoker`.
///
/// This allows agents to call other agents as if they were tools, with
/// the delegation depth tracked to prevent infinite recursion.
pub struct AgentTool {
    /// The target agent ID to invoke
    agent_id: String,
    /// Tool name visible to calling agents
    tool_name: String,
    /// Description of what this agent-tool does
    tool_description: String,
}

impl AgentTool {
    pub fn new(agent_id: String, tool_name: String, tool_description: String) -> Self {
        Self { agent_id, tool_name, tool_description }
    }
}

impl Tool for AgentTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &str {
        &self.tool_description
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The task or question to send to the agent"
                }
            },
            "required": ["message"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let message = params.get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidParameters {
                    message: "Missing required 'message' parameter".to_string(),
                })?;

            let invoker = context.agent_invoker.as_ref()
                .ok_or_else(|| ToolError::ExecutionFailed {
                    message: "No agent invoker available for agent-as-tool delegation".to_string(),
                })?;

            let max_depth = 3u32;
            if context.delegation_depth >= max_depth {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "Agent delegation depth limit ({max_depth}) exceeded"
                    ),
                });
            }

            // Prevent self-delegation
            if self.agent_id == context.agent_id {
                return Err(ToolError::ExecutionFailed {
                    message: "Agent cannot delegate to itself".to_string(),
                });
            }

            match invoker.invoke_agent(
                &self.agent_id,
                message,
                &context.session_id,
                context.delegation_depth + 1,
            ).await {
                Ok(response) => Ok(ToolResult::Text { content: response }),
                Err(e) => Err(ToolError::ExecutionFailed {
                    message: format!("Agent '{}' invocation failed: {e}", self.agent_id),
                }),
            }
        })
    }
}

impl ToolRegistry {
    /// Create a new tool registry
    pub async fn new(config: ToolConfig) -> Result<Self> {
        let registry = Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            config,
        };
        
        // Register built-in tools based on profile
        registry.register_builtin_tools().await?;
        
        Ok(registry)
    }
    
    /// Register built-in tools based on configuration
    async fn register_builtin_tools(&self) -> Result<()> {
        let tools_to_register = match self.config.profile.as_str() {
            "minimal" => vec!["read", "write"],
            "standard" => vec!["read", "write", "edit", "exec", "glob", "grep", "patch", "invoke_agent", "handoff", "web_fetch", "web_search", "test", "lint", "clarify"],
            "full" => vec!["read", "write", "edit", "exec", "glob", "grep", "patch", "memory_get", "memory_search", "invoke_agent", "handoff", "web_fetch", "web_search", "browser", "test", "lint", "clarify", "blackboard_read", "blackboard_write"],
            _ => vec!["read", "write", "edit", "exec", "glob", "grep", "patch"],
        };
        
        for tool_name in tools_to_register {
            if self.config.deny.contains(&tool_name.to_string()) {
                continue;
            }
            
            if let Some(tool) = self.create_builtin_tool(tool_name).await? {
                self.register_tool(tool).await;
            }
        }
        
        Ok(())
    }
    
    /// Create a built-in tool by name
    async fn create_builtin_tool(&self, name: &str) -> Result<Option<Arc<dyn Tool>>> {
        match name {
            "read" => Ok(Some(Arc::new(builtin::ReadTool::new()))),
            "write" => Ok(Some(Arc::new(builtin::WriteTool::new()))),
            "edit" => Ok(Some(Arc::new(builtin::EditTool::new()))),
            "exec" => Ok(Some(Arc::new(builtin::ExecTool::new()))),
            "glob" => Ok(Some(Arc::new(builtin::GlobTool::new()))),
            "grep" => Ok(Some(Arc::new(builtin::GrepTool::new()))),
            "patch" => Ok(Some(Arc::new(builtin::PatchTool::new()))),
            "memory_get" => Ok(Some(Arc::new(builtin::MemoryGetTool::new()))),
            "memory_search" => Ok(Some(Arc::new(builtin::MemorySearchTool::new()))),
            "invoke_agent" => Ok(Some(Arc::new(builtin::InvokeAgentTool::new()))),
            "handoff" => Ok(Some(Arc::new(builtin::HandoffTool::new()))),
            "blackboard_read" => Ok(Some(Arc::new(builtin::BlackboardReadTool::new()))),
            "blackboard_write" => Ok(Some(Arc::new(builtin::BlackboardWriteTool::new()))),
            "web_fetch" => Ok(Some(Arc::new(builtin::WebFetchTool::new()))),
            "web_search" => Ok(Some(Arc::new(builtin::WebSearchTool::new()))),
            "browser" => Ok(Some(Arc::new(builtin::BrowserTool::new()))),
            "test" => Ok(Some(Arc::new(builtin::TestTool::new()))),
            "lint" => Ok(Some(Arc::new(builtin::LintTool::new()))),
            "clarify" => Ok(Some(Arc::new(builtin::ClarifyTool::new()))),
            _ => Ok(None),
        }
    }
    
    /// Register a tool
    pub async fn register_tool(&self, tool: Arc<dyn Tool>) {
        let mut tools = self.tools.write().await;
        tools.insert(tool.name().to_string(), tool);
    }
    
    /// Get available tools for given capabilities
    pub async fn get_available_tools(&self, capabilities: &Capabilities) -> Result<Vec<Arc<dyn Tool>>> {
        let tools = self.tools.read().await;
        let mut available = Vec::new();
        
        for tool in tools.values() {
            if capabilities.allows(&tool.required_capabilities()) {
                available.push(tool.clone());
            }
        }
        
        Ok(available)
    }
    
    /// Get tool definitions for LLM integration
    pub async fn get_tool_definitions(&self, tool_names: &[String]) -> Result<Vec<ToolDefinition>> {
        let tools = self.tools.read().await;
        let mut definitions = Vec::new();
        
        for tool_name in tool_names {
            if let Some(tool) = tools.get(tool_name) {
                definitions.push(ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    parameters: tool.parameters(),
                    requires_approval: tool.requires_approval(),
                });
            }
        }
        
        Ok(definitions)
    }
    
    /// Execute a tool by name
    pub async fn execute_tool(
        &self,
        tool_name: &str,
        params: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult> {
        let start_time = std::time::Instant::now();
        
        let tools = self.tools.read().await;
        let tool = tools.get(tool_name)
            .ok_or_else(|| ToolError::NotFound { name: tool_name.to_string() })?;
        
        // Parse parameters
        let params: serde_json::Value = serde_json::from_str(params)
            .map_err(|e| ToolError::InvalidParameters { message: e.to_string() })?;
        
        // Check capabilities
        if !context.security_context.capabilities.allows(&tool.required_capabilities()) {
            return Err(ToolError::SecurityError {
                message: "Insufficient capabilities for tool execution".to_string(),
            });
        }

        // Check if tool requires human approval
        if tool.requires_approval() {
            // Check command allowlist for exec-like tools
            let is_allowlisted = params.get("command")
                .and_then(|v| v.as_str())
                .and_then(|cmd| cmd.split_whitespace().next())
                .is_some_and(|exe| context.command_allowlist.iter().any(|a| a == exe));

            if !is_allowlisted {
                if let Some(ref callback) = context.approval_callback {
                    let result = callback(
                        tool_name.to_string(),
                        context.agent_id.clone(),
                        params.clone(),
                    ).await;
                    match result {
                        ApprovalResult::Approved => { /* proceed */ }
                        ApprovalResult::Denied { reason } => {
                            return Ok(ToolExecutionResult {
                                tool_name: tool_name.to_string(),
                                result: ToolResult::error(format!("Tool execution denied: {reason}")),
                                execution_time_ms: start_time.elapsed().as_millis() as u64,
                                success: false,
                            });
                        }
                        ApprovalResult::Pending { request_id } => {
                            return Ok(ToolExecutionResult {
                                tool_name: tool_name.to_string(),
                                result: ToolResult::error(format!("Approval pending: {request_id}")),
                                execution_time_ms: start_time.elapsed().as_millis() as u64,
                                success: false,
                            });
                        }
                    }
                }
                // If no approval callback, proceed (autonomous mode)
            }
        }

        // Enforce sandbox restrictions
        let restrictions = &context.security_context.restrictions;

        // Check file path restrictions for file-related tools
        if let Some(path_val) = params.get("path").and_then(|v| v.as_str()) {
            let path = std::path::Path::new(path_val);
            if let rockbot_security::EnforcementResult::Denied { reason } =
                rockbot_security::enforce_path(path, restrictions)
            {
                return Err(ToolError::SecurityError { message: reason });
            }
        }

        // Check executable restrictions for exec-like tools
        if let Some(cmd_val) = params.get("command").and_then(|v| v.as_str()) {
            // Extract the executable name (first word of the command)
            let executable = cmd_val.split_whitespace().next().unwrap_or(cmd_val);
            if let rockbot_security::EnforcementResult::Denied { reason } =
                rockbot_security::enforce_executable(executable, restrictions)
            {
                return Err(ToolError::SecurityError { message: reason });
            }
        }

        // Execute tool
        let result = tool.execute(params, context).await;
        let execution_time_ms = start_time.elapsed().as_millis() as u64;
        
        let (tool_result, success) = match result {
            Ok(result) => (result, true),
            Err(e) => (ToolResult::error(e.to_string()), false),
        };
        
        Ok(ToolExecutionResult {
            tool_name: tool_name.to_string(),
            result: tool_result,
            execution_time_ms,
            success,
        })
    }
}

// Message types for compatibility
pub mod message {
    use serde::{Deserialize, Serialize};
    
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum ToolResult {
        Text { content: String },
        Json { data: serde_json::Value },
        File { path: String, content: Option<Vec<u8>>, mime_type: Option<String> },
        Error { message: String, code: Option<String>, details: Option<serde_json::Value> },
        Handoff {
            target_agent_id: String,
            context: String,
            message_override: Option<String>,
        },
    }

    impl ToolResult {
        pub fn text<S: Into<String>>(content: S) -> Self {
            Self::Text { content: content.into() }
        }

        pub fn json(data: serde_json::Value) -> Self {
            Self::Json { data }
        }

        pub fn error<S: Into<String>>(message: S) -> Self {
            Self::Error {
                message: message.into(),
                code: None,
                details: None,
            }
        }

        pub fn handoff<S: Into<String>, C: Into<String>>(target_agent_id: S, context: C) -> Self {
            Self::Handoff {
                target_agent_id: target_agent_id.into(),
                context: context.into(),
                message_override: None,
            }
        }
    }
}

/// Mock tool registry for testing
pub struct MockToolRegistry;

impl Default for MockToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MockToolRegistry {
    pub fn new() -> Self {
        Self
    }
    
    pub async fn get_available_tools(&self, _capabilities: &rockbot_security::Capabilities) -> Result<Vec<Arc<dyn Tool>>> {
        Ok(Vec::new())
    }
    
    pub async fn get_tool_definitions(&self, _tool_names: &[String]) -> Result<Vec<ToolDefinition>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    
    #[tokio::test]
    async fn test_tool_registry_creation() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };

        let registry = ToolRegistry::new(config).await.unwrap();

        // Should have registered standard tools
        let tools = registry.tools.read().await;
        assert!(tools.contains_key("read"));
        assert!(tools.contains_key("write"));
        assert!(tools.contains_key("exec"));
    }

    #[tokio::test]
    async fn test_approval_required_tools_denied_by_callback() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();

        let context = ToolExecutionContext {
            session_id: "test".to_string(),
            agent_id: "agent1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp"),
            security_context: rockbot_security::SecurityContext {
                session_id: "test".to_string(),
                capabilities: {
                    let mut caps = rockbot_security::Capabilities::new();
                    caps.add(rockbot_security::Capability::ProcessExecute);
                    caps
                },
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec![],
            approval_callback: Some(Arc::new(|_tool, _agent, _params| {
                Box::pin(async { ApprovalResult::Denied { reason: "not allowed".to_string() } })
            })),
            agent_invoker: None,
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        };

        let result = registry.execute_tool(
            "exec",
            r#"{"command": "echo hello"}"#,
            context,
        ).await.unwrap();

        assert!(!result.success);
        match &result.result {
            crate::message::ToolResult::Error { message, .. } => {
                assert!(message.contains("denied"));
            }
            other => panic!("Expected Error result, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_allowlisted_command_skips_approval() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();

        let context = ToolExecutionContext {
            session_id: "test".to_string(),
            agent_id: "agent1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp"),
            security_context: rockbot_security::SecurityContext {
                session_id: "test".to_string(),
                capabilities: {
                    let mut caps = rockbot_security::Capabilities::new();
                    caps.add(rockbot_security::Capability::ProcessExecute);
                    caps
                },
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec!["echo".to_string()],
            approval_callback: Some(Arc::new(|_tool, _agent, _params| {
                // Should never be called for allowlisted commands
                Box::pin(async { ApprovalResult::Denied { reason: "should not reach".to_string() } })
            })),
            agent_invoker: None,
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        };

        let result = registry.execute_tool(
            "exec",
            r#"{"command": "echo hello"}"#,
            context,
        ).await.unwrap();

        // echo should succeed since it's allowlisted, skipping the deny callback
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_write_tool_requires_approval() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();
        let tools = registry.tools.read().await;
        let write_tool = tools.get("write").unwrap();
        assert!(write_tool.requires_approval());
    }

    #[tokio::test]
    async fn test_read_tool_no_approval() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();
        let tools = registry.tools.read().await;
        let read_tool = tools.get("read").unwrap();
        assert!(!read_tool.requires_approval());
    }

    /// Mock agent invoker for testing
    struct MockAgentInvoker;

    #[async_trait::async_trait]
    impl AgentInvoker for MockAgentInvoker {
        async fn invoke_agent(
            &self,
            agent_id: &str,
            message: &str,
            _session_id: &str,
            _depth: u32,
        ) -> Result<String> {
            Ok(format!("Response from {agent_id}: processed '{message}'"))
        }
    }

    #[tokio::test]
    async fn test_invoke_agent_tool_success() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();

        let context = ToolExecutionContext {
            session_id: "test".to_string(),
            agent_id: "agent1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp"),
            security_context: rockbot_security::SecurityContext {
                session_id: "test".to_string(),
                capabilities: rockbot_security::Capabilities::new(),
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec![],
            approval_callback: None,
            agent_invoker: Some(Arc::new(MockAgentInvoker)),
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        };

        let result = registry.execute_tool(
            "invoke_agent",
            r#"{"agent_id": "agent2", "message": "do something"}"#,
            context,
        ).await.unwrap();

        assert!(result.success);
        match &result.result {
            crate::message::ToolResult::Text { content } => {
                assert!(content.contains("agent2"));
                assert!(content.contains("do something"));
            }
            other => panic!("Expected Text result, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_invoke_agent_depth_limit() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();

        let context = ToolExecutionContext {
            session_id: "test".to_string(),
            agent_id: "agent1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp"),
            security_context: rockbot_security::SecurityContext {
                session_id: "test".to_string(),
                capabilities: rockbot_security::Capabilities::new(),
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec![],
            approval_callback: None,
            agent_invoker: Some(Arc::new(MockAgentInvoker)),
            delegation_depth: 3, // At max depth
            blackboard: None,
            swarm_id: None,
        };

        let result = registry.execute_tool(
            "invoke_agent",
            r#"{"agent_id": "agent2", "message": "do something"}"#,
            context,
        ).await.unwrap();

        // Tool returns an error message via ToolResult::Error (not Err)
        match &result.result {
            crate::message::ToolResult::Error { message, .. } => {
                assert!(message.contains("depth"));
            }
            other => panic!("Expected Error result, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_invoke_agent_self_delegation_blocked() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();

        let context = ToolExecutionContext {
            session_id: "test".to_string(),
            agent_id: "agent1".to_string(),
            workspace_path: std::path::PathBuf::from("/tmp"),
            security_context: rockbot_security::SecurityContext {
                session_id: "test".to_string(),
                capabilities: rockbot_security::Capabilities::new(),
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec![],
            approval_callback: None,
            agent_invoker: Some(Arc::new(MockAgentInvoker)),
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        };

        let result = registry.execute_tool(
            "invoke_agent",
            r#"{"agent_id": "agent1", "message": "do something"}"#,
            context,
        ).await.unwrap();

        // Tool returns an error message via ToolResult::Error (not Err)
        match &result.result {
            crate::message::ToolResult::Error { message, .. } => {
                assert!(message.contains("self"));
            }
            other => panic!("Expected Error result, got: {other:?}"),
        }
    }

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let result = crate::builtin::strip_html_tags(html);
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
        assert!(!result.contains("<"));
    }

    #[tokio::test]
    async fn test_web_tools_registered() {
        let config = ToolConfig {
            profile: "standard".to_string(),
            deny: vec![],
            configs: HashMap::new(),
        };
        let registry = ToolRegistry::new(config).await.unwrap();
        let tools = registry.tools.read().await;
        assert!(tools.contains_key("web_fetch"));
        assert!(tools.contains_key("web_search"));
    }
}