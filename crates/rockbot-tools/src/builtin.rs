//! Built-in tool implementations

use crate::message::ToolResult;
use crate::{Tool, ToolExecutionContext, Result};
use rockbot_security::Capabilities;
use serde_json::json;
use std::path::PathBuf;
use std::pin::Pin;
use std::future::Future;
use tokio::process::Command;
use regex::Regex;
use std::io::BufRead;

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

/// File glob pattern matching tool
pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to match files (e.g. '**/*.rs', 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search from (defaults to workspace)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }

    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let pattern: String = params.get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "pattern is required".to_string()
                })?
                .to_string();

            let base_dir = params.get("path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| context.workspace_path.clone());

            // Build full glob pattern
            let full_pattern = if PathBuf::from(&pattern).is_absolute() {
                pattern
            } else {
                format!("{}/{}", base_dir.display(), pattern)
            };

            let entries = glob::glob(&full_pattern)
                .map_err(|e| crate::ToolError::InvalidParameters {
                    message: format!("Invalid glob pattern: {}", e)
                })?;

            let mut matches: Vec<String> = Vec::new();
            for entry in entries {
                match entry {
                    Ok(path) => {
                        // Return paths relative to base_dir when possible
                        let display_path = path.strip_prefix(&base_dir)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| path.display().to_string());
                        matches.push(display_path);
                    }
                    Err(e) => {
                        tracing::warn!("Glob error for entry: {}", e);
                    }
                }
            }

            matches.sort();

            Ok(ToolResult::json(json!({
                "matches": matches,
                "count": matches.len()
            })))
        })
    }
}

/// Content search tool using regex
pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using regex patterns"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file to search in (defaults to workspace)"
                },
                "include": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g. '*.rs', '*.py')"
                },
                "max_results": {
                    "type": "number",
                    "description": "Maximum number of matches to return (default: 100)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }

    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let pattern_str: String = params.get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "pattern is required".to_string()
                })?
                .to_string();

            let search_path = params.get("path")
                .and_then(|v| v.as_str())
                .map(|p| {
                    let pb = PathBuf::from(p);
                    if pb.is_absolute() { pb } else { context.workspace_path.join(p) }
                })
                .unwrap_or_else(|| context.workspace_path.clone());

            let include_pattern = params.get("include")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let max_results = params.get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as usize;

            let regex = Regex::new(&pattern_str)
                .map_err(|e| crate::ToolError::InvalidParameters {
                    message: format!("Invalid regex: {}", e)
                })?;

            let mut results: Vec<serde_json::Value> = Vec::new();

            // Collect files to search
            let files = if search_path.is_file() {
                vec![search_path.clone()]
            } else {
                let glob_pattern = if let Some(ref include) = include_pattern {
                    format!("{}/**/{}", search_path.display(), include)
                } else {
                    format!("{}/**/*", search_path.display())
                };

                glob::glob(&glob_pattern)
                    .map_err(|e| crate::ToolError::ExecutionFailed {
                        message: format!("Glob error: {}", e)
                    })?
                    .filter_map(|e| e.ok())
                    .filter(|p| p.is_file())
                    .collect()
            };

            'outer: for file_path in files {
                // Skip binary files
                let file = match std::fs::File::open(&file_path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let reader = std::io::BufReader::new(file);

                for (line_num, line) in reader.lines().enumerate() {
                    let line = match line {
                        Ok(l) => l,
                        Err(_) => continue,
                    };

                    if regex.is_match(&line) {
                        let display_path = file_path.strip_prefix(&context.workspace_path)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| file_path.display().to_string());

                        results.push(json!({
                            "file": display_path,
                            "line": line_num + 1,
                            "content": line.chars().take(500).collect::<String>()
                        }));

                        if results.len() >= max_results {
                            break 'outer;
                        }
                    }
                }
            }

            Ok(ToolResult::json(json!({
                "matches": results,
                "count": results.len(),
                "truncated": results.len() >= max_results
            })))
        })
    }
}

/// Unified diff patch application tool
pub struct PatchTool;

impl PatchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a file"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to patch"
                },
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content"
                }
            },
            "required": ["file_path", "patch"]
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

            let patch: String = params.get("patch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "patch is required".to_string()
                })?
                .to_string();

            let path = if PathBuf::from(&file_path).is_absolute() {
                PathBuf::from(&file_path)
            } else {
                context.workspace_path.join(&file_path)
            };

            // Read existing file content
            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read file: {}", e)
                })?;

            let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
            let mut hunks_applied = 0;
            let mut offset: i64 = 0;

            // Parse unified diff hunks
            let hunk_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
                .unwrap();

            let patch_lines: Vec<&str> = patch.lines().collect();
            let mut i = 0;

            while i < patch_lines.len() {
                if let Some(caps) = hunk_re.captures(patch_lines[i]) {
                    let orig_start: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(1);

                    i += 1;

                    let mut removals: Vec<usize> = Vec::new();
                    let mut additions: Vec<(usize, String)> = Vec::new();
                    let mut pos = ((orig_start - 1) as i64 + offset) as usize;

                    while i < patch_lines.len() && !patch_lines[i].starts_with("@@") {
                        let line = patch_lines[i];
                        if let Some(removed) = line.strip_prefix('-') {
                            removals.push(pos);
                            pos += 1;
                            let _ = removed; // content used for verification
                        } else if let Some(added) = line.strip_prefix('+') {
                            additions.push((pos, added.to_string()));
                        } else if line.starts_with(' ') || line.is_empty() {
                            pos += 1;
                        }
                        i += 1;
                    }

                    // Apply removals in reverse order
                    for &idx in removals.iter().rev() {
                        if idx < lines.len() {
                            lines.remove(idx);
                        }
                    }

                    // Apply additions
                    let add_offset = removals.iter().filter(|&&r| r <= additions.first().map(|a| a.0).unwrap_or(usize::MAX)).count();
                    for (j, (idx, content)) in additions.iter().enumerate() {
                        let insert_pos = (*idx - add_offset + j).min(lines.len());
                        lines.insert(insert_pos, content.clone());
                    }

                    offset += additions.len() as i64 - removals.len() as i64;
                    hunks_applied += 1;
                } else {
                    i += 1;
                }
            }

            if hunks_applied == 0 {
                return Ok(ToolResult::error("No valid diff hunks found in patch"));
            }

            // Write patched content
            let new_content = lines.join("\n");
            tokio::fs::write(&path, new_content.as_bytes()).await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to write patched file: {}", e)
                })?;

            Ok(ToolResult::text(format!("Successfully applied {} hunk(s) to {}", hunks_applied, path.display())))
        })
    }
}

/// Memory retrieval tool
pub struct MemoryGetTool;

impl MemoryGetTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
    }

    fn description(&self) -> &str {
        "Retrieve a value from the agent's memory store"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Memory key to retrieve"
                }
            },
            "required": ["key"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }

    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let key: String = params.get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "key is required".to_string()
                })?
                .to_string();

            // Memory is stored as JSON in the agent's workspace
            let memory_path = context.workspace_path.join(".memory").join("core.json");

            if !memory_path.exists() {
                return Ok(ToolResult::json(json!({
                    "key": key,
                    "found": false,
                    "value": null
                })));
            }

            let content = tokio::fs::read_to_string(&memory_path).await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read memory: {}", e)
                })?;

            let memory: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to parse memory: {}", e)
                })?;

            let value = memory.get(&key);

            Ok(ToolResult::json(json!({
                "key": key,
                "found": value.is_some(),
                "value": value
            })))
        })
    }
}

/// Memory search tool
pub struct MemorySearchTool;

impl MemorySearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search the agent's memory store by keyword"
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find relevant memories"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of results (default: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::filesystem_read()
    }

    fn execute(&self, params: serde_json::Value, context: ToolExecutionContext) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let query: String = params.get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "query is required".to_string()
                })?
                .to_string();

            let limit = params.get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;

            let memory_dir = context.workspace_path.join(".memory");
            let mut results: Vec<serde_json::Value> = Vec::new();

            if !memory_dir.exists() {
                return Ok(ToolResult::json(json!({
                    "results": [],
                    "count": 0
                })));
            }

            let query_lower = query.to_lowercase();
            let query_terms: Vec<&str> = query_lower.split_whitespace().collect();

            // Search through all memory files
            let glob_pattern = format!("{}/**/*", memory_dir.display());
            let entries = glob::glob(&glob_pattern)
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Glob error: {}", e)
                })?;

            for entry in entries.filter_map(|e| e.ok()).filter(|p| p.is_file()) {
                let content = match std::fs::read_to_string(&entry) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let content_lower = content.to_lowercase();

                // Score by number of matching terms
                let score: usize = query_terms.iter()
                    .filter(|term| content_lower.contains(*term))
                    .count();

                if score > 0 {
                    let display_path = entry.strip_prefix(&memory_dir)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| entry.display().to_string());

                    // Extract relevant snippet
                    let snippet = content.lines()
                        .filter(|line| {
                            let line_lower = line.to_lowercase();
                            query_terms.iter().any(|term| line_lower.contains(term))
                        })
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("\n");

                    results.push(json!({
                        "file": display_path,
                        "score": score,
                        "snippet": snippet
                    }));
                }
            }

            // Sort by score descending
            results.sort_by(|a, b| {
                b.get("score").and_then(|v| v.as_u64()).unwrap_or(0)
                    .cmp(&a.get("score").and_then(|v| v.as_u64()).unwrap_or(0))
            });
            results.truncate(limit);

            let count = results.len();
            Ok(ToolResult::json(json!({
                "results": results,
                "count": count
            })))
        })
    }
}