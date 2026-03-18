use redb::TableHandle;

use crate::tables;

/// Replication priority for a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Replicate immediately on every write.
    Eager,
    /// Replicate on a best-effort / periodic basis.
    Eventual,
    /// Never replicate; node-local only.
    LocalOnly,
}

/// Returns the sync policy for a given table name.
pub fn policy_for(table_name: &str) -> SyncPolicy {
    match table_name {
        n if n == tables::CREDENTIALS.name() => SyncPolicy::Eager,
        n if n == tables::VAULT_META.name() => SyncPolicy::Eager,
        n if n == tables::PERMISSIONS.name() => SyncPolicy::Eager,
        n if n == tables::PKI_INDEX.name() => SyncPolicy::Eager,
        n if n == tables::ENDPOINTS.name() => SyncPolicy::Eventual,
        n if n == tables::ROUTE_BINDINGS.name() => SyncPolicy::Eventual,
        n if n == tables::KV_STORE.name() => SyncPolicy::Eventual,
        n if n == tables::SESSIONS.name() => SyncPolicy::Eventual,
        n if n == tables::SESSION_MESSAGES.name() => SyncPolicy::Eventual,
        n if n == tables::CRON_JOBS.name() => SyncPolicy::Eventual,
        n if n == tables::AGENTS.name() => SyncPolicy::Eventual,
        n if n == tables::NODE_KEYS.name() => SyncPolicy::Eager,
        n if n == tables::VAULT_OBJECTS.name() => SyncPolicy::Eager,
        n if n == tables::VAULT_PROVIDER_GRANTS.name() => SyncPolicy::Eager,
        n if n == tables::VAULT_NODE_GRANTS.name() => SyncPolicy::Eager,
        n if n == tables::VAULT_POLICIES.name() => SyncPolicy::Eager,
        _ => SyncPolicy::LocalOnly,
    }
}

/// Returns all table names and their policies.
pub fn all_policies() -> Vec<(&'static str, SyncPolicy)> {
    vec![
        (tables::VAULT_META.name(), SyncPolicy::Eager),
        (tables::CREDENTIALS.name(), SyncPolicy::Eager),
        (tables::PERMISSIONS.name(), SyncPolicy::Eager),
        (tables::PKI_INDEX.name(), SyncPolicy::Eager),
        (tables::ENDPOINTS.name(), SyncPolicy::Eventual),
        (tables::ROUTE_BINDINGS.name(), SyncPolicy::Eventual),
        (tables::KV_STORE.name(), SyncPolicy::Eventual),
        (tables::SESSIONS.name(), SyncPolicy::Eventual),
        (tables::SESSION_MESSAGES.name(), SyncPolicy::Eventual),
        (tables::CRON_JOBS.name(), SyncPolicy::Eventual),
        (tables::AGENTS.name(), SyncPolicy::Eventual),
        (tables::NODE_KEYS.name(), SyncPolicy::Eager),
        (tables::VAULT_OBJECTS.name(), SyncPolicy::Eager),
        (tables::VAULT_PROVIDER_GRANTS.name(), SyncPolicy::Eager),
        (tables::VAULT_NODE_GRANTS.name(), SyncPolicy::Eager),
        (tables::VAULT_POLICIES.name(), SyncPolicy::Eager),
    ]
}

/// Returns only the table names that should be replicated (Eager or Eventual).
pub fn replicated_tables() -> Vec<(&'static str, SyncPolicy)> {
    all_policies()
        .into_iter()
        .filter(|(_, p)| *p != SyncPolicy::LocalOnly)
        .collect()
}
