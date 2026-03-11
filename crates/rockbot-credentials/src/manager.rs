//! High-level credential manager for gateway integration
//!
//! Provides a thread-safe, async-compatible interface to the credential vault
//! suitable for use in the rockbot gateway.

use crate::audit::AuditLog;
use crate::crypto::MasterKey;
use crate::error::{CredentialError, Result};
use crate::storage::CredentialVault;
use crate::types::{ApprovalStatus, AuditEntry, CredentialType, Endpoint, EndpointType, PermissionLevel};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};
use uuid::Uuid;

/// Simple path-based permission policy for credential access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathPermission {
    /// Permission ID
    pub id: Uuid,
    /// Path pattern (glob-style: `*` and `**` supported)
    pub path_pattern: String,
    /// Permission level
    pub level: PermissionLevel,
    /// Optional description
    pub description: Option<String>,
}

/// Result of evaluating path permissions
#[derive(Debug, Clone)]
pub struct PathPermissionResult {
    /// The evaluated permission level
    pub level: PermissionLevel,
    /// ID of the matching rule, if any
    pub matched_rule: Option<Uuid>,
    /// Human-readable reason
    pub reason: String,
}

/// A pending HIL (Human-in-the-Loop) approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilApprovalRequest {
    /// Unique request ID
    pub id: Uuid,
    /// Path being accessed
    pub path: String,
    /// Agent requesting access
    pub agent_id: String,
    /// Reason for the request
    pub reason: String,
    /// Permission level required
    pub permission_level: PermissionLevel,
    /// When the request was created
    pub created_at: DateTime<Utc>,
    /// Request timeout (when it expires)
    pub expires_at: DateTime<Utc>,
    /// Current status
    pub status: ApprovalStatus,
    /// Who approved/denied (if resolved)
    pub resolved_by: Option<String>,
    /// When it was resolved
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Response to an HIL approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HilApprovalResponse {
    /// Request ID being responded to
    pub request_id: Uuid,
    /// Whether approved
    pub approved: bool,
    /// Who made the decision
    pub resolved_by: String,
    /// Optional reason for denial
    pub denial_reason: Option<String>,
}

/// Internal tracking for pending approvals
struct PendingApproval {
    request: HilApprovalRequest,
    response_tx: oneshot::Sender<HilApprovalResponse>,
}

/// Channel for notifying about new HIL requests
pub type HilNotificationSender = mpsc::UnboundedSender<HilApprovalRequest>;
pub type HilNotificationReceiver = mpsc::UnboundedReceiver<HilApprovalRequest>;

/// Simple path-based permission evaluator
#[derive(Debug, Default)]
struct PathPermissionEvaluator {
    permissions: Vec<PathPermission>,
}

impl PathPermissionEvaluator {
    fn new() -> Self {
        Self { permissions: Vec::new() }
    }

    fn add_permission(&mut self, perm: PathPermission) {
        self.permissions.push(perm);
    }

    fn remove_permission(&mut self, id: Uuid) -> bool {
        if let Some(pos) = self.permissions.iter().position(|p| p.id == id) {
            self.permissions.remove(pos);
            true
        } else {
            false
        }
    }

    fn list_permissions(&self) -> &[PathPermission] {
        &self.permissions
    }

    /// Evaluate a path against stored permissions
    /// Returns the most restrictive matching rule, or Deny if no match
    fn evaluate(&self, path: &str) -> PathPermissionResult {
        let mut best_match: Option<(&PathPermission, usize)> = None;

        for perm in &self.permissions {
            if self.pattern_matches(&perm.path_pattern, path) {
                let specificity = perm.path_pattern.len();
                match &best_match {
                    None => best_match = Some((perm, specificity)),
                    Some((_, best_specificity)) => {
                        // Prefer longer (more specific) patterns
                        // If tied, prefer more restrictive
                        if specificity > *best_specificity
                            || (specificity == *best_specificity
                                && self.restriction_level(perm.level) > self.restriction_level(best_match.as_ref().unwrap().0.level))
                        {
                            best_match = Some((perm, specificity));
                        }
                    }
                }
            }
        }

        match best_match {
            Some((perm, _)) => PathPermissionResult {
                level: perm.level,
                matched_rule: Some(perm.id),
                reason: format!("matched rule: {}", perm.path_pattern),
            },
            None => PathPermissionResult {
                level: PermissionLevel::Deny,
                matched_rule: None,
                reason: "no matching permission rule".to_string(),
            },
        }
    }

    fn restriction_level(&self, level: PermissionLevel) -> u8 {
        match level {
            PermissionLevel::Allow => 0,
            PermissionLevel::AllowHil => 1,
            PermissionLevel::AllowHil2fa => 2,
            PermissionLevel::Deny => 3,
        }
    }

    /// Simple glob matching for path patterns
    fn pattern_matches(&self, pattern: &str, path: &str) -> bool {
        let pattern_parts: Vec<&str> = pattern.split('/').collect();
        let path_parts: Vec<&str> = path.split('/').collect();

        self.match_parts(&pattern_parts, &path_parts)
    }

    fn match_parts(&self, pattern: &[&str], path: &[&str]) -> bool {
        if pattern.is_empty() && path.is_empty() {
            return true;
        }
        if pattern.is_empty() {
            return false;
        }

        let pat = pattern[0];

        if pat == "**" {
            // ** matches zero or more path segments
            if pattern.len() == 1 {
                return true; // ** at end matches everything
            }
            // Try matching rest of pattern at every position
            for i in 0..=path.len() {
                if self.match_parts(&pattern[1..], &path[i..]) {
                    return true;
                }
            }
            return false;
        }

        if path.is_empty() {
            return false;
        }

        let path_part = path[0];

        if pat == "*" {
            // * matches exactly one segment
            return self.match_parts(&pattern[1..], &path[1..]);
        }

        // Check for wildcard in pattern part
        if pat.contains('*') {
            if self.glob_match_segment(pat, path_part) {
                return self.match_parts(&pattern[1..], &path[1..]);
            }
            return false;
        }

        // Exact match
        if pat == path_part {
            return self.match_parts(&pattern[1..], &path[1..]);
        }

        false
    }

    /// Match a single segment with wildcards
    fn glob_match_segment(&self, pattern: &str, text: &str) -> bool {
        let mut pi = 0;
        let mut ti = 0;
        let mut star_pi = None;
        let mut star_ti = None;
        let pattern: Vec<char> = pattern.chars().collect();
        let text: Vec<char> = text.chars().collect();

        while ti < text.len() {
            if pi < pattern.len() && pattern[pi] == '*' {
                star_pi = Some(pi);
                star_ti = Some(ti);
                pi += 1;
            } else if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
                pi += 1;
                ti += 1;
            } else if let Some(sp) = star_pi {
                pi = sp + 1;
                star_ti = Some(star_ti.unwrap() + 1);
                ti = star_ti.unwrap();
            } else {
                return false;
            }
        }

        while pi < pattern.len() && pattern[pi] == '*' {
            pi += 1;
        }

        pi == pattern.len()
    }
}

/// High-level credential manager for gateway integration
///
/// Thread-safe wrapper around CredentialVault with permission enforcement
/// and audit logging.
pub struct CredentialManager {
    vault: Arc<RwLock<CredentialVault>>,
    permissions: Arc<RwLock<PathPermissionEvaluator>>,
    locked: Arc<RwLock<bool>>,
    /// Pending HIL approval requests
    pending_approvals: Arc<RwLock<HashMap<Uuid, PendingApproval>>>,
    /// Channel to notify about new HIL requests
    hil_notification_tx: Arc<RwLock<Option<HilNotificationSender>>>,
    /// Default HIL timeout in seconds
    hil_timeout_secs: u64,
    /// Path to the vault directory (for audit log access)
    vault_path: PathBuf,
}

/// Result of a credential request with permission info
#[derive(Debug)]
pub struct CredentialRequestResult {
    /// The permission evaluation result
    pub permission: PathPermissionResult,
    /// The decrypted credential (if permission allowed)
    pub credential: Option<Vec<u8>>,
    /// Human-readable reason if denied
    pub reason: Option<String>,
}

impl CredentialManager {
    /// Create a new credential manager with an existing vault directory
    ///
    /// The vault starts in a locked state. Call `unlock()` before use.
    pub fn new<P: AsRef<Path>>(vault_path: P) -> Result<Self> {
        let vault_path_buf = vault_path.as_ref().to_path_buf();
        let vault = CredentialVault::open(&vault_path_buf)?;
        
        Ok(Self {
            vault: Arc::new(RwLock::new(vault)),
            permissions: Arc::new(RwLock::new(PathPermissionEvaluator::new())),
            locked: Arc::new(RwLock::new(true)),
            pending_approvals: Arc::new(RwLock::new(HashMap::new())),
            hil_notification_tx: Arc::new(RwLock::new(None)),
            hil_timeout_secs: 300, // 5 minutes default
            vault_path: vault_path_buf,
        })
    }

    /// Create a new credential manager with custom HIL timeout
    pub fn with_hil_timeout<P: AsRef<Path>>(vault_path: P, timeout_secs: u64) -> Result<Self> {
        let mut manager = Self::new(vault_path)?;
        manager.hil_timeout_secs = timeout_secs;
        Ok(manager)
    }

    /// Subscribe to HIL notification events
    ///
    /// Returns a receiver that will get new HIL approval requests.
    /// Only one subscriber is supported; subsequent calls will replace the previous one.
    pub async fn subscribe_hil_notifications(&self) -> HilNotificationReceiver {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut notifier = self.hil_notification_tx.write().await;
        *notifier = Some(tx);
        rx
    }

    /// Get list of pending HIL approval requests
    pub async fn list_pending_approvals(&self) -> Vec<HilApprovalRequest> {
        let approvals = self.pending_approvals.read().await;
        approvals
            .values()
            .map(|p| p.request.clone())
            .collect()
    }

    /// Respond to an HIL approval request
    pub async fn respond_to_approval(&self, response: HilApprovalResponse) -> Result<()> {
        let mut approvals = self.pending_approvals.write().await;
        
        if let Some(pending) = approvals.remove(&response.request_id) {
            // Send response (ignore if receiver dropped)
            let _ = pending.response_tx.send(response);
            Ok(())
        } else {
            Err(CredentialError::ValidationFailed(format!(
                "approval request {} not found or already resolved",
                response.request_id
            )))
        }
    }

    /// Internal method to request HIL approval and wait for response
    async fn request_hil_approval(
        &self,
        path: &str,
        agent_id: &str,
        reason: &str,
        permission_level: PermissionLevel,
    ) -> Result<bool> {
        let now = Utc::now();
        let expires_at = now + chrono::Duration::seconds(self.hil_timeout_secs as i64);
        
        let request = HilApprovalRequest {
            id: Uuid::new_v4(),
            path: path.to_string(),
            agent_id: agent_id.to_string(),
            reason: reason.to_string(),
            permission_level,
            created_at: now,
            expires_at,
            status: ApprovalStatus::Pending,
            resolved_by: None,
            resolved_at: None,
        };
        
        let (response_tx, response_rx) = oneshot::channel();
        
        // Store pending approval
        {
            let mut approvals = self.pending_approvals.write().await;
            approvals.insert(request.id, PendingApproval {
                request: request.clone(),
                response_tx,
            });
        }
        
        // Notify subscribers
        {
            let notifier = self.hil_notification_tx.read().await;
            if let Some(ref tx) = *notifier {
                let _ = tx.send(request.clone());
            }
        }
        
        // Wait for response with timeout
        let timeout = tokio::time::Duration::from_secs(self.hil_timeout_secs);
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(response)) => Ok(response.approved),
            Ok(Err(_)) => {
                // Channel closed (probably manager dropped)
                self.cleanup_expired_approval(request.id).await;
                Err(CredentialError::ApprovalTimeout)
            }
            Err(_) => {
                // Timeout
                self.cleanup_expired_approval(request.id).await;
                Err(CredentialError::ApprovalTimeout)
            }
        }
    }

    /// Clean up an expired approval request
    async fn cleanup_expired_approval(&self, request_id: Uuid) {
        let mut approvals = self.pending_approvals.write().await;
        approvals.remove(&request_id);
    }

    /// Unlock the vault with a password.
    /// Derives the master key from the stored salt and verifies the password.
    pub async fn unlock_with_password(&self, password: &str) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.unlock_with_password(password)?;
        
        let mut locked = self.locked.write().await;
        *locked = false;
        
        Ok(())
    }

    /// Unlock the vault with a key file.
    /// Only works if the vault was initialized with keyfile-based encryption.
    pub async fn unlock_with_keyfile(&self, keyfile_path: &std::path::Path) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.unlock_with_keyfile(keyfile_path)?;
        
        let mut locked = self.locked.write().await;
        *locked = false;
        
        Ok(())
    }

    /// Unlock the vault with an Age identity.
    /// Only works if the vault was initialized with Age encryption.
    pub async fn unlock_with_age(&self, age_identity: &str) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.unlock_with_age(age_identity)?;
        
        let mut locked = self.locked.write().await;
        *locked = false;
        
        Ok(())
    }

    /// Unlock the vault with an SSH private key.
    /// Only works if the vault was initialized with SSH key encryption.
    pub async fn unlock_with_ssh(&self, private_key_path: &std::path::Path, passphrase: Option<&str>) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.unlock_with_ssh(private_key_path, passphrase)?;
        
        let mut locked = self.locked.write().await;
        *locked = false;
        
        Ok(())
    }

    /// Unlock the vault with a pre-derived master key.
    /// Use `unlock_with_password` for normal password-based unlocking.
    pub async fn unlock(&self, master_key: MasterKey) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.unlock(master_key);
        
        let mut locked = self.locked.write().await;
        *locked = false;
        
        Ok(())
    }

    /// Lock the vault, zeroing the master key
    pub async fn lock(&self) -> Result<()> {
        let mut vault = self.vault.write().await;
        vault.lock();
        
        let mut locked = self.locked.write().await;
        *locked = true;
        
        Ok(())
    }

    /// Check if the vault is locked
    pub async fn is_locked(&self) -> bool {
        *self.locked.read().await
    }

    /// Get the unlock method for this vault
    pub async fn get_unlock_method(&self) -> Option<crate::storage::UnlockMethod> {
        let vault = self.vault.read().await;
        vault.unlock_method().cloned()
    }

    /// Add a permission policy
    pub async fn add_permission(&self, permission: PathPermission) {
        let mut perms = self.permissions.write().await;
        perms.add_permission(permission);
    }

    /// Check permission for a credential path without blocking on HIL
    ///
    /// Returns the permission level without waiting for HIL approval.
    /// Use this to check what permission level would be required before
    /// making a blocking request.
    pub async fn check_permission(&self, path: &str) -> PathPermissionResult {
        let perms = self.permissions.read().await;
        perms.evaluate(path)
    }

    /// Request a credential by path (e.g., "homeassistant://api/token")
    ///
    /// This is the primary interface for agents to request credentials.
    /// It checks permissions and returns appropriate result based on policy.
    ///
    /// For `Allow` permissions, returns the credential immediately.
    /// For `AllowHil`, blocks until human approval is received (or timeout).
    /// For `Deny`, returns an error immediately.
    pub async fn request_credential(
        &self,
        path: &str,
        agent_id: &str,
        reason: &str,
    ) -> Result<CredentialRequestResult> {
        // Check permission first
        let perms = self.permissions.read().await;
        let permission_result = perms.evaluate(path);
        drop(perms); // Release lock before potentially blocking on HIL

        match permission_result.level {
            PermissionLevel::Deny => {
                Ok(CredentialRequestResult {
                    permission: permission_result,
                    credential: None,
                    reason: Some(format!("Access denied by policy: {}", path)),
                })
            }
            PermissionLevel::AllowHil => {
                // Request human approval
                match self.request_hil_approval(path, agent_id, reason, PermissionLevel::AllowHil).await {
                    Ok(true) => {
                        // Approved - retrieve credential
                        self.retrieve_credential(path, permission_result).await
                    }
                    Ok(false) => {
                        // Denied by human
                        Ok(CredentialRequestResult {
                            permission: permission_result,
                            credential: None,
                            reason: Some("Request denied by human operator".to_string()),
                        })
                    }
                    Err(CredentialError::ApprovalTimeout) => {
                        Ok(CredentialRequestResult {
                            permission: permission_result,
                            credential: None,
                            reason: Some("Approval request timed out".to_string()),
                        })
                    }
                    Err(e) => Err(e),
                }
            }
            PermissionLevel::AllowHil2fa => {
                // For now, treat same as AllowHil (2FA stubbed)
                // In future: require YubiKey touch before approval
                match self.request_hil_approval(path, agent_id, reason, PermissionLevel::AllowHil2fa).await {
                    Ok(true) => {
                        self.retrieve_credential(path, permission_result).await
                    }
                    Ok(false) => {
                        Ok(CredentialRequestResult {
                            permission: permission_result,
                            credential: None,
                            reason: Some("Request denied by human operator".to_string()),
                        })
                    }
                    Err(CredentialError::ApprovalTimeout) => {
                        Ok(CredentialRequestResult {
                            permission: permission_result,
                            credential: None,
                            reason: Some("Approval request timed out".to_string()),
                        })
                    }
                    Err(e) => Err(e),
                }
            }
            PermissionLevel::Allow => {
                // Permission granted - retrieve credential immediately
                self.retrieve_credential(path, permission_result).await
            }
        }
    }

    /// Internal helper to retrieve a credential after permission check
    async fn retrieve_credential(
        &self,
        path: &str,
        permission_result: PathPermissionResult,
    ) -> Result<CredentialRequestResult> {
        if self.is_locked().await {
            return Err(CredentialError::VaultLocked);
        }

        let vault = self.vault.read().await;
        
        // Parse the path to find endpoint and credential
        // Expected format: "service://path" or just endpoint ID
        let credential = self.resolve_credential_from_path(&vault, path)?;
        
        Ok(CredentialRequestResult {
            permission: permission_result,
            credential: Some(credential),
            reason: None,
        })
    }

    /// Create a new endpoint
    pub async fn create_endpoint(
        &self,
        name: String,
        endpoint_type: EndpointType,
        base_url: String,
    ) -> Result<Endpoint> {
        if self.is_locked().await {
            return Err(CredentialError::VaultLocked);
        }
        
        let mut vault = self.vault.write().await;
        vault.create_endpoint(name, endpoint_type, base_url)
    }

    /// Store a credential for an endpoint
    pub async fn store_credential(
        &self,
        endpoint_id: Uuid,
        credential_type: CredentialType,
        secret: &[u8],
    ) -> Result<()> {
        if self.is_locked().await {
            return Err(CredentialError::VaultLocked);
        }
        
        let mut vault = self.vault.write().await;
        vault.store_credential(endpoint_id, credential_type, secret)?;
        Ok(())
    }

    /// List all endpoints
    pub async fn list_endpoints(&self) -> Vec<Endpoint> {
        let vault = self.vault.read().await;
        vault.list_endpoints().into_iter().cloned().collect()
    }

    /// Resolve a credential from a path string
    fn resolve_credential_from_path(
        &self,
        vault: &CredentialVault,
        path: &str,
    ) -> Result<Vec<u8>> {
        // Try to parse as "service://..." URI
        if let Some(rest) = path.strip_prefix("saggyclaw://") {
            // saggyclaw://endpoint_name/path
            let parts: Vec<&str> = rest.splitn(2, '/').collect();
            let endpoint_name = parts[0];
            
            // Find endpoint by name
            let endpoints = vault.list_endpoints();
            let endpoint = endpoints
                .iter()
                .find(|e| e.name.to_lowercase() == endpoint_name.to_lowercase())
                .ok_or_else(|| CredentialError::ValidationFailed(
                    format!("endpoint '{}' not found", endpoint_name),
                ))?;
            
            vault.decrypt_credential_for_endpoint(endpoint.id)
        } else if let Ok(uuid) = Uuid::parse_str(path) {
            // Direct UUID reference
            vault.decrypt_credential_for_endpoint(uuid)
        } else {
            // Try as endpoint name
            let endpoints = vault.list_endpoints();
            let endpoint = endpoints
                .iter()
                .find(|e| e.name.to_lowercase() == path.to_lowercase())
                .ok_or_else(|| CredentialError::ValidationFailed(
                    format!("endpoint '{}' not found", path),
                ))?;
            
            vault.decrypt_credential_for_endpoint(endpoint.id)
        }
    }

    /// List all path permissions
    pub async fn list_path_permissions(&self) -> Vec<PathPermission> {
        let perms = self.permissions.read().await;
        perms.list_permissions().to_vec()
    }

    /// Remove a permission by ID
    pub async fn remove_permission(&self, id: Uuid) -> bool {
        let mut perms = self.permissions.write().await;
        perms.remove_permission(id)
    }

    /// Get recent audit entries
    ///
    /// Returns the last `limit` audit entries (most recent first).
    /// If the audit log doesn't exist or can't be read, returns an empty vec.
    pub fn get_audit_entries(&self, limit: usize) -> Vec<AuditEntry> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let audit_path = self.vault_path.join("audit.log");
        
        let file = match File::open(&audit_path) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };
        
        let reader = BufReader::new(file);
        let mut entries: Vec<AuditEntry> = Vec::new();
        
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<AuditEntry>(&line) {
                entries.push(entry);
            }
        }
        
        // Return most recent entries (last N)
        if entries.len() > limit {
            entries.split_off(entries.len() - limit)
        } else {
            entries
        }
    }

    /// Delete an endpoint and its credential
    pub async fn delete_endpoint(&self, endpoint_id: Uuid) -> Result<()> {
        if self.is_locked().await {
            return Err(CredentialError::VaultLocked);
        }
        
        let mut vault = self.vault.write().await;
        vault.delete_endpoint(endpoint_id)
    }
}

impl Clone for CredentialManager {
    fn clone(&self) -> Self {
        Self {
            vault: Arc::clone(&self.vault),
            permissions: Arc::clone(&self.permissions),
            locked: Arc::clone(&self.locked),
            pending_approvals: Arc::clone(&self.pending_approvals),
            hil_notification_tx: Arc::clone(&self.hil_notification_tx),
            hil_timeout_secs: self.hil_timeout_secs,
            vault_path: self.vault_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::generate_salt;
    use tempfile::TempDir;

    fn create_test_manager() -> (CredentialManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let manager = CredentialManager::new(temp_dir.path()).unwrap();
        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_manager_starts_locked() {
        let (manager, _temp) = create_test_manager();
        assert!(manager.is_locked().await);
    }

    #[tokio::test]
    async fn test_manager_unlock_lock() {
        let (manager, _temp) = create_test_manager();
        
        let salt = generate_salt();
        let master_key = MasterKey::derive_from_password("test-password", &salt).unwrap();
        
        manager.unlock(master_key).await.unwrap();
        assert!(!manager.is_locked().await);
        
        manager.lock().await.unwrap();
        assert!(manager.is_locked().await);
    }

    #[tokio::test]
    async fn test_locked_operations_fail() {
        let (manager, _temp) = create_test_manager();
        
        let result = manager
            .create_endpoint(
                "test".to_string(),
                EndpointType::HomeAssistant,
                "http://test".to_string(),
            )
            .await;
        
        assert!(matches!(result, Err(CredentialError::VaultLocked)));
    }

    #[tokio::test]
    async fn test_permission_denied() {
        let (manager, _temp) = create_test_manager();
        
        // Add deny permission (use ** to match multiple path segments)
        manager.add_permission(PathPermission {
            id: Uuid::new_v4(),
            path_pattern: "secret://**".to_string(),
            level: PermissionLevel::Deny,
            description: Some("Deny all secrets".to_string()),
        }).await;
        
        let result = manager
            .request_credential("secret://api/key", "test-agent", "testing")
            .await
            .unwrap();
        
        assert!(result.credential.is_none());
        assert_eq!(result.permission.level, PermissionLevel::Deny);
    }

    #[tokio::test]
    async fn test_permission_hil_required() {
        let (manager, _temp) = create_test_manager();
        
        // Add HIL permission (use ** to match multiple path segments)
        manager.add_permission(PathPermission {
            id: Uuid::new_v4(),
            path_pattern: "bank://**".to_string(),
            level: PermissionLevel::AllowHil,
            description: Some("Require approval for bank credentials".to_string()),
        }).await;
        
        // Use check_permission to avoid blocking on HIL
        let permission = manager.check_permission("bank://account/token").await;
        
        assert_eq!(permission.level, PermissionLevel::AllowHil);
    }

    #[tokio::test]
    async fn test_hil_approval_flow() {
        // Create manager with short timeout for testing
        let temp_dir = TempDir::new().unwrap();
        let manager = CredentialManager::with_hil_timeout(temp_dir.path(), 2).unwrap();
        
        let salt = generate_salt();
        let master_key = MasterKey::derive_from_password("test-password", &salt).unwrap();
        manager.unlock(master_key).await.unwrap();
        
        // Add HIL permission for saggyclaw:// URIs
        manager.add_permission(PathPermission {
            id: Uuid::new_v4(),
            path_pattern: "saggyclaw://hil-test/**".to_string(),
            level: PermissionLevel::AllowHil,
            description: Some("Test HIL".to_string()),
        }).await;
        
        // Create endpoint and credential
        let endpoint = manager
            .create_endpoint(
                "hil-test".to_string(),
                EndpointType::GenericRest,
                "http://test".to_string(),
            )
            .await
            .unwrap();
        
        manager
            .store_credential(endpoint.id, CredentialType::BearerToken, b"secret")
            .await
            .unwrap();
        
        // Subscribe to HIL notifications
        let mut hil_rx = manager.subscribe_hil_notifications().await;
        
        // Spawn a task to auto-approve
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            if let Some(request) = hil_rx.recv().await {
                manager_clone.respond_to_approval(HilApprovalResponse {
                    request_id: request.id,
                    approved: true,
                    resolved_by: "test".to_string(),
                    denial_reason: None,
                }).await.unwrap();
            }
        });
        
        // Request credential - should succeed after approval
        let result = manager
            .request_credential("saggyclaw://hil-test/api", "test-agent", "testing")
            .await
            .unwrap();
        
        assert!(result.credential.is_some());
        assert_eq!(result.credential.unwrap(), b"secret");
    }

    #[tokio::test]
    async fn test_hil_timeout() {
        // Create manager with very short timeout
        let temp_dir = TempDir::new().unwrap();
        let manager = CredentialManager::with_hil_timeout(temp_dir.path(), 1).unwrap();
        
        manager.add_permission(PathPermission {
            id: Uuid::new_v4(),
            path_pattern: "timeout-test://**".to_string(),
            level: PermissionLevel::AllowHil,
            description: None,
        }).await;
        
        // Request without responding - should timeout
        let result = manager
            .request_credential("timeout-test://api", "test-agent", "testing")
            .await
            .unwrap();
        
        assert!(result.credential.is_none());
        assert!(result.reason.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn test_full_credential_flow() {
        let (manager, _temp) = create_test_manager();
        
        // Unlock vault
        let salt = generate_salt();
        let master_key = MasterKey::derive_from_password("test-password", &salt).unwrap();
        manager.unlock(master_key).await.unwrap();
        
        // Add allow permission
        manager.add_permission(PathPermission {
            id: Uuid::new_v4(),
            path_pattern: "saggyclaw://homeassistant/**".to_string(),
            level: PermissionLevel::Allow,
            description: Some("Allow Home Assistant".to_string()),
        }).await;
        
        // Create endpoint
        let endpoint = manager
            .create_endpoint(
                "homeassistant".to_string(),
                EndpointType::HomeAssistant,
                "http://homeassistant:8123".to_string(),
            )
            .await
            .unwrap();
        
        // Store credential
        let secret = b"my-api-token";
        manager
            .store_credential(endpoint.id, CredentialType::BearerToken, secret)
            .await
            .unwrap();
        
        // Request credential by name (via saggyclaw:// URI)
        let result = manager
            .request_credential("saggyclaw://homeassistant/api", "test-agent", "testing")
            .await
            .unwrap();
        
        assert!(result.credential.is_some());
        assert_eq!(result.credential.unwrap(), secret);
        assert_eq!(result.permission.level, PermissionLevel::Allow);
    }

    #[tokio::test]
    async fn test_manager_clone() {
        let (manager, _temp) = create_test_manager();
        
        let salt = generate_salt();
        let master_key = MasterKey::derive_from_password("test-password", &salt).unwrap();
        manager.unlock(master_key).await.unwrap();
        
        // Clone should share state
        let manager2 = manager.clone();
        assert!(!manager2.is_locked().await);
        
        manager.lock().await.unwrap();
        assert!(manager2.is_locked().await);
    }
}
