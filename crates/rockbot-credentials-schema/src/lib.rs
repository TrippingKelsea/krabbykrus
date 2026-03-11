//! Credential schema types for RockBot provider plugins.
//!
//! This lightweight crate defines the types that any provider (LLM, Channel, Tool)
//! uses to describe its credential requirements. UIs use these schemas to build
//! dynamic configuration forms.

use serde::{Deserialize, Serialize};

/// Credential schema — describes what credentials a provider needs.
/// Providers return this so that UIs can build dynamic configuration forms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSchema {
    /// Provider identifier (e.g. "bedrock", "anthropic", "discord")
    pub provider_id: String,
    /// Human-readable provider name
    pub provider_name: String,
    /// Category for grouping in UIs
    pub category: CredentialCategory,
    /// Available authentication methods (first is default)
    pub auth_methods: Vec<AuthMethod>,
}

/// Category for credential grouping
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialCategory {
    Model,
    Communication,
    Tool,
}

/// An authentication method with its required fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMethod {
    /// Machine identifier (e.g. "api_key", "oauth", "aws_credentials")
    pub id: String,
    /// Human-readable label (e.g. "API Key", "OAuth (Claude Code)")
    pub label: String,
    /// Fields the user must fill in
    pub fields: Vec<CredentialField>,
    /// Hint text shown to the user
    pub hint: Option<String>,
    /// Documentation URL
    pub docs_url: Option<String>,
}

/// A single field in a credential form
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialField {
    /// Field identifier (e.g. "api_key", "region", "base_url")
    pub id: String,
    /// Display label
    pub label: String,
    /// Whether this field contains a secret (should be masked in UI)
    pub secret: bool,
    /// Default value (if any)
    pub default: Option<String>,
    /// Placeholder text
    pub placeholder: Option<String>,
    /// Whether this field is required
    pub required: bool,
    /// Environment variable that can provide this value
    pub env_var: Option<String>,
}
