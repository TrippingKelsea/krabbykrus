//! Built-in tool implementations

use crate::message::ToolResult;
use crate::{Tool, ToolExecutionContext, Result};
use krabbykrus_security::Capabilities;
use serde_json::json;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use tokio::process::Command;

/// File reading tool
pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }
    
    fn description(&self) -> &str {
        "Read the contents of a file"
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read",
                    "minimum": 1
                },
                "offset": {
                    "type": "number",
                    "description": "Line number to start reading from (1-indexed)",
                    "minimum": 1
                }
            },
            "required": ["file_path"]
        })
    }
    
    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }
    
    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let file_path: String = params.get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "file_path is required".to_string() 
                })?
                .to_string();
            
            let limit = params.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
            let offset = params.get("offset").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(1);
            
            // Resolve path relative to workspace
            let path = if PathBuf::from(&file_path).is_absolute() {
                PathBuf::from(file_path)
            } else {
                context.workspace_path.join(file_path)
            };
            
            // Read file content
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| crate::ToolError::ExecutionFailed { 
                    message: format!("Failed to read file: {}", e)
                })?;
            
            // Apply offset and limit
            let lines: Vec<&str> = content.lines().collect();
            let start = offset.saturating_sub(1);
            let end = if let Some(limit) = limit {
                (start + limit).min(lines.len())
            } else {
                lines.len()
            };
            
            let result_lines = &lines[start..end];
            let result_content = result_lines.join("\n");
            
            Ok(ToolResult::text(result_content))
        })
    }
}

/// File writing tool
pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }
    
    fn description(&self) -> &str {
        "Write content to a file"
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }
    
    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_write()
    }
    
    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let file_path: String = params.get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "file_path is required".to_string() 
                })?
                .to_string();
            
            let content: String = params.get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "content is required".to_string() 
                })?
                .to_string();
            
            // Resolve path relative to workspace
            let path = if PathBuf::from(&file_path).is_absolute() {
                PathBuf::from(file_path)
            } else {
                context.workspace_path.join(file_path)
            };
            
            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| crate::ToolError::ExecutionFailed { 
                        message: format!("Failed to create directories: {}", e)
                    })?;
            }
            
            // Write content to file
            tokio::fs::write(&path, content.as_bytes()).await
                .map_err(|e| crate::ToolError::ExecutionFailed { 
                    message: format!("Failed to write file: {}", e)
                })?;
            
            let bytes_written = content.len();
            Ok(ToolResult::text(format!("Successfully wrote {} bytes to {}", bytes_written, path.display())))
        })
    }
}

/// File editing tool
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }
    
    fn description(&self) -> &str {
        "Edit a file by replacing exact text"
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to edit"
                },
                "old_text": {
                    "type": "string",
                    "description": "Exact text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "New text to replace the old text with"
                }
            },
            "required": ["file_path", "old_text", "new_text"]
        })
    }
    
    fn required_capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::filesystem_read();
        caps.extend(Capabilities::filesystem_write());
        caps
    }
    
    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let file_path: String = params.get("file_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "file_path is required".to_string() 
                })?
                .to_string();
            
            let old_text: String = params.get("old_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "old_text is required".to_string() 
                })?
                .to_string();
            
            let new_text: String = params.get("new_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "new_text is required".to_string() 
                })?
                .to_string();
            
            // Resolve path relative to workspace
            let path = if PathBuf::from(&file_path).is_absolute() {
                PathBuf::from(file_path)
            } else {
                context.workspace_path.join(file_path)
            };
            
            // Read current content
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| crate::ToolError::ExecutionFailed { 
                    message: format!("Failed to read file: {}", e)
                })?;
            
            // Replace text
            if !content.contains(&old_text) {
                return Ok(ToolResult::error(format!(
                    "Text '{}' not found in file", 
                    old_text.chars().take(50).collect::<String>()
                )));
            }
            
            let new_content = content.replace(&old_text, &new_text);
            
            // Write updated content
            tokio::fs::write(&path, new_content.as_bytes()).await
                .map_err(|e| crate::ToolError::ExecutionFailed { 
                    message: format!("Failed to write file: {}", e)
                })?;
            
            Ok(ToolResult::text(format!("Successfully replaced text in {}", path.display())))
        })
    }
}

/// Command execution tool
pub struct ExecTool;

impl ExecTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for ExecTool {
    fn name(&self) -> &str {
        "exec"
    }
    
    fn description(&self) -> &str {
        "Execute shell commands"
    }
    
    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory (defaults to workspace)"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds"
                }
            },
            "required": ["command"]
        })
    }
    
    fn required_capabilities(&self) -> Capabilities {
        Capabilities::process_execute()
    }
    
    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let command: String = params.get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters { 
                    message: "command is required".to_string() 
                })?
                .to_string();
            
            let workdir = params.get("workdir")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| context.workspace_path.clone());
            
            let timeout = params.get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(30); // Default 30 second timeout
            
            // Execute command with timeout
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(&command).current_dir(&workdir);
            
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                cmd.output()
            ).await
            .map_err(|_| crate::ToolError::ExecutionFailed { 
                message: "Command timed out".to_string()
            })?
            .map_err(|e| crate::ToolError::ExecutionFailed { 
                message: format!("Failed to execute command: {}", e)
            })?;
            
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);
            
            let result = json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "success": output.status.success()
            });
            
            Ok(ToolResult::json(result))
        })
    }
}