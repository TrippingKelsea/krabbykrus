use redb::TableDefinition;

pub const VAULT_META: TableDefinition<'static, &str, &str> = TableDefinition::new("vault_meta");
pub const ENDPOINTS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("endpoints");
pub const CREDENTIALS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("credentials");
pub const PERMISSIONS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("permissions");
pub const KV_STORE: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("kv_store");
pub const SESSIONS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("sessions");
pub const SESSION_MESSAGES: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("session_messages");
pub const CRON_JOBS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("cron_jobs");
pub const ROUTE_BINDINGS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("route_bindings");
pub const PKI_INDEX: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("pki_index");
pub const AGENTS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("agents");
pub const AGENT_DOCUMENTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("agent_documents");
pub const AGENT_OBJECTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("agent_objects");
pub const TOPOLOGY_NODES: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("topology_nodes");
pub const TOPOLOGY_EDGES: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("topology_edges");
pub const TOPOLOGY_EDGES_FROM: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("topology_edges_from");
pub const TOPOLOGY_EDGES_TO: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("topology_edges_to");
pub const ZONES: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("zones");
pub const ZONE_MEMBERS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("zone_members");
pub const BLACKBOARDS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("blackboards");
pub const BLACKBOARD_ACL: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("blackboard_acl");
pub const OWNERSHIP_EVENTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("ownership_events");
pub const AGENT_VDISKS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("agent_vdisks");
pub const REPLICATION_META: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("replication_meta");
pub const NODE_KEYS: TableDefinition<'static, &str, &[u8]> = TableDefinition::new("node_keys");
pub const VAULT_OBJECTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("vault_objects");
pub const VAULT_PROVIDER_GRANTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("vault_provider_grants");
pub const VAULT_NODE_GRANTS: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("vault_node_grants");
pub const VAULT_POLICIES: TableDefinition<'static, &str, &[u8]> =
    TableDefinition::new("vault_policies");
