//! Container-based sandbox execution for RockBot.
//!
//! Provides isolated command execution via Docker containers when the sandbox
//! mode is set to `"docker"` or `"container"`. Falls back to direct execution
//! when Docker is unavailable or disabled.

use crate::config::SandboxConfig;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, warn};

/// Result of a sandboxed command execution.
#[derive(Debug, Clone)]
pub struct SandboxResult {
    /// Combined stdout output.
    pub stdout: String,
    /// Combined stderr output.
    pub stderr: String,
    /// Process exit code (None if killed by signal/timeout).
    pub exit_code: Option<i32>,
    /// Whether the command was run inside a container.
    pub containerized: bool,
}

/// Check if Docker is available on the system.
pub async fn is_docker_available() -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// Execute a command inside a Docker container sandbox.
///
/// The container is created with:
/// - `--rm`: auto-remove on exit
/// - `--network none`: no network access (security)
/// - `-v workspace:/workspace`: mount the workspace read-write
/// - Memory and CPU limits
/// - Timeout enforcement via tokio
///
/// # Arguments
/// * `config` - Sandbox configuration (must have `image` set)
/// * `workspace` - Host path to mount as /workspace
/// * `command` - Shell command to run inside the container
/// * `timeout` - Maximum execution time
pub async fn execute_in_container(
    config: &SandboxConfig,
    workspace: &Path,
    command: &str,
    timeout: Duration,
) -> Result<SandboxResult, SandboxError> {
    let image = config
        .image
        .as_deref()
        .unwrap_or("ubuntu:22.04");

    let workspace_str = workspace
        .to_str()
        .ok_or_else(|| SandboxError::InvalidWorkspace(workspace.display().to_string()))?;

    let mut docker_cmd = Command::new("docker");
    docker_cmd.args([
        "run",
        "--rm",
        "--network", "none",
        "--memory", "512m",
        "--cpus", "1.0",
        "--pids-limit", "256",
        "--read-only",
        "--tmpfs", "/tmp:rw,size=64m",
        "-v", &format!("{workspace_str}:/workspace:rw"),
        "-w", "/workspace",
        image,
        "sh", "-c", command,
    ]);

    docker_cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    debug!(
        "Executing in container (image={image}): {command}",
    );

    let child = docker_cmd
        .spawn()
        .map_err(|e| SandboxError::SpawnFailed(e.to_string()))?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(SandboxError::ExecutionFailed(e.to_string())),
        Err(_) => {
            warn!("Container command timed out after {}s", timeout.as_secs());
            return Err(SandboxError::Timeout(timeout.as_secs()));
        }
    };

    Ok(SandboxResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code(),
        containerized: true,
    })
}

/// Execute a command with sandbox awareness.
///
/// If the sandbox config specifies container mode and Docker is available,
/// runs the command in a container. Otherwise, falls back to direct execution.
pub async fn execute_sandboxed(
    config: &SandboxConfig,
    workspace: &Path,
    command: &str,
    timeout: Duration,
) -> Result<SandboxResult, SandboxError> {
    let use_container = matches!(config.mode.as_str(), "docker" | "container")
        && config.image.is_some();

    if use_container {
        if is_docker_available().await {
            return execute_in_container(config, workspace, command, timeout).await;
        }
        warn!("Docker requested but not available, falling back to direct execution");
    }

    // Direct execution fallback
    execute_direct(workspace, command, timeout).await
}

/// Execute a command directly (no container).
async fn execute_direct(
    workspace: &Path,
    command: &str,
    timeout: Duration,
) -> Result<SandboxResult, SandboxError> {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command])
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| SandboxError::SpawnFailed(e.to_string()))?;

    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(SandboxError::ExecutionFailed(e.to_string())),
        Err(_) => return Err(SandboxError::Timeout(timeout.as_secs())),
    };

    Ok(SandboxResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code(),
        containerized: false,
    })
}

/// Errors that can occur during sandboxed execution.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("Invalid workspace path: {0}")]
    InvalidWorkspace(String),
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Command timed out after {0}s")]
    Timeout(u64),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_sandbox_result_default_values() {
        let result = SandboxResult {
            stdout: "hello".to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            containerized: false,
        };
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.containerized);
    }

    #[tokio::test]
    async fn test_execute_direct_echo() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute_direct(dir.path(), "echo hello", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.containerized);
    }

    #[tokio::test]
    async fn test_execute_direct_failure() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute_direct(dir.path(), "exit 42", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(42));
    }

    #[tokio::test]
    async fn test_execute_direct_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let result = execute_direct(dir.path(), "sleep 60", Duration::from_millis(100)).await;
        assert!(matches!(result, Err(SandboxError::Timeout(_))));
    }

    #[tokio::test]
    async fn test_execute_sandboxed_fallback_to_direct() {
        let dir = tempfile::tempdir().unwrap();
        let config = SandboxConfig {
            mode: "disabled".to_string(),
            scope: "none".to_string(),
            image: None,
        };
        let result = execute_sandboxed(&config, dir.path(), "echo test", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "test");
        assert!(!result.containerized);
    }
}
