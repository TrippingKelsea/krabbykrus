//! Built-in tool implementations

use crate::message::ToolResult;
use crate::{Result, Tool, ToolExecutionContext};
use regex::Regex;
use rockbot_security::Capabilities;
use serde_json::json;
use std::future::Future;
use std::io::BufRead;
use std::net::IpAddr;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, PartialEq, Eq)]
struct DetectedCommand {
    program: String,
    args: Vec<String>,
}

const MAX_EXEC_TIMEOUT_SECS: u64 = 600;
const EXEC_OUTPUT_MAX_CHARS_DEFAULT: usize = 30_000;
const EXEC_OUTPUT_MAX_CHARS_UPPER_LIMIT: usize = 150_000;
const READ_MAX_FILE_BYTES_DEFAULT: u64 = 512 * 1024;
const READ_MAX_FILE_BYTES_UPPER_LIMIT: u64 = 8 * 1024 * 1024;
const READ_BINARY_SAMPLE_BYTES: usize = 8192;
const BLOCKED_DEVICE_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/tty",
    "/dev/console",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

impl std::fmt::Display for DetectedCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.args.is_empty() {
            write!(f, "{}", self.program)
        } else {
            write!(f, "{} {}", self.program, self.args.join(" "))
        }
    }
}

impl PartialEq<&str> for DetectedCommand {
    fn eq(&self, other: &&str) -> bool {
        self.to_string() == *other
    }
}

fn validate_runner_filter(filter: &str) -> Result<()> {
    if filter.is_empty() {
        return Err(crate::ToolError::InvalidParameters {
            message: "filter cannot be empty".to_string(),
        });
    }

    if !filter
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return Err(crate::ToolError::InvalidParameters {
            message: "filter contains unsupported characters".to_string(),
        });
    }

    Ok(())
}

fn get_string_param<'a>(params: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(|v| v.as_str()))
}

fn parse_command_line(command: &str) -> Result<DetectedCommand> {
    let parts = shell_words::split(command).map_err(|e| crate::ToolError::InvalidParameters {
        message: format!("invalid command syntax: {e}"),
    })?;

    let Some((program, args)) = parts.split_first() else {
        return Err(crate::ToolError::InvalidParameters {
            message: "command cannot be empty".to_string(),
        });
    };

    Ok(DetectedCommand {
        program: program.clone(),
        args: args.to_vec(),
    })
}

fn sanitize_shell_command(command: &str) -> Result<DetectedCommand> {
    if command.trim().is_empty() {
        return Err(crate::ToolError::InvalidParameters {
            message: "command cannot be empty".to_string(),
        });
    }

    if command
        .chars()
        .any(|c| c == '\0' || c == '\n' || c == '\r' || (c.is_control() && c != '\t'))
    {
        return Err(crate::ToolError::InvalidParameters {
            message: "command contains unsupported control characters".to_string(),
        });
    }

    parse_command_line(command)
}

fn get_bounded_env_usize(name: &str, default: usize, upper_limit: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=upper_limit).contains(value))
        .unwrap_or(default)
}

fn get_bounded_env_u64(name: &str, default: u64, upper_limit: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=upper_limit).contains(value))
        .unwrap_or(default)
}

fn get_exec_output_max_chars() -> usize {
    get_bounded_env_usize(
        "ROCKBOT_EXEC_MAX_OUTPUT_CHARS",
        EXEC_OUTPUT_MAX_CHARS_DEFAULT,
        EXEC_OUTPUT_MAX_CHARS_UPPER_LIMIT,
    )
}

fn get_read_max_file_bytes() -> u64 {
    get_bounded_env_u64(
        "ROCKBOT_READ_MAX_FILE_BYTES",
        READ_MAX_FILE_BYTES_DEFAULT,
        READ_MAX_FILE_BYTES_UPPER_LIMIT,
    )
}

fn truncate_utf8_chars(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn truncate_stream_output(text: &str, max_chars: usize, label: &str) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }

    let truncated = truncate_utf8_chars(text, max_chars);
    format!("{truncated}\n...[{label} truncated at {max_chars} of {total_chars} chars]")
}

fn truncate_combined_output(text: &str, max_chars: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.to_string();
    }

    let truncated = truncate_utf8_chars(text, max_chars);
    format!("{truncated}\n...[output truncated at {max_chars} of {total_chars} chars]")
}

fn is_blocked_device_path(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    BLOCKED_DEVICE_PATHS
        .iter()
        .any(|blocked| *blocked == path_str)
        || (path_str.starts_with("/proc/")
            && (path_str.ends_with("/fd/0")
                || path_str.ends_with("/fd/1")
                || path_str.ends_with("/fd/2")))
}

async fn ensure_safe_text_readable(path: &std::path::Path, limit: Option<usize>) -> Result<()> {
    let metadata =
        tokio::fs::metadata(path)
            .await
            .map_err(|e| crate::ToolError::ExecutionFailed {
                message: format!("Failed to stat file: {e}"),
            })?;

    if !metadata.is_file() {
        return Err(crate::ToolError::ExecutionFailed {
            message: format!("{} is not a regular file", path.display()),
        });
    }

    if is_blocked_device_path(path) {
        return Err(crate::ToolError::ExecutionFailed {
            message: format!("Refusing to read blocked device path {}", path.display()),
        });
    }

    let max_bytes = get_read_max_file_bytes();
    if metadata.len() > max_bytes && limit.is_none() {
        return Err(crate::ToolError::ExecutionFailed {
            message: format!(
                "File {} is too large to read in full ({} bytes > {} bytes). Use offset/limit to read a smaller window.",
                path.display(),
                metadata.len(),
                max_bytes
            ),
        });
    }

    let mut file =
        tokio::fs::File::open(path)
            .await
            .map_err(|e| crate::ToolError::ExecutionFailed {
                message: format!("Failed to open file: {e}"),
            })?;
    let mut sample = vec![0_u8; READ_BINARY_SAMPLE_BYTES];
    let bytes_read =
        file.read(&mut sample)
            .await
            .map_err(|e| crate::ToolError::ExecutionFailed {
                message: format!("Failed to read file sample: {e}"),
            })?;
    sample.truncate(bytes_read);
    if sample.contains(&0) {
        return Err(crate::ToolError::ExecutionFailed {
            message: format!(
                "Refusing to read binary file {}. Use a more specific tool for binary content.",
                path.display()
            ),
        });
    }

    Ok(())
}

async fn read_text_window(
    path: &std::path::Path,
    offset: usize,
    limit: Option<usize>,
) -> Result<String> {
    ensure_safe_text_readable(path, limit).await?;

    if limit.is_none() {
        return tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::ToolError::ExecutionFailed {
                message: format!("Failed to read file: {e}"),
            }
        });
    }

    let file =
        tokio::fs::File::open(path)
            .await
            .map_err(|e| crate::ToolError::ExecutionFailed {
                message: format!("Failed to open file: {e}"),
            })?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let start = offset.saturating_sub(1);
    let max_lines = limit.unwrap_or(usize::MAX);
    let mut current = 0usize;
    let mut results = Vec::new();

    while let Some(line) =
        lines
            .next_line()
            .await
            .map_err(|e| crate::ToolError::ExecutionFailed {
                message: format!("Failed to read file: {e}"),
            })?
    {
        if current >= start {
            results.push(line);
            if results.len() >= max_lines {
                break;
            }
        }
        current += 1;
    }

    Ok(results.join("\n"))
}

fn normalized_whitespace_match(haystack: &str, needle: &str) -> Option<String> {
    let norm_needle = normalize_whitespace(needle);
    let norm_haystack = normalize_whitespace(haystack);
    if !norm_haystack.contains(&norm_needle) {
        return None;
    }
    find_original_span(haystack, needle)
}

pub(crate) fn is_safe_read_only_exec_command(command: &str) -> bool {
    let Ok(parsed) = parse_command_line(command) else {
        return false;
    };

    match parsed.program.as_str() {
        "pwd" => parsed.args.is_empty(),
        "git" => is_safe_read_only_git_command(&parsed.args),
        "ls" => args_are_flags_or_safe_paths(&parsed.args),
        "cat" | "head" | "tail" | "wc" | "stat" => args_are_flags_or_safe_paths(&parsed.args),
        "find" => is_safe_read_only_find_command(&parsed.args),
        _ => false,
    }
}

fn is_safe_read_only_git_command(args: &[String]) -> bool {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return false;
    };

    match subcommand {
        "status" | "diff" | "show" | "log" | "rev-parse" | "ls-files" => true,
        "branch" => args.iter().skip(1).all(|arg| {
            matches!(
                arg.as_str(),
                "--show-current" | "--all" | "-a" | "--remotes" | "-r" | "--list" | "-vv" | "-v"
            )
        }),
        _ => false,
    }
}

fn args_are_flags_or_safe_paths(args: &[String]) -> bool {
    args.iter()
        .all(|arg| arg.starts_with('-') || is_safe_workspace_relative_arg(arg))
}

fn is_safe_workspace_relative_arg(arg: &str) -> bool {
    !arg.is_empty()
        && !arg.starts_with('/')
        && !arg.starts_with('~')
        && !arg.contains("..")
        && !arg.contains('\0')
}

fn is_safe_read_only_find_command(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }

    let dangerous_terms = [
        "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprintf", "-fls",
    ];
    if args
        .iter()
        .any(|arg| dangerous_terms.iter().any(|term| arg == term))
    {
        return false;
    }

    args.iter().all(|arg| {
        if arg.starts_with('-') {
            true
        } else {
            is_safe_workspace_relative_arg(arg)
        }
    })
}

fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn canonicalize_existing_ancestor(path: &std::path::Path) -> std::io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    let mut suffix = Vec::new();

    loop {
        if current.exists() {
            let mut resolved = std::fs::canonicalize(&current)?;
            for component in suffix.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }

        let Some(name) = current.file_name().map(|name| name.to_os_string()) else {
            return std::fs::canonicalize(&current);
        };
        suffix.push(name);

        if !current.pop() {
            return std::fs::canonicalize(path);
        }
    }
}

fn resolve_workspace_path(workspace_path: &std::path::Path, raw_path: &str) -> Result<PathBuf> {
    let workspace = canonicalize_existing_ancestor(workspace_path).map_err(|e| {
        crate::ToolError::InvalidParameters {
            message: format!(
                "failed to resolve workspace '{}': {e}",
                workspace_path.display()
            ),
        }
    })?;
    let candidate = if PathBuf::from(raw_path).is_absolute() {
        normalize_path(std::path::Path::new(raw_path))
    } else {
        normalize_path(&workspace.join(raw_path))
    };
    let candidate = canonicalize_existing_ancestor(&candidate).map_err(|e| {
        crate::ToolError::InvalidParameters {
            message: format!("failed to resolve path '{raw_path}': {e}"),
        }
    })?;

    if !candidate.starts_with(&workspace) {
        return Err(crate::ToolError::InvalidParameters {
            message: format!("path '{raw_path}' escapes the workspace"),
        });
    }

    Ok(candidate)
}

fn resolve_tool_path(
    context: &ToolExecutionContext,
    params: &serde_json::Value,
    keys: &[&str],
) -> Result<PathBuf> {
    let raw_path = get_string_param(params, keys)
        .ok_or_else(|| crate::ToolError::InvalidParameters {
            message: format!("{} is required", keys[0]),
        })?
        .to_string()
        .trim()
        .to_string();

    resolve_workspace_path(&context.workspace_path, &raw_path)
}

/// File reading tool
pub struct ReadTool;

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let limit = params
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .map(|v| v as usize);
            let offset = params
                .get("offset")
                .and_then(serde_json::Value::as_u64)
                .map_or(1, |v| v as usize);

            // Resolve path relative to workspace
            let path = resolve_tool_path(&context, &params, &["file_path", "path", "file"])?;

            let result_content = read_text_window(&path, offset, limit).await?;
            Ok(ToolResult::text(result_content))
        })
    }
}

/// File writing tool
pub struct WriteTool;

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let path = resolve_tool_path(&context, &params, &["file_path", "path", "file"])?;

            let content: String = get_string_param(&params, &["content", "text"])
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "content is required".to_string(),
                })?
                .to_string();

            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    crate::ToolError::ExecutionFailed {
                        message: format!("Failed to create directories: {e}"),
                    }
                })?;
            }

            // Write content to file
            tokio::fs::write(&path, content.as_bytes())
                .await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to write file: {e}"),
                })?;

            let bytes_written = content.len();
            Ok(ToolResult::text(format!(
                "Successfully wrote {} bytes to {}",
                bytes_written,
                path.display()
            )))
        })
    }
}

/// File editing tool
pub struct EditTool;

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

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
        "Edit a file by replacing text. Requires the old_text to be unique in the file unless replace_all is set to true."
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
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "When true, replace all occurrences of old_text. Default is false, which requires old_text to appear exactly once."
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

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let path = resolve_tool_path(&context, &params, &["file_path", "path", "file"])?;

            let old_text: String = params
                .get("old_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "old_text is required".to_string(),
                })?
                .to_string();

            let new_text: String = params
                .get("new_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "new_text is required".to_string(),
                })?
                .to_string();

            // Read current content
            let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read file: {e}"),
                }
            })?;

            // Count occurrences and enforce uniqueness
            let count = content.matches(old_text.as_str()).count();

            if count == 0 {
                if let Some(matched_text) = normalized_whitespace_match(&content, &old_text) {
                    let new_content = content.replacen(&matched_text, &new_text, 1);
                    tokio::fs::write(&path, new_content.as_bytes())
                        .await
                        .map_err(|e| crate::ToolError::ExecutionFailed {
                            message: format!("Failed to write file: {e}"),
                        })?;
                    return Ok(ToolResult::text(format!(
                        "Edited {} (normalized whitespace match)",
                        path.display()
                    )));
                }

                return Ok(ToolResult::error(format!(
                    "old_text not found in {}. The file may have changed; re-read the file and retry with more exact surrounding context.",
                    path.display()
                )));
            }

            let replace_all = params
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if count > 1 && !replace_all {
                return Ok(ToolResult::error(format!(
                    "old_text appears {count} times in the file. Provide more surrounding context to make it unique, or set replace_all=true to replace all occurrences."
                )));
            }

            let new_content = if replace_all {
                content.replace(old_text.as_str(), &new_text)
            } else {
                content.replacen(old_text.as_str(), &new_text, 1)
            };

            // Write updated content
            tokio::fs::write(&path, new_content.as_bytes())
                .await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to write file: {e}"),
                })?;

            Ok(ToolResult::text(format!(
                "Successfully replaced text in {}",
                path.display()
            )))
        })
    }
}

/// Command execution tool
pub struct ExecTool;

impl Default for ExecTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let command: String = params
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "command is required".to_string(),
                })?
                .to_string();

            let workdir = params.get("workdir").and_then(|v| v.as_str()).map_or_else(
                || Ok(context.workspace_path.clone()),
                |p| resolve_workspace_path(&context.workspace_path, p),
            )?;

            let timeout = params
                .get("timeout")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(30)
                .min(MAX_EXEC_TIMEOUT_SECS); // Default 30 second timeout

            let detected = sanitize_shell_command(&command)?;

            let mut cmd = Command::new(&detected.program);
            cmd.args(&detected.args).current_dir(&workdir);

            let output =
                tokio::time::timeout(std::time::Duration::from_secs(timeout), cmd.output())
                    .await
                    .map_err(|_| crate::ToolError::ExecutionFailed {
                        message: "Command timed out".to_string(),
                    })?
                    .map_err(|e| crate::ToolError::ExecutionFailed {
                        message: format!("Failed to execute command: {e}"),
                    })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let max_output_chars = get_exec_output_max_chars();
            let exit_code = output.status.code().unwrap_or(-1);

            let result = json!({
                "exit_code": exit_code,
                "stdout": truncate_stream_output(&stdout, max_output_chars, "stdout"),
                "stderr": truncate_stream_output(&stderr, max_output_chars, "stderr"),
                "success": output.status.success()
            });

            Ok(ToolResult::json(result))
        })
    }
}

/// File glob pattern matching tool
pub struct GlobTool;

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let pattern: String = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "pattern is required".to_string(),
                })?
                .to_string();

            let base_dir = params.get("path").and_then(|v| v.as_str()).map_or_else(
                || Ok(context.workspace_path.clone()),
                |p| resolve_workspace_path(&context.workspace_path, p),
            )?;

            // Build full glob pattern
            if PathBuf::from(&pattern).is_absolute() {
                return Err(crate::ToolError::InvalidParameters {
                    message: "absolute glob patterns are not allowed".to_string(),
                });
            }
            let full_pattern = format!("{}/{}", base_dir.display(), pattern);

            let entries =
                glob::glob(&full_pattern).map_err(|e| crate::ToolError::InvalidParameters {
                    message: format!("Invalid glob pattern: {e}"),
                })?;

            let mut matches: Vec<String> = Vec::new();
            for entry in entries {
                match entry {
                    Ok(path) => {
                        // Return paths relative to base_dir when possible
                        let display_path = path.strip_prefix(&base_dir).map_or_else(
                            |_| path.display().to_string(),
                            |p| p.display().to_string(),
                        );
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

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let pattern_str: String = params
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "pattern is required".to_string(),
                })?
                .to_string();

            let search_path = params.get("path").and_then(|v| v.as_str()).map_or_else(
                || Ok(context.workspace_path.clone()),
                |p| resolve_workspace_path(&context.workspace_path, p),
            )?;

            let include_pattern = params
                .get("include")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string);

            let max_results = params
                .get("max_results")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(100) as usize;

            let regex =
                Regex::new(&pattern_str).map_err(|e| crate::ToolError::InvalidParameters {
                    message: format!("Invalid regex: {e}"),
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
                        message: format!("Glob error: {e}"),
                    })?
                    .filter_map(std::result::Result::ok)
                    .filter(|p| p.is_file())
                    .collect()
            };

            'outer: for file_path in files {
                // Skip binary files
                let Ok(file) = std::fs::File::open(&file_path) else {
                    continue;
                };
                let reader = std::io::BufReader::new(file);

                for (line_num, line) in reader.lines().enumerate() {
                    let Ok(line) = line else {
                        continue;
                    };

                    if regex.is_match(&line) {
                        let display_path =
                            file_path.strip_prefix(&context.workspace_path).map_or_else(
                                |_| file_path.display().to_string(),
                                |p| p.display().to_string(),
                            );

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

impl Default for PatchTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let patch: String = params
                .get("patch")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "patch is required".to_string(),
                })?
                .to_string();

            let path = resolve_tool_path(&context, &params, &["file_path", "path", "file"])?;

            // Read existing file content
            let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read file: {e}"),
                }
            })?;

            let mut lines: Vec<String> = content
                .lines()
                .map(std::string::ToString::to_string)
                .collect();
            let mut hunks_applied = 0;
            let mut offset: i64 = 0;

            // Parse unified diff hunks
            #[allow(clippy::unwrap_used)] // static pattern, can never fail to compile
            let hunk_re = Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();

            let patch_lines: Vec<&str> = patch.lines().collect();
            let mut i = 0;

            while i < patch_lines.len() {
                if let Some(caps) = hunk_re.captures(patch_lines[i]) {
                    #[allow(clippy::unwrap_used)]
                    // group 1 is non-optional in the regex, always present when caps exists
                    let orig_start: i64 = caps.get(1).unwrap().as_str().parse().unwrap_or(1);

                    i += 1;

                    let mut removals: Vec<usize> = Vec::new();
                    let mut additions: Vec<(usize, String)> = Vec::new();
                    let mut pos = ((orig_start - 1) + offset) as usize;

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
                    let add_offset = removals
                        .iter()
                        .filter(|&&r| r <= additions.first().map_or(usize::MAX, |a| a.0))
                        .count();
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
            tokio::fs::write(&path, new_content.as_bytes())
                .await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to write patched file: {e}"),
                })?;

            Ok(ToolResult::text(format!(
                "Successfully applied {} hunk(s) to {}",
                hunks_applied,
                path.display()
            )))
        })
    }
}

/// Memory retrieval tool
pub struct MemoryGetTool;

impl Default for MemoryGetTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let key: String = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "key is required".to_string(),
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

            let content = tokio::fs::read_to_string(&memory_path).await.map_err(|e| {
                crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read memory: {e}"),
                }
            })?;

            let memory: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to parse memory: {e}"),
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

impl Default for MemorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

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

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let query: String = params
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "query is required".to_string(),
                })?
                .to_string();

            let limit = params
                .get("limit")
                .and_then(serde_json::Value::as_u64)
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
            let entries =
                glob::glob(&glob_pattern).map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Glob error: {e}"),
                })?;

            for entry in entries
                .filter_map(std::result::Result::ok)
                .filter(|p| p.is_file())
            {
                let Ok(content) = std::fs::read_to_string(&entry) else {
                    continue;
                };

                let content_lower = content.to_lowercase();

                // Score by number of matching terms
                let score: usize = query_terms
                    .iter()
                    .filter(|term| content_lower.contains(*term))
                    .count();

                if score > 0 {
                    let display_path = entry
                        .strip_prefix(&memory_dir)
                        .map_or_else(|_| entry.display().to_string(), |p| p.display().to_string());

                    // Extract relevant snippet
                    let snippet = content
                        .lines()
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
                b.get("score")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    .cmp(
                        &a.get("score")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                    )
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

/// Tool for delegating work to another agent (subagent pattern)
pub struct InvokeAgentTool;

impl Default for InvokeAgentTool {
    fn default() -> Self {
        Self::new()
    }
}

impl InvokeAgentTool {
    pub fn new() -> Self {
        Self
    }
}

/// Maximum delegation depth to prevent infinite recursion
const MAX_DELEGATION_DEPTH: u32 = 3;

impl Tool for InvokeAgentTool {
    fn name(&self) -> &str {
        "invoke_agent"
    }

    fn description(&self) -> &str {
        "Delegate a task to another agent by ID. The target agent processes the message and returns its response."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The ID of the agent to invoke"
                },
                "message": {
                    "type": "string",
                    "description": "The task to send to the target agent"
                },
                "context": {
                    "type": "string",
                    "description": "Optional background context to help the sub-agent (e.g. relevant file paths, prior findings). Keep this focused — only include what the sub-agent needs."
                }
            },
            "required": ["agent_id", "message"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::new()
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let agent_id: String = params
                .get("agent_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "agent_id is required".to_string(),
                })?
                .to_string();

            let raw_message: String = params
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "message is required".to_string(),
                })?
                .to_string();

            // Build the scoped message for the sub-agent
            let sub_context = params.get("context").and_then(|v| v.as_str()).unwrap_or("");

            let message = if sub_context.is_empty() {
                raw_message
            } else {
                format!(
                    "Context from parent agent ({}):\n{sub_context}\n\nTask:\n{raw_message}",
                    context.agent_id
                )
            };

            // Prevent self-delegation
            if agent_id == context.agent_id {
                return Ok(ToolResult::error(
                    "Cannot invoke self — would cause infinite recursion",
                ));
            }

            // Check delegation depth
            if context.delegation_depth >= MAX_DELEGATION_DEPTH {
                return Ok(ToolResult::error(format!(
                    "Maximum delegation depth ({MAX_DELEGATION_DEPTH}) exceeded. Cannot delegate further."
                )));
            }

            // Check if we have an agent invoker
            let invoker = match &context.agent_invoker {
                Some(inv) => inv.clone(),
                None => {
                    return Ok(ToolResult::error(
                        "Agent delegation is not available in this context",
                    ));
                }
            };

            // Timeout sub-agent invocation to prevent indefinite hangs
            let invoke_timeout = std::time::Duration::from_secs(300);
            match tokio::time::timeout(
                invoke_timeout,
                invoker.invoke_agent(
                    &agent_id,
                    &message,
                    &context.session_id,
                    context.delegation_depth + 1,
                ),
            )
            .await
            {
                Ok(Ok(response)) => Ok(ToolResult::text(response)),
                Ok(Err(e)) => Ok(ToolResult::error(format!("Agent invocation failed: {e}"))),
                Err(_) => Ok(ToolResult::error(format!(
                    "Agent invocation timed out after {}s",
                    invoke_timeout.as_secs()
                ))),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Handoff tool — transfers conversation control to another agent
// ---------------------------------------------------------------------------

/// Handoff tool — transfers conversation control to another agent.
///
/// Unlike `invoke_agent` (which delegates and returns), `handoff` transfers
/// the *entire conversation* to the target agent. The current agent's turn
/// ends immediately and the target agent takes over.
pub struct HandoffTool;

impl Default for HandoffTool {
    fn default() -> Self {
        Self::new()
    }
}

impl HandoffTool {
    pub fn new() -> Self {
        Self
    }
}

/// Maximum delegation depth for handoffs (same as invoke_agent)
const MAX_HANDOFF_DEPTH: u32 = 3;

impl Tool for HandoffTool {
    fn name(&self) -> &str {
        "handoff"
    }

    fn description(&self) -> &str {
        "Transfer conversation control to another agent. Unlike invoke_agent (which delegates a subtask and returns the result), handoff completely transfers the conversation — the current agent's turn ends and the target agent takes over."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "target_agent_id": {
                    "type": "string",
                    "description": "ID of the agent to hand off to"
                },
                "context": {
                    "type": "string",
                    "description": "Context/instructions to pass to the target agent explaining why the handoff is happening and what they should do"
                },
                "message": {
                    "type": "string",
                    "description": "Optional message override — if provided, the target agent receives this instead of the original user message"
                }
            },
            "required": ["target_agent_id", "context"]
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
            let target_agent_id = params
                .get("target_agent_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "target_agent_id is required".to_string(),
                })?
                .to_string();

            let handoff_context = params
                .get("context")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "context is required".to_string(),
                })?
                .to_string();

            let message_override = params
                .get("message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Prevent self-handoff
            if target_agent_id == context.agent_id {
                return Ok(ToolResult::error(
                    "Cannot hand off to self — would cause infinite loop",
                ));
            }

            // Check delegation depth
            if context.delegation_depth >= MAX_HANDOFF_DEPTH {
                return Ok(ToolResult::error(format!(
                    "Maximum handoff depth ({MAX_HANDOFF_DEPTH}) exceeded. Cannot hand off further."
                )));
            }

            Ok(ToolResult::Handoff {
                target_agent_id,
                context: handoff_context,
                message_override,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Blackboard tools — shared state for swarm coordination
// ---------------------------------------------------------------------------

/// Blackboard read tool — read shared state from the swarm blackboard.
pub struct BlackboardReadTool;

impl Default for BlackboardReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BlackboardReadTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for BlackboardReadTool {
    fn name(&self) -> &str {
        "blackboard_read"
    }

    fn description(&self) -> &str {
        "Read from the shared swarm blackboard. Omit 'key' to read all entries. Only available to agents that belong to a swarm."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key to read. Omit to read all entries."
                }
            }
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
            let swarm_id = match &context.swarm_id {
                Some(id) => id.clone(),
                None => return Ok(ToolResult::error(
                    "This agent is not part of a swarm. blackboard_read is only available to swarm members."
                )),
            };

            let blackboard = match &context.blackboard {
                Some(bb) => bb.clone(),
                None => return Ok(ToolResult::error("No blackboard available in this context")),
            };

            let key = params.get("key").and_then(|v| v.as_str());

            if let Some(key) = key {
                match blackboard.read(&swarm_id, key).await {
                    Some(value) => Ok(ToolResult::json(serde_json::json!({
                        "key": key,
                        "value": value,
                    }))),
                    None => Ok(ToolResult::json(serde_json::json!({
                        "key": key,
                        "value": null,
                        "note": "Key not found on blackboard"
                    }))),
                }
            } else {
                let all = blackboard.read_all(&swarm_id).await;
                Ok(ToolResult::json(serde_json::json!(all)))
            }
        })
    }
}

/// Blackboard write tool — write shared state to the swarm blackboard.
pub struct BlackboardWriteTool;

impl Default for BlackboardWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BlackboardWriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for BlackboardWriteTool {
    fn name(&self) -> &str {
        "blackboard_write"
    }

    fn description(&self) -> &str {
        "Write a key-value pair to the shared swarm blackboard. Only available to agents that belong to a swarm."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key to write"
                },
                "value": {
                    "description": "The value to store (any JSON type)"
                }
            },
            "required": ["key", "value"]
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
            let swarm_id = match &context.swarm_id {
                Some(id) => id.clone(),
                None => return Ok(ToolResult::error(
                    "This agent is not part of a swarm. blackboard_write is only available to swarm members."
                )),
            };

            let blackboard = match &context.blackboard {
                Some(bb) => bb.clone(),
                None => return Ok(ToolResult::error("No blackboard available in this context")),
            };

            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "key is required".to_string(),
                })?
                .to_string();

            let value = params.get("value").cloned().ok_or_else(|| {
                crate::ToolError::InvalidParameters {
                    message: "value is required".to_string(),
                }
            })?;

            blackboard.write(&swarm_id, &key, value.clone()).await;

            Ok(ToolResult::json(serde_json::json!({
                "status": "ok",
                "key": key,
                "swarm_id": swarm_id,
            })))
        })
    }
}

/// Maximum response body size for web_fetch (256 KB)
const WEB_FETCH_MAX_BODY: usize = 256 * 1024;

fn is_private_or_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.octets() == [169, 254, 169, 254]
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
        }
    }
}

async fn validate_web_fetch_url(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).map_err(|e| crate::ToolError::InvalidParameters {
        message: format!("Invalid URL: {e}"),
    })?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(crate::ToolError::InvalidParameters {
                message: format!("Unsupported URL scheme: {other}"),
            })
        }
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| crate::ToolError::InvalidParameters {
            message: "URL host is required".to_string(),
        })?;

    if matches!(host, "localhost" | "localhost.localdomain")
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return Err(crate::ToolError::InvalidParameters {
            message: "web_fetch does not allow localhost or internal hostnames".to_string(),
        });
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_or_local_ip(ip) {
            return Err(crate::ToolError::InvalidParameters {
                message: "web_fetch does not allow private, loopback, or link-local addresses"
                    .to_string(),
            });
        }
        return Ok(parsed);
    }

    let port = parsed.port_or_known_default().unwrap_or(80);
    let resolved = tokio::net::lookup_host((host, port)).await.map_err(|e| {
        crate::ToolError::ExecutionFailed {
            message: format!("Failed to resolve host '{host}': {e}"),
        }
    })?;
    for socket_addr in resolved {
        if is_private_or_local_ip(socket_addr.ip()) {
            return Err(crate::ToolError::InvalidParameters {
                message: "web_fetch target resolves to a private, loopback, or link-local address"
                    .to_string(),
            });
        }
    }

    Ok(parsed)
}

/// Web fetch tool — HTTP GET with text extraction
pub struct WebFetchTool;

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the contents of a URL via HTTP GET. Returns the response body as text, with HTML tags stripped."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Request timeout in seconds (default: 15)",
                    "minimum": 1,
                    "maximum": 60
                },
                "raw": {
                    "type": "boolean",
                    "description": "If true, return raw HTML without stripping tags (default: false)"
                }
            },
            "required": ["url"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new();
        caps.add(rockbot_security::Capability::NetworkAccess("*".to_string()));
        caps
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "url is required".to_string(),
                })?
                .to_string();
            let parsed_url = validate_web_fetch_url(&url).await?;

            let timeout_secs = params
                .get("timeout_secs")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(15);

            let raw = params
                .get("raw")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(timeout_secs))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to create HTTP client: {e}"),
                })?;

            let response = client.get(parsed_url).send().await.map_err(|e| {
                crate::ToolError::ExecutionFailed {
                    message: format!("HTTP request failed: {e}"),
                }
            })?;

            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown")
                .to_string();

            let body = response
                .text()
                .await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to read response body: {e}"),
                })?;

            // Truncate if too large
            let body = if body.len() > WEB_FETCH_MAX_BODY {
                format!(
                    "{}...\n[truncated at {} bytes]",
                    &body[..WEB_FETCH_MAX_BODY],
                    WEB_FETCH_MAX_BODY
                )
            } else {
                body
            };

            // Strip HTML tags if not raw and content is HTML
            let body = if !raw && content_type.contains("html") {
                strip_html_tags(&body)
            } else {
                body
            };

            Ok(ToolResult::json(json!({
                "url": url,
                "status": status.as_u16(),
                "content_type": content_type,
                "body": body,
            })))
        })
    }
}

/// Simple HTML tag stripper — removes tags and collapses whitespace
pub(crate) fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut last_was_space = false;

    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_was_space {
                    result.push(' ');
                    last_was_space = true;
                }
            }
            _ if !in_tag => {
                if ch.is_whitespace() {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                } else {
                    result.push(ch);
                    last_was_space = false;
                }
            }
            _ => {}
        }
    }

    result.trim().to_string()
}

/// Web search tool — configurable search provider
pub struct WebSearchTool;

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web using a configured search provider. Returns search results with titles, URLs, and snippets."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "max_results": {
                    "type": "number",
                    "description": "Maximum number of results to return (default: 5)",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new();
        caps.add(rockbot_security::Capability::NetworkAccess("*".to_string()));
        caps
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let query = params
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "query is required".to_string(),
                })?
                .to_string();

            let max_results = params
                .get("max_results")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(5) as usize;

            // Try to get search API credentials
            let api_key = if let Some(ref accessor) = context.credential_accessor {
                match accessor
                    .get_credential("rockbot://web_search/api_key", &context.agent_id)
                    .await
                {
                    Ok(crate::CredentialResult::Granted { secret, .. }) => {
                        String::from_utf8(secret).ok()
                    }
                    _ => None,
                }
            } else {
                None
            };

            if api_key.is_none() {
                return Ok(ToolResult::json(json!({
                    "error": "Web search is not configured. Set up a search API credential (Brave Search, SerpAPI, etc.) to enable web search.",
                    "query": query,
                    "results": [],
                })));
            }

            // Use Brave Search API if key is available
            let api_key = api_key.unwrap_or_default();
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Failed to create HTTP client: {e}"),
                })?;

            let resp = client
                .get("https://api.search.brave.com/res/v1/web/search")
                .header("X-Subscription-Token", &api_key)
                .header("Accept", "application/json")
                .query(&[("q", &query), ("count", &max_results.to_string())])
                .send()
                .await
                .map_err(|e| crate::ToolError::ExecutionFailed {
                    message: format!("Search request failed: {e}"),
                })?;

            let body: serde_json::Value =
                resp.json()
                    .await
                    .map_err(|e| crate::ToolError::ExecutionFailed {
                        message: format!("Failed to parse search response: {e}"),
                    })?;

            // Extract web results from Brave Search response
            let results: Vec<serde_json::Value> = body
                .get("web")
                .and_then(|w| w.get("results"))
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .take(max_results)
                        .map(|r| {
                            json!({
                                "title": r.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                                "url": r.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                                "snippet": r.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            Ok(ToolResult::json(json!({
                "query": query,
                "results": results,
                "count": results.len(),
            })))
        })
    }
}

// ---------------------------------------------------------------------------
// TestTool — auto-detect project type and run tests
// ---------------------------------------------------------------------------

/// Run the project's test suite, auto-detecting project type (Cargo, npm, pytest, Go, Make).
pub struct TestTool;

impl Default for TestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TestTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for TestTool {
    fn name(&self) -> &str {
        "test"
    }

    fn description(&self) -> &str {
        "Run the project's test suite. Auto-detects the project type (Cargo, npm/Jest, pytest, Go, Make) and runs the appropriate test command."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional test name filter (passed to the test runner)"
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional working directory relative to workspace root"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)"
                }
            }
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::process_execute()
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let filter = params
                .get("filter")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let workdir = params.get("workdir").and_then(|v| v.as_str()).map_or_else(
                || Ok(context.workspace_path.clone()),
                |p| resolve_workspace_path(&context.workspace_path, p),
            )?;

            let timeout_secs = params
                .get("timeout")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(120)
                .min(MAX_EXEC_TIMEOUT_SECS);

            let cmd = detect_test_command(&workdir, filter.as_deref()).await?;

            let mut child = Command::new(&cmd.program);
            child.args(&cmd.args).current_dir(&workdir);

            let output =
                tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.output())
                    .await
                    .map_err(|_| crate::ToolError::ExecutionFailed {
                        message: format!("Test command timed out after {timeout_secs}s"),
                    })?
                    .map_err(|e| crate::ToolError::ExecutionFailed {
                        message: format!("Failed to run test command: {e}"),
                    })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined =
                truncate_combined_output(&format!("{stdout}{stderr}"), get_exec_output_max_chars());
            let exit_code = output.status.code().unwrap_or(-1);

            if output.status.success() {
                Ok(ToolResult::text(format!(
                    "Tests passed (exit {exit_code}).\n\n{combined}"
                )))
            } else {
                Ok(ToolResult::error(format!(
                    "Tests failed (exit {exit_code}).\n\n{combined}"
                )))
            }
        })
    }
}

/// Detect which test command to run based on files present in `workdir`.
async fn detect_test_command(
    workdir: &std::path::Path,
    filter: Option<&str>,
) -> Result<DetectedCommand> {
    if workdir.join("Cargo.toml").exists() {
        let mut args = vec!["test".to_string()];
        if let Some(f) = filter {
            validate_runner_filter(f)?;
            args.push("--".to_string());
            args.push(f.to_string());
        }
        return Ok(DetectedCommand {
            program: "cargo".to_string(),
            args,
        });
    }

    if workdir.join("package.json").exists() {
        if let Some(f) = filter {
            validate_runner_filter(f)?;
            return Ok(DetectedCommand {
                program: "npx".to_string(),
                args: vec!["jest".to_string(), f.to_string()],
            });
        }
        return Ok(DetectedCommand {
            program: "npm".to_string(),
            args: vec!["test".to_string()],
        });
    }

    if workdir.join("pyproject.toml").exists() || workdir.join("setup.py").exists() {
        let mut args = Vec::new();
        if let Some(f) = filter {
            validate_runner_filter(f)?;
            args.push(f.to_string());
        }
        return Ok(DetectedCommand {
            program: "pytest".to_string(),
            args,
        });
    }

    if workdir.join("go.mod").exists() {
        let mut args = vec!["test".to_string(), "./...".to_string()];
        if let Some(f) = filter {
            validate_runner_filter(f)?;
            args.push("-run".to_string());
            args.push(f.to_string());
        }
        return Ok(DetectedCommand {
            program: "go".to_string(),
            args,
        });
    }

    // Makefile with a test target
    let makefile = workdir.join("Makefile");
    if makefile.exists() {
        let contents = tokio::fs::read_to_string(&makefile)
            .await
            .unwrap_or_default();
        if contents.contains("\ntest:") || contents.starts_with("test:") {
            return Ok(DetectedCommand {
                program: "make".to_string(),
                args: vec!["test".to_string()],
            });
        }
    }

    Err(crate::ToolError::ExecutionFailed {
        message: "Could not detect project type. No Cargo.toml, package.json, pyproject.toml, setup.py, go.mod, or Makefile with test target found.".to_string(),
    })
}

// ---------------------------------------------------------------------------
// LintTool — auto-detect project type and run linter
// ---------------------------------------------------------------------------

/// Run the project's linter, auto-detecting project type (Cargo/Clippy, npm, ruff/flake8, golangci-lint).
pub struct LintTool;

impl Default for LintTool {
    fn default() -> Self {
        Self::new()
    }
}

impl LintTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for LintTool {
    fn name(&self) -> &str {
        "lint"
    }

    fn description(&self) -> &str {
        "Run the project's linter. Auto-detects the project type (Cargo/Clippy, npm, ruff/flake8, golangci-lint) and runs the appropriate lint command."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional path or pattern filter (passed to the lint runner)"
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional working directory relative to workspace root"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)"
                }
            }
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::process_execute()
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute(
        &self,
        params: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let filter = params
                .get("filter")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let workdir = params.get("workdir").and_then(|v| v.as_str()).map_or_else(
                || Ok(context.workspace_path.clone()),
                |p| resolve_workspace_path(&context.workspace_path, p),
            )?;

            let timeout_secs = params
                .get("timeout")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(120)
                .min(MAX_EXEC_TIMEOUT_SECS);

            let cmd = detect_lint_command(&workdir, filter.as_deref()).await?;

            let mut child = Command::new(&cmd.program);
            child.args(&cmd.args).current_dir(&workdir);

            let output =
                tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.output())
                    .await
                    .map_err(|_| crate::ToolError::ExecutionFailed {
                        message: format!("Lint command timed out after {timeout_secs}s"),
                    })?
                    .map_err(|e| crate::ToolError::ExecutionFailed {
                        message: format!("Failed to run lint command: {e}"),
                    })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined =
                truncate_combined_output(&format!("{stdout}{stderr}"), get_exec_output_max_chars());
            let exit_code = output.status.code().unwrap_or(-1);

            if output.status.success() {
                Ok(ToolResult::text(format!(
                    "Lint passed (exit {exit_code}).\n\n{combined}"
                )))
            } else {
                Ok(ToolResult::error(format!(
                    "Lint failed (exit {exit_code}).\n\n{combined}"
                )))
            }
        })
    }
}

/// Detect which lint command to run based on files present in `workdir`.
async fn detect_lint_command(
    workdir: &std::path::Path,
    filter: Option<&str>,
) -> Result<DetectedCommand> {
    if workdir.join("Cargo.toml").exists() {
        return Ok(DetectedCommand {
            program: "cargo".to_string(),
            args: vec![
                "clippy".to_string(),
                "--".to_string(),
                "-D".to_string(),
                "warnings".to_string(),
            ],
        });
    }

    if workdir.join("package.json").exists() {
        // Only run npm run lint if a lint script exists in package.json
        let pkg_contents = tokio::fs::read_to_string(workdir.join("package.json"))
            .await
            .unwrap_or_default();
        if pkg_contents.contains("\"lint\"") {
            return Ok(DetectedCommand {
                program: "npm".to_string(),
                args: vec!["run".to_string(), "lint".to_string()],
            });
        }
        return Err(crate::ToolError::ExecutionFailed {
            message: "package.json has no 'lint' script defined.".to_string(),
        });
    }

    if workdir.join("pyproject.toml").exists() || workdir.join("setup.py").exists() {
        // Prefer ruff, fall back to flake8
        let target = filter.unwrap_or(".");
        validate_runner_filter(target)?;
        let ruff_available = Command::new("sh")
            .arg("-c")
            .arg("command -v ruff")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ruff_available {
            return Ok(DetectedCommand {
                program: "ruff".to_string(),
                args: vec!["check".to_string(), target.to_string()],
            });
        }
        return Ok(DetectedCommand {
            program: "flake8".to_string(),
            args: vec![target.to_string()],
        });
    }

    if workdir.join("go.mod").exists() {
        return Ok(DetectedCommand {
            program: "golangci-lint".to_string(),
            args: vec!["run".to_string()],
        });
    }

    Err(crate::ToolError::ExecutionFailed {
        message: "Could not detect project type for linting. No Cargo.toml, package.json, pyproject.toml, setup.py, or go.mod found.".to_string(),
    })
}

/// Tool for asking the user a clarifying question.
///
/// When an agent is uncertain about how to proceed, it can use this tool
/// to pose a question back to the user. The tool returns a special result
/// that the agent loop interprets as "pause and wait for user input."
pub struct ClarifyTool;

impl Default for ClarifyTool {
    fn default() -> Self {
        Self
    }
}

impl ClarifyTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for ClarifyTool {
    fn name(&self) -> &str {
        "clarify"
    }

    fn description(&self) -> &str {
        "Ask the user a clarifying question when you need more information to proceed. \
         Use this when the user's request is ambiguous or you need to confirm a destructive action."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional list of suggested options for the user to choose from"
                },
                "context": {
                    "type": "string",
                    "description": "Brief context explaining why you need this clarification"
                }
            },
            "required": ["question"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        Capabilities::new()
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let question = params
                .get("question")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "Missing required parameter: question".to_string(),
                })?;

            let options = params.get("options").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect::<Vec<_>>()
            });

            let context = params.get("context").and_then(|v| v.as_str());

            let mut response = String::new();
            if let Some(ctx) = context {
                response.push_str(&format!("**Context:** {ctx}\n\n"));
            }
            response.push_str(&format!("**Question:** {question}"));
            if let Some(opts) = options {
                response.push_str("\n\n**Options:**\n");
                for (i, opt) in opts.iter().enumerate() {
                    response.push_str(&format!("{}. {}\n", i + 1, opt));
                }
            }

            // Return the clarification as a text result.
            // The agent loop will interpret this as a message to forward to the user.
            Ok(ToolResult::text(response))
        })
    }
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_original_span(haystack: &str, needle: &str) -> Option<String> {
    // Try to find a span in haystack that, when whitespace-normalized, matches the normalized needle
    let norm_needle = normalize_whitespace(needle);
    let haystack_lines: Vec<&str> = haystack.lines().collect();
    let needle_lines: Vec<&str> = needle.lines().collect();
    let window_size = needle_lines.len();

    if window_size == 0 || haystack_lines.len() < window_size {
        return None;
    }

    for i in 0..=(haystack_lines.len() - window_size) {
        let window: String = haystack_lines[i..i + window_size].join("\n");
        if normalize_whitespace(&window) == norm_needle {
            return Some(window);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// BrowserTool — headless browser fetch with JS rendering
// ---------------------------------------------------------------------------

/// Maximum text length returned by browser tool (8 KB)
const BROWSER_MAX_TEXT: usize = 8 * 1024;

/// Navigate to a URL using a headless browser (Chrome/Chromium) and return the
/// rendered text content. Falls back to plain HTTP GET when no browser binary
/// is found.
pub struct BrowserTool;

impl Default for BrowserTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Navigate to a URL and extract rendered page content. Useful for JavaScript-heavy pages \
         that web_fetch cannot properly render. Optionally extract specific elements via CSS selector."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to navigate to"
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector to extract specific content"
                },
                "wait_ms": {
                    "type": "integer",
                    "description": "Milliseconds to wait for page load (default: 3000)"
                }
            },
            "required": ["url"]
        })
    }

    fn required_capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new();
        caps.add(rockbot_security::Capability::NetworkAccess("*".to_string()));
        caps
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _context: ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| crate::ToolError::InvalidParameters {
                    message: "url is required".to_string(),
                })?
                .to_string();

            let parsed_url = validate_web_fetch_url(&url).await?;
            let normalized_url = parsed_url.to_string();

            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .map(str::to_string);

            let wait_ms = params
                .get("wait_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(3000);

            // Try headless Chrome/Chromium first
            if let Ok(content) =
                fetch_with_headless_chrome(&normalized_url, selector.as_deref(), wait_ms).await
            {
                return Ok(ToolResult::text(content));
            }

            // Fall back to plain HTTP GET + HTML stripping
            fetch_with_http_fallback(&normalized_url).await
        })
    }
}

/// Attempt to render `url` using a locally-installed headless Chrome/Chromium
/// binary. Returns `Err` if no binary is found or the process fails.
async fn fetch_with_headless_chrome(
    url: &str,
    selector: Option<&str>,
    wait_ms: u64,
) -> Result<String> {
    let browser = find_chrome_binary().ok_or_else(|| crate::ToolError::ExecutionFailed {
        message: "No Chrome/Chromium binary found".to_string(),
    })?;

    let mut cmd = Command::new(&browser);
    cmd.args([
        "--headless",
        "--disable-gpu",
        "--no-sandbox",
        "--dump-dom",
        url,
    ]);

    // Give the browser a bit of extra time beyond the requested wait
    let timeout_dur = std::time::Duration::from_millis(wait_ms + 5000);
    let output = tokio::time::timeout(timeout_dur, cmd.output())
        .await
        .map_err(|_| crate::ToolError::ExecutionFailed {
            message: "Browser timed out".to_string(),
        })?
        .map_err(|e| crate::ToolError::ExecutionFailed {
            message: format!("Browser error: {e}"),
        })?;

    if !output.status.success() {
        return Err(crate::ToolError::ExecutionFailed {
            message: format!("Browser exited with {}", output.status),
        });
    }

    let html = String::from_utf8_lossy(&output.stdout);
    let text = strip_html_tags(&html);

    let result = if let Some(sel) = selector {
        format!(
            "Page content (selector: {sel}):\n\n{}",
            truncate_browser_text(&text, BROWSER_MAX_TEXT)
        )
    } else {
        truncate_browser_text(&text, BROWSER_MAX_TEXT)
    };

    Ok(result)
}

/// Probe standard binary names and return the first one that exists on PATH.
fn find_chrome_binary() -> Option<String> {
    for bin in [
        "google-chrome",
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
    ] {
        // Use `which` via a synchronous PATH search: check if any directory in
        // PATH contains this executable.
        if std::env::var_os("PATH")
            .map(|path_val| std::env::split_paths(&path_val).any(|dir| dir.join(bin).is_file()))
            .unwrap_or(false)
        {
            return Some(bin.to_string());
        }
    }
    None
}

/// Fallback: plain HTTP GET with HTML tag stripping (no JS rendering).
async fn fetch_with_http_fallback(url: &str) -> Result<ToolResult> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| crate::ToolError::ExecutionFailed {
            message: e.to_string(),
        })?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| crate::ToolError::ExecutionFailed {
            message: format!("HTTP error: {e}"),
        })?;

    let body = resp
        .text()
        .await
        .map_err(|e| crate::ToolError::ExecutionFailed {
            message: format!("Read error: {e}"),
        })?;

    let text = strip_html_tags(&body);
    Ok(ToolResult::text(format!(
        "Page content (HTTP fallback, no JS rendering):\n\n{}",
        truncate_browser_text(&text, BROWSER_MAX_TEXT)
    )))
}

/// Truncate text to `max_chars`, appending a note when truncation occurs.
fn truncate_browser_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!(
            "{}...\n[truncated at {} chars]",
            &text[..max_chars],
            max_chars
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::message::ToolResult;

    fn make_context(workspace: std::path::PathBuf) -> ToolExecutionContext {
        ToolExecutionContext {
            session_id: "test-session".to_string(),
            agent_id: "test-agent".to_string(),
            workspace_path: workspace,
            security_context: rockbot_security::SecurityContext {
                session_id: "test-session".to_string(),
                capabilities: {
                    let mut caps = rockbot_security::Capabilities::new();
                    caps.extend(rockbot_security::Capabilities::filesystem_read());
                    caps.extend(rockbot_security::Capabilities::filesystem_write());
                    caps
                },
                sandbox_enabled: false,
                restrictions: rockbot_security::SecurityRestrictions::default(),
            },
            credential_accessor: None,
            command_allowlist: vec![],
            approval_callback: None,
            agent_invoker: None,
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        }
    }

    #[tokio::test]
    async fn test_edit_single_occurrence_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world\n").await.unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "hello",
            "new_text": "goodbye"
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();

        assert!(matches!(result, ToolResult::Text { .. }));
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "goodbye world\n");
    }

    #[tokio::test]
    async fn test_edit_zero_occurrences_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world\n").await.unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "nonexistent",
            "new_text": "replacement"
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();

        assert!(matches!(result, ToolResult::Error { .. }));
        if let ToolResult::Error { message, .. } = result {
            assert!(message.contains("not found"));
        }
    }

    #[tokio::test]
    async fn test_edit_multiple_occurrences_without_replace_all_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "foo bar foo baz foo\n")
            .await
            .unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "foo",
            "new_text": "qux"
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();

        assert!(matches!(result, ToolResult::Error { .. }));
        if let ToolResult::Error { message, .. } = result {
            assert!(
                message.contains('3'),
                "expected count 3 in message: {message}"
            );
            assert!(message.contains("replace_all=true"));
        }
        // File should be unchanged
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "foo bar foo baz foo\n");
    }

    #[tokio::test]
    async fn test_edit_multiple_occurrences_with_replace_all_replaces_all() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "foo bar foo baz foo\n")
            .await
            .unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "foo",
            "new_text": "qux",
            "replace_all": true
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();

        assert!(matches!(result, ToolResult::Text { .. }));
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "qux bar qux baz qux\n");
    }

    #[tokio::test]
    async fn test_read_tool_offset_beyond_end_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello\nworld\n")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "offset": 10,
            "limit": 5
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();

        if let ToolResult::Text { content } = result {
            assert!(content.is_empty());
        } else {
            panic!("expected text result");
        }
    }

    #[tokio::test]
    async fn test_read_tool_rejects_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.bin");
        tokio::fs::write(&file_path, b"abc\0def").await.unwrap();

        let tool = ReadTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap()
        });
        let context = make_context(dir.path().to_path_buf());
        let error = tool.execute(params, context).await.unwrap_err();
        assert!(error.to_string().contains("binary file"));
    }

    // --- TestTool / LintTool detection tests ---

    fn make_exec_context(workspace: std::path::PathBuf) -> ToolExecutionContext {
        ToolExecutionContext {
            session_id: "test-session".to_string(),
            agent_id: "test-agent".to_string(),
            workspace_path: workspace,
            security_context: rockbot_security::SecurityContext {
                session_id: "test-session".to_string(),
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
            approval_callback: None,
            agent_invoker: None,
            delegation_depth: 0,
            blackboard: None,
            swarm_id: None,
        }
    }

    #[test]
    fn test_tool_name_and_description() {
        let t = TestTool::new();
        assert_eq!(t.name(), "test");
        assert!(!t.description().is_empty());
        assert!(!t.requires_approval());
    }

    #[test]
    fn test_lint_tool_name_and_description() {
        let l = LintTool::new();
        assert_eq!(l.name(), "lint");
        assert!(!l.description().is_empty());
        assert!(!l.requires_approval());
    }

    #[tokio::test]
    async fn test_detect_test_command_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let cmd = detect_test_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "cargo test");
    }

    #[tokio::test]
    async fn test_detect_test_command_cargo_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let cmd = detect_test_command(dir.path(), Some("my_test"))
            .await
            .unwrap();
        assert_eq!(cmd, "cargo test -- my_test");
    }

    #[tokio::test]
    async fn test_detect_test_command_npm() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let cmd = detect_test_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "npm test");
    }

    #[test]
    fn test_parse_command_line_supports_quoted_args() {
        let parsed = parse_command_line("printf 'hello world'").unwrap();
        assert_eq!(parsed.program, "printf");
        assert_eq!(parsed.args, vec!["hello world"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_workspace_path_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = dir.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        let err = resolve_workspace_path(dir.path(), "escape/secret.txt").unwrap_err();
        assert!(matches!(err, crate::ToolError::InvalidParameters { .. }));
    }

    #[test]
    fn test_parse_command_line_rejects_empty_command() {
        let err = parse_command_line("   ").unwrap_err();
        assert!(matches!(err, crate::ToolError::InvalidParameters { .. }));
    }

    #[test]
    fn test_truncate_stream_output_appends_notice() {
        let text = "abcdef";
        let truncated = truncate_stream_output(text, 3, "stdout");
        assert!(truncated.starts_with("abc"));
        assert!(truncated.contains("stdout truncated"));
        assert!(truncated.contains("6 chars"));
    }

    #[test]
    fn test_truncate_combined_output_appends_notice() {
        let text = "abcdefgh";
        let truncated = truncate_combined_output(text, 4);
        assert!(truncated.starts_with("abcd"));
        assert!(truncated.contains("output truncated"));
        assert!(truncated.contains("8 chars"));
    }

    #[test]
    fn test_safe_read_only_exec_command_detection() {
        assert!(is_safe_read_only_exec_command("pwd"));
        assert!(is_safe_read_only_exec_command("git status --short"));
        assert!(is_safe_read_only_exec_command("git rev-parse HEAD"));
        assert!(is_safe_read_only_exec_command("ls -la src"));
        assert!(is_safe_read_only_exec_command("find src -name '*.rs'"));
        assert!(is_safe_read_only_exec_command("cat README.md"));
        assert!(!is_safe_read_only_exec_command("git commit -m test"));
        assert!(!is_safe_read_only_exec_command("cat /etc/passwd"));
        assert!(!is_safe_read_only_exec_command("find . -delete"));
        assert!(!is_safe_read_only_exec_command("rm -rf /tmp/nope"));
    }

    #[tokio::test]
    async fn test_exec_tool_does_not_expand_shell_metacharacters() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker.txt");
        let tool = ExecTool::new();
        let context = make_exec_context(dir.path().to_path_buf());

        let result = tool
            .execute(
                serde_json::json!({
                    "command": format!("printf safe ; touch {}", marker.display()),
                    "workdir": "."
                }),
                context,
            )
            .await
            .unwrap();

        assert!(
            !marker.exists(),
            "shell metacharacters should not be interpreted"
        );
        if let ToolResult::Json { data } = result {
            assert_eq!(data.get("success").and_then(|v| v.as_bool()), Some(true));
            assert_eq!(data.get("stdout").and_then(|v| v.as_str()), Some("safe"));
        } else {
            panic!("expected JSON result");
        }
    }

    #[test]
    fn test_exec_output_limit_env_uses_default_for_invalid_values() {
        assert_eq!(
            get_bounded_env_usize("ROCKBOT_TEST_MISSING_LIMIT", 123, 456),
            123
        );
    }

    #[test]
    fn test_sanitize_shell_command_rejects_newlines() {
        let err = sanitize_shell_command("echo hello\nrm -rf /").unwrap_err();
        assert!(matches!(err, crate::ToolError::InvalidParameters { .. }));
    }

    #[tokio::test]
    async fn test_detect_test_command_npm_with_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        let cmd = detect_test_command(dir.path(), Some("myspec"))
            .await
            .unwrap();
        assert_eq!(cmd, "npx jest myspec");
    }

    #[tokio::test]
    async fn test_detect_test_command_python_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "[tool.pytest]").unwrap();
        let cmd = detect_test_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "pytest");
    }

    #[tokio::test]
    async fn test_detect_test_command_python_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("setup.py"), "").unwrap();
        let cmd = detect_test_command(dir.path(), Some("test_foo"))
            .await
            .unwrap();
        assert_eq!(cmd, "pytest test_foo");
    }

    #[tokio::test]
    async fn test_detect_test_command_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/m").unwrap();
        let cmd = detect_test_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "go test ./...");
    }

    #[tokio::test]
    async fn test_detect_test_command_makefile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Makefile"), "\ntest:\n\t./run_tests.sh\n").unwrap();
        let cmd = detect_test_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "make test");
    }

    #[tokio::test]
    async fn test_detect_test_command_no_project() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_test_command(dir.path(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_detect_lint_command_cargo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let cmd = detect_lint_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "cargo clippy -- -D warnings");
    }

    #[tokio::test]
    async fn test_detect_lint_command_npm_with_lint_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts": {"lint": "eslint ."}}"#,
        )
        .unwrap();
        let cmd = detect_lint_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "npm run lint");
    }

    #[tokio::test]
    async fn test_detect_lint_command_npm_without_lint_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"scripts": {}}"#).unwrap();
        let result = detect_lint_command(dir.path(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_detect_lint_command_go() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("go.mod"), "module example.com/m").unwrap();
        let cmd = detect_lint_command(dir.path(), None).await.unwrap();
        assert_eq!(cmd, "golangci-lint run");
    }

    #[tokio::test]
    async fn test_detect_lint_command_no_project() {
        let dir = tempfile::tempdir().unwrap();
        let result = detect_lint_command(dir.path(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_test_tool_timeout() {
        let dir = tempfile::tempdir().unwrap();
        // Use a Makefile with a sleep target so detection succeeds but execution times out
        std::fs::write(dir.path().join("Makefile"), "\ntest:\n\tsleep 10\n").unwrap();
        let tool = TestTool::new();
        let ctx = make_exec_context(dir.path().to_path_buf());
        let params = serde_json::json!({ "timeout": 1 });
        let result = tool.execute(params, ctx).await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "Expected timeout error, got: {msg}"
        );
    }

    #[test]
    fn test_clarify_tool_name() {
        let tool = ClarifyTool::new();
        assert_eq!(tool.name(), "clarify");
        assert!(!tool.requires_approval());
    }

    #[tokio::test]
    async fn test_clarify_with_question_only() {
        let tool = ClarifyTool::new();
        let params = serde_json::json!({
            "question": "Which database should I use?"
        });
        let context = make_context(std::path::PathBuf::from("/tmp"));
        let result = tool.execute(params, context).await.unwrap();
        if let ToolResult::Text { content } = &result {
            assert!(content.contains("Which database should I use?"));
        } else {
            panic!("Expected Text result");
        }
    }

    #[tokio::test]
    async fn test_clarify_with_options() {
        let tool = ClarifyTool::new();
        let params = serde_json::json!({
            "question": "Which approach do you prefer?",
            "options": ["Option A: Simple", "Option B: Complex"],
            "context": "There are multiple ways to solve this"
        });
        let context = make_context(std::path::PathBuf::from("/tmp"));
        let result = tool.execute(params, context).await.unwrap();
        if let ToolResult::Text { content } = &result {
            assert!(content.contains("Option A"));
            assert!(content.contains("Option B"));
            assert!(content.contains("Context"));
        } else {
            panic!("Expected Text result");
        }
    }

    #[tokio::test]
    async fn test_edit_fuzzy_whitespace_match() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.rs");
        tokio::fs::write(&file_path, "fn  hello()  {\n    println!(\"hi\");\n}\n")
            .await
            .unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "fn hello() {\n    println!(\"hi\");\n}",
            "new_text": "fn goodbye() {\n    println!(\"bye\");\n}"
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();
        assert!(matches!(result, ToolResult::Text { .. }));
    }

    #[tokio::test]
    async fn test_edit_no_approximate_auto_match() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world\n").await.unwrap();

        let tool = EditTool::new();
        let params = serde_json::json!({
            "file_path": file_path.to_str().unwrap(),
            "old_text": "completely different text that has no similarity",
            "new_text": "replacement"
        });
        let context = make_context(dir.path().to_path_buf());
        let result = tool.execute(params, context).await.unwrap();
        assert!(matches!(result, ToolResult::Error { .. }));
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "hello world\n");
    }

    // -----------------------------------------------------------------------
    // BrowserTool tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_browser_tool_name_and_approval() {
        let tool = BrowserTool::new();
        assert_eq!(tool.name(), "browser");
        assert!(tool.requires_approval());
    }

    #[tokio::test]
    async fn test_browser_tool_rejects_non_http_url() {
        let tool = BrowserTool::new();
        let params = serde_json::json!({ "url": "ftp://example.com/file" });
        let context = make_context(std::path::PathBuf::from("/tmp"));
        let error = tool.execute(params, context).await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Unsupported URL scheme"));
        assert!(message.contains("ftp"));
    }

    #[test]
    fn test_strip_html_tags_browser() {
        assert_eq!(strip_html_tags("<p>Hello <b>world</b></p>"), "Hello world");
        assert_eq!(strip_html_tags("no tags here"), "no tags here");
        assert_eq!(strip_html_tags(""), "");
    }

    #[test]
    fn test_truncate_browser_text_short() {
        assert_eq!(truncate_browser_text("short", 100), "short");
    }

    #[test]
    fn test_truncate_browser_text_long() {
        let long = "a".repeat(200);
        let truncated = truncate_browser_text(&long, 50);
        assert!(truncated.len() < 200);
        assert!(truncated.contains("truncated"));
        assert!(truncated.contains("50"));
    }
}
