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

/// Tool execution context
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub session_id: String,
    pub agent_id: String,
    pub workspace_path: PathBuf,
    pub security_context: SecurityContext,
    /// Optional credential accessor for tools that need API credentials
    pub credential_accessor: Option<Arc<dyn CredentialAccessor>>,
}

impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("session_id", &self.session_id)
            .field("agent_id", &self.agent_id)
            .field("workspace_path", &self.workspace_path)
            .field("security_context", &self.security_context)
            .field("has_credential_accessor", &self.credential_accessor.is_some())
            .finish()
    }
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
            "standard" => vec!["read", "write", "edit", "exec", "glob", "grep", "patch"],
            "full" => vec!["read", "write", "edit", "exec", "glob", "grep", "patch", "memory_get", "memory_search"],
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
    }
}

/// Mock tool registry for testing
pub struct MockToolRegistry;

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
}