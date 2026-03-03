//! Core data types for krabbykrus-credentials.
//!
//! These types represent the fundamental entities in the credential system:
//! - Endpoints: External services that can be accessed
//! - Credentials: Authentication data for endpoints
//! - Permissions: Access control rules
//! - Audit entries: Hash-chained log records
//! - Approval requests: Human-in-loop workflow items

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CredentialError;

/// Represents a configured external service endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    /// Unique identifier for this endpoint.
    pub id: Uuid,
    /// Human-readable name (e.g., "Home Assistant").
    pub name: String,
    /// Type of endpoint, determines adapter and known API patterns.
    pub endpoint_type: EndpointType,
    /// Base URL for the service (e.g., "http://homeassistant:8123").
    pub base_url: String,
    /// Reference to credential in vault.
    pub credential_id: Uuid,
    /// When this endpoint was created.
    pub created_at: DateTime<Utc>,
    /// When this endpoint was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Type of endpoint, determines which adapter to use.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EndpointType {
    /// Home Assistant smart home platform.
    HomeAssistant,
    /// Gmail API.
    Gmail,
    /// Spotify API.
    Spotify,
    /// Generic REST API with configurable auth.
    GenericRest,
    /// Generic OAuth2 service.
    GenericOAuth2,
}

impl EndpointType {
    /// Returns the string representation for this endpoint type.
    pub fn as_str(&self) -> &'static str {
        match self {
            EndpointType::HomeAssistant => "home_assistant",
            EndpointType::Gmail => "gmail",
            EndpointType::Spotify => "spotify",
            EndpointType::GenericRest => "generic_rest",
            EndpointType::GenericOAuth2 => "generic_oauth2",
        }
    }
}

/// Stored credential for an endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// Unique identifier for this credential.
    pub id: Uuid,
    /// Associated endpoint (1:1 relationship).
    pub endpoint_id: Uuid,
    /// Type of credential (determines how to use it).
    pub credential_type: CredentialType,
    /// Encrypted credential data (AES-256-GCM), hex-encoded.
    pub encrypted_data: String,
    /// Nonce used for AES-GCM encryption (12 bytes), hex-encoded.
    pub nonce: String,
    /// When this credential was created.
    pub created_at: DateTime<Utc>,
    /// When this credential was last rotated.
    pub rotated_at: Option<DateTime<Utc>>,
}

impl Credential {
    /// Gets the encrypted data as bytes.
    pub fn encrypted_data_bytes(&self) -> Result<Vec<u8>, CredentialError> {
        hex_decode(&self.encrypted_data).map_err(CredentialError::DeserializationError)
    }

    /// Gets the nonce as bytes.
    pub fn nonce_bytes(&self) -> Result<Vec<u8>, CredentialError> {
        hex_decode(&self.nonce).map_err(CredentialError::DeserializationError)
    }

    /// Sets the encrypted data from bytes.
    pub fn set_encrypted_data(&mut self, data: &[u8]) {
        self.encrypted_data = hex_encode(data);
    }

    /// Sets the nonce from bytes.
    pub fn set_nonce(&mut self, nonce: &[u8]) {
        self.nonce = hex_encode(nonce);
    }
}

/// Type of credential, determines how to apply authentication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialType {
    /// Bearer token in Authorization header.
    BearerToken,
    /// Basic auth with username; password in encrypted_data.
    BasicAuth {
        /// Username for basic auth.
        username: String,
    },
    /// API key in custom header.
    ApiKey {
        /// Header name for the API key.
        header_name: String,
    },
    /// OAuth 2.0 credentials.
    OAuth2 {
        /// OAuth client ID.
        client_id: String,
        /// Token endpoint URL.
        token_url: String,
        /// Authorized scopes.
        scopes: Vec<String>,
    },
    /// Client certificate authentication.
    ClientCertificate,
    /// FIDO2/Passkey credential.
    Passkey,
}

/// Permission rule for an endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Permission {
    /// Unique identifier for this permission rule.
    pub id: Uuid,
    /// Endpoint this permission applies to.
    pub endpoint_id: Uuid,
    /// Path pattern (glob or regex).
    pub path_pattern: String,
    /// HTTP method restriction (None = all methods).
    pub method: Option<HttpMethod>,
    /// Permission level for matching requests.
    pub permission_level: PermissionLevel,
    /// When this permission was created.
    pub created_at: DateTime<Utc>,
}

/// HTTP methods supported by the system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    /// Returns the string representation of this HTTP method.
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }

    /// Parses an HTTP method from a string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "PUT" => Some(HttpMethod::Put),
            "DELETE" => Some(HttpMethod::Delete),
            "PATCH" => Some(HttpMethod::Patch),
            "HEAD" => Some(HttpMethod::Head),
            "OPTIONS" => Some(HttpMethod::Options),
            _ => None,
        }
    }
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Permission level determining how a request is handled.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Execute immediately without human involvement.
    Allow,
    /// Human-in-loop: require approval via TUI.
    AllowHil,
    /// Human-in-loop with YubiKey touch required.
    AllowHil2fa,
    /// Reject request and log attempt.
    Deny,
}

impl PermissionLevel {
    /// Returns the string representation of this permission level.
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionLevel::Allow => "allow",
            PermissionLevel::AllowHil => "allow_hil",
            PermissionLevel::AllowHil2fa => "allow_hil_2fa",
            PermissionLevel::Deny => "deny",
        }
    }

    /// Returns whether this permission level requires human approval.
    pub fn requires_approval(&self) -> bool {
        matches!(self, PermissionLevel::AllowHil | PermissionLevel::AllowHil2fa)
    }

    /// Returns whether this permission level requires YubiKey 2FA.
    pub fn requires_2fa(&self) -> bool {
        matches!(self, PermissionLevel::AllowHil2fa)
    }

    /// Returns whether this permission level allows execution.
    pub fn allows_execution(&self) -> bool {
        !matches!(self, PermissionLevel::Deny)
    }
}

impl std::fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PermissionLevel::Allow => write!(f, "Allow"),
            PermissionLevel::AllowHil => write!(f, "AllowHIL"),
            PermissionLevel::AllowHil2fa => write!(f, "AllowHIL2FA"),
            PermissionLevel::Deny => write!(f, "Deny"),
        }
    }
}

/// Hash type for SHA-256 hashes (32 bytes).
pub type Hash256 = [u8; 32];

/// Zero hash constant for the genesis audit entry.
pub const ZERO_HASH: Hash256 = [0u8; 32];

/// Entry in the hash-chained audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonic sequence number.
    pub sequence: u64,
    /// Timestamp of the event.
    pub timestamp: DateTime<Utc>,
    /// Unique request identifier.
    pub request_id: Uuid,
    /// Source of the request (e.g., "agent:hex", "tui:kelsea").
    pub source: String,
    /// Target endpoint.
    pub endpoint_id: Uuid,
    /// HTTP method used.
    pub method: HttpMethod,
    /// Request path.
    pub path: String,
    /// SHA-256 hash of request body/parameters (hex-encoded).
    pub parameters_hash: String,
    /// Permission level that was applied.
    pub permission_level: PermissionLevel,
    /// Reference to approval if HIL was required.
    pub approval_id: Option<Uuid>,
    /// Result status of the operation.
    pub result_status: ResultStatus,
    /// SHA-256 hash of response body (hex-encoded).
    pub result_hash: String,
    /// Error message if operation failed.
    pub error_message: Option<String>,
    /// Hash of the previous audit entry (hex-encoded).
    pub previous_hash: String,
    /// Hash of this entry (hex-encoded).
    pub entry_hash: String,
}

impl AuditEntry {
    /// Gets the parameters hash as bytes.
    pub fn parameters_hash_bytes(&self) -> Result<Hash256, String> {
        hex_decode_hash(&self.parameters_hash)
    }

    /// Gets the result hash as bytes.
    pub fn result_hash_bytes(&self) -> Result<Hash256, String> {
        hex_decode_hash(&self.result_hash)
    }

    /// Gets the previous hash as bytes.
    pub fn previous_hash_bytes(&self) -> Result<Hash256, String> {
        hex_decode_hash(&self.previous_hash)
    }

    /// Gets the entry hash as bytes.
    pub fn entry_hash_bytes(&self) -> Result<Hash256, String> {
        hex_decode_hash(&self.entry_hash)
    }
}

/// Result status of an operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    /// Operation completed successfully.
    Success,
    /// Client error (4xx HTTP status).
    ClientError,
    /// Server error (5xx HTTP status).
    ServerError,
    /// Request timed out.
    Timeout,
    /// Permission was denied.
    Denied,
    /// Awaiting human approval.
    PendingApproval,
    /// HIL request was cancelled.
    Cancelled,
}

impl ResultStatus {
    /// Returns the string representation of this result status.
    pub fn as_str(&self) -> &'static str {
        match self {
            ResultStatus::Success => "success",
            ResultStatus::ClientError => "client_error",
            ResultStatus::ServerError => "server_error",
            ResultStatus::Timeout => "timeout",
            ResultStatus::Denied => "denied",
            ResultStatus::PendingApproval => "pending_approval",
            ResultStatus::Cancelled => "cancelled",
        }
    }
}

impl std::fmt::Display for ResultStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Human-in-loop approval request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Unique identifier for this approval request.
    pub id: Uuid,
    /// Associated audit entry.
    pub audit_entry_id: Uuid,
    /// Target endpoint.
    pub endpoint_id: Uuid,
    /// HTTP method of the request.
    pub method: HttpMethod,
    /// Request path.
    pub path: String,
    /// Truncated, sanitized preview of request body.
    pub body_preview: Option<String>,
    /// Required permission level.
    pub permission_level: PermissionLevel,
    /// Current approval status.
    pub status: ApprovalStatus,
    /// When this approval request was created.
    pub created_at: DateTime<Utc>,
    /// When this approval was resolved.
    pub resolved_at: Option<DateTime<Utc>>,
    /// Who resolved this approval.
    pub resolved_by: Option<String>,
    /// Whether YubiKey verification was completed.
    pub yubikey_verified: bool,
}

/// Status of an approval request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    /// Awaiting human decision.
    Pending,
    /// Approved by human.
    Approved,
    /// Denied by human.
    Denied,
    /// Timed out waiting for approval.
    Expired,
}

impl ApprovalStatus {
    /// Returns the string representation of this approval status.
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Denied => "denied",
            ApprovalStatus::Expired => "expired",
        }
    }
}

impl std::fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Encodes bytes as a lowercase hex string.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(s, "{:02x}", byte).unwrap();
    }
    s
}

/// Decodes a hex string to bytes.
pub fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex string must have even length".to_string());
    }
    let mut bytes = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16)
            .map_err(|_| format!("invalid hex character at position {}", i))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

/// Decodes a hex string to a Hash256.
pub fn hex_decode_hash(s: &str) -> Result<Hash256, String> {
    let bytes = hex_decode(s)?;
    if bytes.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

/// Encodes a Hash256 as a lowercase hex string.
pub fn hex_encode_hash(hash: &Hash256) -> String {
    hex_encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_roundtrip() {
        let original = vec![0x00, 0x0f, 0xf0, 0xff, 0xab, 0xcd];
        let encoded = hex_encode(&original);
        assert_eq!(encoded, "000ff0ffabcd");
        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_hex_hash_roundtrip() {
        let hash: Hash256 = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b,
            0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let encoded = hex_encode_hash(&hash);
        let decoded = hex_decode_hash(&encoded).unwrap();
        assert_eq!(decoded, hash);
    }

    #[test]
    fn test_hex_decode_invalid() {
        assert!(hex_decode("0").is_err()); // odd length
        assert!(hex_decode("gg").is_err()); // invalid chars
        assert!(hex_decode_hash("00").is_err()); // wrong length
    }

    #[test]
    fn test_http_method_roundtrip() {
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Delete,
            HttpMethod::Patch,
        ] {
            assert_eq!(HttpMethod::from_str(method.as_str()), Some(method));
        }
    }

    #[test]
    fn test_permission_level_properties() {
        assert!(!PermissionLevel::Allow.requires_approval());
        assert!(PermissionLevel::AllowHil.requires_approval());
        assert!(PermissionLevel::AllowHil2fa.requires_approval());
        assert!(!PermissionLevel::Deny.requires_approval());

        assert!(!PermissionLevel::Allow.requires_2fa());
        assert!(!PermissionLevel::AllowHil.requires_2fa());
        assert!(PermissionLevel::AllowHil2fa.requires_2fa());
        assert!(!PermissionLevel::Deny.requires_2fa());

        assert!(PermissionLevel::Allow.allows_execution());
        assert!(PermissionLevel::AllowHil.allows_execution());
        assert!(PermissionLevel::AllowHil2fa.allows_execution());
        assert!(!PermissionLevel::Deny.allows_execution());
    }

    #[test]
    fn test_endpoint_serialization() {
        let endpoint = Endpoint {
            id: Uuid::nil(),
            name: "Test".to_string(),
            endpoint_type: EndpointType::HomeAssistant,
            base_url: "http://localhost:8123".to_string(),
            credential_id: Uuid::nil(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&endpoint).unwrap();
        let parsed: Endpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(endpoint, parsed);
    }

    #[test]
    fn test_credential_type_serialization() {
        let cred_type = CredentialType::OAuth2 {
            client_id: "test-client".to_string(),
            token_url: "https://oauth.example.com/token".to_string(),
            scopes: vec!["read".to_string(), "write".to_string()],
        };
        let json = serde_json::to_string(&cred_type).unwrap();
        let parsed: CredentialType = serde_json::from_str(&json).unwrap();
        assert_eq!(cred_type, parsed);
    }
}
