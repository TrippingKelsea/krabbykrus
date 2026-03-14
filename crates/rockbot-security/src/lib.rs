//! Security and capability system for RockBot

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use thiserror::Error;

/// Security error types
#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("Access denied to resource: {resource}")]
    AccessDenied { resource: String },
    
    #[error("Capability '{capability}' not granted")]
    CapabilityDenied { capability: String },
    
    #[error("Sandbox creation failed: {message}")]
    SandboxCreationFailed { message: String },
    
    #[error("Authentication failed")]
    AuthenticationFailed,
}

/// Result type for security operations
pub type Result<T> = std::result::Result<T, SecurityError>;

/// Capability system for fine-grained permission control
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    /// Read files from specific paths
    FilesystemRead(PathBuf),
    /// Write files to specific paths
    FilesystemWrite(PathBuf),
    /// Execute processes
    ProcessExecute,
    /// Network access to specific domains
    NetworkAccess(String),
    /// System information access
    SystemInfo,
    /// Database access
    DatabaseAccess,
}

/// Set of capabilities
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    capabilities: HashSet<Capability>,
}

/// Security context for a session
#[derive(Debug, Clone)]
pub struct SecurityContext {
    pub session_id: String,
    pub capabilities: Capabilities,
    pub sandbox_enabled: bool,
    pub restrictions: SecurityRestrictions,
}

/// Security restrictions
#[derive(Debug, Clone, Default)]
pub struct SecurityRestrictions {
    pub max_file_size: Option<usize>,
    pub max_execution_time: Option<std::time::Duration>,
    pub allowed_executables: Option<HashSet<String>>,
    pub forbidden_paths: HashSet<PathBuf>,
}

/// Security manager handles capability enforcement
pub struct SecurityManager {
    config: SecurityConfig,
    session_contexts: tokio::sync::RwLock<std::collections::HashMap<String, SecurityContext>>,
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub sandbox: SandboxConfig,
    pub capabilities: CapabilityConfig,
}

/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    pub mode: String,
    pub scope: String,
    pub image: Option<String>,
}

/// Default capability configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityConfig {
    pub filesystem: Option<FilesystemCapabilities>,
    pub network: Option<NetworkCapabilities>,
    pub process: Option<ProcessCapabilities>,
}

/// Filesystem capability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemCapabilities {
    pub read_paths: Vec<PathBuf>,
    pub write_paths: Vec<PathBuf>,
    pub forbidden_paths: Vec<PathBuf>,
}

/// Network capability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkCapabilities {
    pub allowed_domains: Vec<String>,
    pub blocked_domains: Vec<String>,
    pub max_request_size: Option<usize>,
}

/// Process capability configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCapabilities {
    pub allowed_commands: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub max_execution_time: Option<u64>,
}

impl Capabilities {
    /// Create new empty capability set
    pub fn new() -> Self {
        Self {
            capabilities: HashSet::new(),
        }
    }
    
    /// Create filesystem read capabilities
    pub fn filesystem_read() -> Self {
        let mut caps = Self::new();
        caps.add(Capability::FilesystemRead(PathBuf::from(".")));
        caps
    }
    
    /// Create filesystem write capabilities
    pub fn filesystem_write() -> Self {
        let mut caps = Self::new();
        caps.add(Capability::FilesystemWrite(PathBuf::from(".")));
        caps
    }
    
    /// Create process execution capabilities
    pub fn process_execute() -> Self {
        let mut caps = Self::new();
        caps.add(Capability::ProcessExecute);
        caps
    }
    
    /// Add a capability
    pub fn add(&mut self, capability: Capability) {
        self.capabilities.insert(capability);
    }
    
    /// Remove a capability
    pub fn remove(&mut self, capability: &Capability) {
        self.capabilities.remove(capability);
    }
    
    /// Check if this capability set allows the required capabilities
    pub fn allows(&self, required: &Capabilities) -> bool {
        for required_cap in &required.capabilities {
            if !self.has_capability(required_cap) {
                return false;
            }
        }
        true
    }
    
    /// Check if a specific capability is granted
    pub fn has_capability(&self, capability: &Capability) -> bool {
        match capability {
            Capability::FilesystemRead(path) => {
                // Check if we have read access to this path or a parent path
                self.capabilities.iter().any(|cap| match cap {
                    Capability::FilesystemRead(allowed_path) => {
                        path.starts_with(allowed_path) || allowed_path == &PathBuf::from(".")
                    }
                    _ => false,
                })
            }
            Capability::FilesystemWrite(path) => {
                // Check if we have write access to this path or a parent path
                self.capabilities.iter().any(|cap| match cap {
                    Capability::FilesystemWrite(allowed_path) => {
                        path.starts_with(allowed_path) || allowed_path == &PathBuf::from(".")
                    }
                    _ => false,
                })
            }
            _ => self.capabilities.contains(capability),
        }
    }
    
    /// Extend capabilities with another set
    pub fn extend(&mut self, other: Capabilities) {
        self.capabilities.extend(other.capabilities);
    }
}

impl SecurityManager {
    /// Create a new security manager
    pub async fn new(config: SecurityConfig) -> Result<Self> {
        Ok(Self {
            config,
            session_contexts: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        })
    }
    
    /// Get security context for a session
    pub async fn get_session_context(&self, session_id: &str) -> Result<SecurityContext> {
        let contexts = self.session_contexts.read().await;
        if let Some(context) = contexts.get(session_id) {
            Ok(context.clone())
        } else {
            // Create default context for session
            drop(contexts);
            self.create_session_context(session_id).await
        }
    }
    
    /// Create security context for a new session
    pub async fn create_session_context(&self, session_id: &str) -> Result<SecurityContext> {
        let mut capabilities = Capabilities::new();

        // Add capabilities based on configuration.
        // When capability sections are None (unconfigured), grant permissive defaults
        // so agents can use tools out of the box. Explicit config restricts to listed paths.
        if let Some(fs_config) = &self.config.capabilities.filesystem {
            for path in &fs_config.read_paths {
                capabilities.add(Capability::FilesystemRead(path.clone()));
            }
            for path in &fs_config.write_paths {
                capabilities.add(Capability::FilesystemWrite(path.clone()));
            }
        } else {
            // Default: allow read/write from working directory
            capabilities.add(Capability::FilesystemRead(PathBuf::from(".")));
            capabilities.add(Capability::FilesystemWrite(PathBuf::from(".")));
        }

        if let Some(net_config) = &self.config.capabilities.network {
            for domain in &net_config.allowed_domains {
                capabilities.add(Capability::NetworkAccess(domain.clone()));
            }
        }

        if self.config.capabilities.process.is_some() {
            capabilities.add(Capability::ProcessExecute);
        } else {
            // Default: allow process execution for exec tool
            capabilities.add(Capability::ProcessExecute);
        }
        
        let context = SecurityContext {
            session_id: session_id.to_string(),
            capabilities,
            sandbox_enabled: self.config.sandbox.mode != "disabled",
            restrictions: SecurityRestrictions::default(),
        };
        
        // Store context
        let mut contexts = self.session_contexts.write().await;
        contexts.insert(session_id.to_string(), context.clone());
        
        Ok(context)
    }
    
    /// Check if access to a resource is allowed
    pub async fn check_access(&self, session_id: &str, resource: &str, capability: &Capability) -> Result<()> {
        let context = self.get_session_context(session_id).await?;
        
        if !context.capabilities.has_capability(capability) {
            return Err(SecurityError::AccessDenied {
                resource: resource.to_string(),
            });
        }
        
        Ok(())
    }
}

/// Enforcement result from sandbox restriction checks
#[derive(Debug)]
pub enum EnforcementResult {
    /// Action is allowed
    Allowed,
    /// Action is denied with a reason
    Denied { reason: String },
}

/// Enforce security restrictions on a tool execution request.
///
/// Validates the requested action against the `SecurityRestrictions`:
/// - Path access: rejected if the path matches any `forbidden_paths` glob
/// - Executable allowlist: rejected if `allowed_executables` is non-empty and the
///   command is not in the list
/// - Returns the configured `max_execution_time` for use by execution tools
pub fn enforce_path(path: &std::path::Path, restrictions: &SecurityRestrictions) -> EnforcementResult {
    for forbidden in &restrictions.forbidden_paths {
        if path.starts_with(forbidden) || path == forbidden {
            return EnforcementResult::Denied {
                reason: format!("Path '{}' is forbidden by security restrictions", path.display()),
            };
        }
    }
    EnforcementResult::Allowed
}

/// Check if an executable is allowed by the security restrictions.
///
/// If `allowed_executables` is `None` or empty, all executables are allowed.
/// If set, only listed executables may be run.
pub fn enforce_executable(cmd: &str, restrictions: &SecurityRestrictions) -> EnforcementResult {
    if let Some(ref allowed) = restrictions.allowed_executables {
        if !allowed.is_empty() && !allowed.contains(cmd) {
            return EnforcementResult::Denied {
                reason: format!("Executable '{cmd}' is not in the allowed list"),
            };
        }
    }
    EnforcementResult::Allowed
}

/// Get the configured execution timeout from security restrictions.
pub fn enforce_timeout(restrictions: &SecurityRestrictions) -> Option<std::time::Duration> {
    restrictions.max_execution_time
}

/// Enforce all applicable restrictions for a file operation.
pub fn enforce_file_access(path: &std::path::Path, restrictions: &SecurityRestrictions) -> EnforcementResult {
    // Check forbidden paths
    if let result @ EnforcementResult::Denied { .. } = enforce_path(path, restrictions) {
        return result;
    }
    // Check file size (can only be enforced at write time — caller is responsible)
    EnforcementResult::Allowed
}

/// Mock security manager for testing
pub struct MockSecurityManager {
    default_context: SecurityContext,
}

impl Default for MockSecurityManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSecurityManager {
    pub fn new() -> Self {
        let mut capabilities = Capabilities::new();
        capabilities.add(Capability::FilesystemRead(std::path::PathBuf::from(".")));
        capabilities.add(Capability::FilesystemWrite(std::path::PathBuf::from(".")));
        capabilities.add(Capability::ProcessExecute);
        
        Self {
            default_context: SecurityContext {
                session_id: "mock-session".to_string(),
                capabilities,
                sandbox_enabled: false,
                restrictions: SecurityRestrictions::default(),
            },
        }
    }
    
    pub async fn get_session_context(&self, _session_id: &str) -> Result<SecurityContext> {
        Ok(self.default_context.clone())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    
    #[test]
    fn test_capabilities() {
        let mut caps = Capabilities::new();
        caps.add(Capability::FilesystemRead(PathBuf::from("/tmp")));
        
        assert!(caps.has_capability(&Capability::FilesystemRead(PathBuf::from("/tmp/test.txt"))));
        assert!(!caps.has_capability(&Capability::FilesystemWrite(PathBuf::from("/tmp/test.txt"))));
    }
    
    #[tokio::test]
    async fn test_security_manager() {
        let config = SecurityConfig {
            sandbox: SandboxConfig {
                mode: "tools".to_string(),
                scope: "session".to_string(),
                image: None,
            },
            capabilities: CapabilityConfig::default(),
        };
        
        let manager = SecurityManager::new(config).await.unwrap();
        let context = manager.create_session_context("test-session").await.unwrap();
        
        assert_eq!(context.session_id, "test-session");
        assert!(context.sandbox_enabled);
    }

    #[test]
    fn test_enforce_path_allowed() {
        let restrictions = SecurityRestrictions::default();
        assert!(matches!(
            enforce_path(std::path::Path::new("/tmp/test.txt"), &restrictions),
            EnforcementResult::Allowed
        ));
    }

    #[test]
    fn test_enforce_path_forbidden() {
        let mut restrictions = SecurityRestrictions::default();
        restrictions.forbidden_paths.insert(PathBuf::from("/etc/secrets"));
        assert!(matches!(
            enforce_path(std::path::Path::new("/etc/secrets/key.pem"), &restrictions),
            EnforcementResult::Denied { .. }
        ));
    }

    #[test]
    fn test_enforce_executable_no_allowlist() {
        let restrictions = SecurityRestrictions::default();
        assert!(matches!(
            enforce_executable("rm", &restrictions),
            EnforcementResult::Allowed
        ));
    }

    #[test]
    fn test_enforce_executable_allowed() {
        let mut allowed = HashSet::new();
        allowed.insert("ls".to_string());
        allowed.insert("cat".to_string());
        let restrictions = SecurityRestrictions {
            allowed_executables: Some(allowed),
            ..Default::default()
        };
        assert!(matches!(enforce_executable("ls", &restrictions), EnforcementResult::Allowed));
        assert!(matches!(enforce_executable("rm", &restrictions), EnforcementResult::Denied { .. }));
    }

    #[test]
    fn test_enforce_timeout() {
        let restrictions = SecurityRestrictions {
            max_execution_time: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        };
        assert_eq!(enforce_timeout(&restrictions), Some(std::time::Duration::from_secs(30)));
    }
}