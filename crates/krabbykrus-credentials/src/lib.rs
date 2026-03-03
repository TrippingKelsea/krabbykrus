//! Krabbykrus Credentials - Secure credential storage for AI agents
//!
//! This crate provides secure credential management functionality ported from
//! SAGgyClaw. It includes:
//!
//! - **Types**: Core data structures (Endpoint, Credential, Permission, etc.)
//! - **Crypto**: Password-derived encryption and secure credential handling (AES-256-GCM)
//! - **Audit**: Hash-chained audit logging for tamper-evident records
//! - **Storage**: Encrypted credential vault operations
//! - **Permissions**: Permission evaluation for request authorization
//!
//! # Design Principles
//!
//! - Credentials never cross the agent boundary
//! - All operations are logged in a tamper-evident audit trail
//! - Human-in-loop approval for sensitive operations
//! - Local-first: no cloud dependencies
//!
//! # Permission Levels
//!
//! - **Allow**: Execute immediately without human involvement
//! - **AllowHIL**: Human-in-loop approval required
//! - **AllowHIL2FA**: Human-in-loop with YubiKey/2FA verification
//! - **Deny**: Reject request and log attempt
//!
//! # Example
//!
//! ```no_run
//! use krabbykrus_credentials::{CredentialVault, MasterKey, CredentialType, EndpointType};
//! use krabbykrus_credentials::crypto::generate_salt;
//!
//! // Create or open a vault
//! let mut vault = CredentialVault::open("/path/to/credentials").unwrap();
//!
//! // Unlock with a master key derived from password
//! let salt = generate_salt();
//! let master_key = MasterKey::derive_from_password("my-password", &salt).unwrap();
//! vault.unlock(master_key);
//!
//! // Create an endpoint
//! let endpoint = vault.create_endpoint(
//!     "Home Assistant".to_string(),
//!     EndpointType::HomeAssistant,
//!     "http://homeassistant:8123".to_string(),
//! ).unwrap();
//!
//! // Store a credential
//! let token = b"my-api-token";
//! vault.store_credential(
//!     endpoint.id,
//!     CredentialType::BearerToken,
//!     token,
//! ).unwrap();
//!
//! // Later: decrypt the credential
//! let secret = vault.decrypt_credential_for_endpoint(endpoint.id).unwrap();
//! ```

pub mod audit;
pub mod crypto;
pub mod error;
pub mod manager;
pub mod permissions;
pub mod storage;
pub mod types;

// Re-export commonly used types at crate root
pub use error::{CredentialError, ErrorCode, Result};
pub use types::{
    ApprovalRequest, ApprovalStatus, AuditEntry, Credential, CredentialType, Endpoint,
    EndpointType, Hash256, HttpMethod, Permission, PermissionLevel, ResultStatus, ZERO_HASH,
};

// Re-export hex utilities
pub use types::{hex_decode, hex_decode_hash, hex_encode, hex_encode_hash};

// Re-export storage types
pub use storage::{CredentialVault, VaultMeta, UnlockMethod};

// Re-export crypto types and functions
pub use crypto::{generate_salt, MasterKey};

// Re-export permissions types
pub use permissions::{PermissionEvaluator, PermissionResult};

// Re-export audit types
pub use audit::{AuditEntryBuilder, AuditLog, VerificationResult};

// Re-export manager types
pub use manager::{
    CredentialManager, CredentialRequestResult, PathPermission, PathPermissionResult,
    HilApprovalRequest, HilApprovalResponse, HilNotificationReceiver, HilNotificationSender,
};
