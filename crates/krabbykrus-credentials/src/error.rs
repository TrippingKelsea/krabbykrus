//! Error types for krabbykrus-credentials.
//!
//! Uses `thiserror` for ergonomic error handling with proper Display implementations.
//! Designed to integrate with krabbykrus-core's error hierarchy.

use thiserror::Error;
use uuid::Uuid;

/// Main error type for credential operations.
#[derive(Debug, Error)]
pub enum CredentialError {
    // Vault errors
    #[error("vault is locked; unlock with master key first")]
    VaultLocked,

    #[error("vault has not been initialized; run 'krabbykrus credentials init' first")]
    VaultNotInitialized,

    #[error("vault already exists at this location")]
    VaultAlreadyExists,

    #[error("invalid password")]
    InvalidPassword,

    #[error("YubiKey not found or not connected")]
    YubikeyNotFound,

    #[error("YubiKey touch required to proceed")]
    YubikeyTouchRequired,

    #[error("credential not found: {0}")]
    CredentialNotFound(Uuid),

    #[error("failed to decrypt credential data")]
    DecryptionFailed,

    #[error("failed to encrypt credential data")]
    EncryptionFailed,

    #[error("invalid master key or corrupted data")]
    InvalidMasterKey,

    // Permission errors
    #[error("endpoint not found: {0}")]
    EndpointNotFound(Uuid),

    #[error("permission denied for path '{path}': requires {required} permission")]
    PermissionDenied { path: String, required: String },

    #[error("approval request timed out")]
    ApprovalTimeout,

    #[error("approval request was denied by human operator")]
    ApprovalDenied,

    // Validation errors
    #[error("validation failed: {0}")]
    ValidationFailed(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("invalid HTTP method: {0}")]
    InvalidMethod(String),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    // Audit errors
    #[error("failed to write to audit log: {0}")]
    AuditWriteFailed(String),

    #[error("audit log integrity check failed: chain broken at sequence {0}")]
    AuditChainBroken(u64),

    #[error("failed to read audit log: {0}")]
    AuditReadFailed(String),

    // Internal errors
    #[error("internal error: {0}")]
    Internal(String),

    // Serialization errors
    #[error("serialization error: {0}")]
    SerializationError(String),

    #[error("deserialization error: {0}")]
    DeserializationError(String),

    // IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type alias for credential operations.
pub type Result<T> = std::result::Result<T, CredentialError>;

/// Error codes for structured error responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    GeneralError,
    InvalidArguments,
    EndpointNotFound,
    CredentialNotFound,
    PermissionDenied,
    ApprovalTimeout,
    ApprovalDenied,
    AuditError,
    VaultLocked,
}

impl ErrorCode {
    /// Returns a numeric code for the error.
    pub fn code(&self) -> u32 {
        match self {
            ErrorCode::GeneralError => 1000,
            ErrorCode::InvalidArguments => 1001,
            ErrorCode::EndpointNotFound => 1002,
            ErrorCode::CredentialNotFound => 1003,
            ErrorCode::PermissionDenied => 1004,
            ErrorCode::ApprovalTimeout => 1005,
            ErrorCode::ApprovalDenied => 1006,
            ErrorCode::AuditError => 1007,
            ErrorCode::VaultLocked => 1008,
        }
    }

    /// Returns the string code for JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::GeneralError => "CREDENTIAL_GENERAL_ERROR",
            ErrorCode::InvalidArguments => "CREDENTIAL_INVALID_ARGUMENTS",
            ErrorCode::EndpointNotFound => "CREDENTIAL_ENDPOINT_NOT_FOUND",
            ErrorCode::CredentialNotFound => "CREDENTIAL_NOT_FOUND",
            ErrorCode::PermissionDenied => "CREDENTIAL_PERMISSION_DENIED",
            ErrorCode::ApprovalTimeout => "CREDENTIAL_APPROVAL_TIMEOUT",
            ErrorCode::ApprovalDenied => "CREDENTIAL_APPROVAL_DENIED",
            ErrorCode::AuditError => "CREDENTIAL_AUDIT_ERROR",
            ErrorCode::VaultLocked => "CREDENTIAL_VAULT_LOCKED",
        }
    }

    /// Returns the error category.
    pub fn category(&self) -> &'static str {
        match self {
            ErrorCode::GeneralError | ErrorCode::InvalidArguments => "client",
            ErrorCode::EndpointNotFound | ErrorCode::CredentialNotFound => "lookup",
            ErrorCode::PermissionDenied | ErrorCode::ApprovalTimeout | ErrorCode::ApprovalDenied => {
                "permission"
            }
            ErrorCode::AuditError => "audit",
            ErrorCode::VaultLocked => "vault",
        }
    }
}

impl From<&CredentialError> for ErrorCode {
    fn from(error: &CredentialError) -> Self {
        match error {
            CredentialError::VaultLocked
            | CredentialError::VaultNotInitialized
            | CredentialError::VaultAlreadyExists
            | CredentialError::InvalidPassword
            | CredentialError::InvalidMasterKey
            | CredentialError::EncryptionFailed
            | CredentialError::DecryptionFailed => ErrorCode::VaultLocked,
            CredentialError::YubikeyNotFound | CredentialError::YubikeyTouchRequired => {
                ErrorCode::GeneralError
            }
            CredentialError::CredentialNotFound(_) => ErrorCode::CredentialNotFound,
            CredentialError::EndpointNotFound(_) => ErrorCode::EndpointNotFound,
            CredentialError::PermissionDenied { .. } => ErrorCode::PermissionDenied,
            CredentialError::ApprovalTimeout => ErrorCode::ApprovalTimeout,
            CredentialError::ApprovalDenied => ErrorCode::ApprovalDenied,
            CredentialError::ValidationFailed(_)
            | CredentialError::InvalidPath(_)
            | CredentialError::InvalidMethod(_)
            | CredentialError::InvalidUrl(_) => ErrorCode::InvalidArguments,
            CredentialError::AuditWriteFailed(_)
            | CredentialError::AuditChainBroken(_)
            | CredentialError::AuditReadFailed(_) => ErrorCode::AuditError,
            CredentialError::Internal(_)
            | CredentialError::SerializationError(_)
            | CredentialError::DeserializationError(_)
            | CredentialError::Io(_) => ErrorCode::GeneralError,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let error = CredentialError::PermissionDenied {
            path: "/api/services/light/turn_on".to_string(),
            required: "AllowHIL".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "permission denied for path '/api/services/light/turn_on': requires AllowHIL permission"
        );
    }

    #[test]
    fn test_error_to_code() {
        let error = CredentialError::EndpointNotFound(Uuid::nil());
        let code: ErrorCode = (&error).into();
        assert_eq!(code, ErrorCode::EndpointNotFound);
    }

    #[test]
    fn test_error_codes() {
        assert_eq!(ErrorCode::GeneralError.code(), 1000);
        assert_eq!(ErrorCode::VaultLocked.code(), 1008);
    }
}
