//! Unified embedded storage layer for RockBot.
//!
//! Wraps [redb](https://docs.rs/redb) with optional ChaCha20 encryption
//! and (behind the `replication` feature) OpenRaft-based multi-node
//! replication.

pub mod encrypted_backend;
pub mod sync;
pub mod tables;

#[cfg(feature = "replication")]
pub mod raft;

pub use redb::TableDefinition;

use encrypted_backend::EncryptedBackend;
use redb::{Database, ReadableTable};
use rockbot_vdisk::VolumeBackend;
use std::path::Path;

/// The unified embedded store.
///
/// Open with [`Store::open`] (plaintext) or [`Store::open_encrypted`]
/// (ChaCha20-encrypted file).
pub struct Store {
    db: Database,
}

pub(crate) type BytesTableDefinition = TableDefinition<'static, &'static str, &'static [u8]>;

impl Store {
    pub const DEFAULT_DATA_FILE: &str = "rockbot.data";

    /// Open a plaintext redb database at `path`.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db = Database::create(path)?;
        let store = Self { db };
        store.initialize_tables()?;
        Ok(store)
    }

    /// Open an encrypted redb database at `path` using the given 32-byte key.
    pub fn open_encrypted(path: &Path, key: [u8; 32]) -> anyhow::Result<Self> {
        let backend = EncryptedBackend::open(path, key)?;
        let db = Database::builder().create_with_backend(backend)?;
        let store = Self { db };
        store.initialize_tables()?;
        Ok(store)
    }

    /// Open a store using encrypted storage when a key is available, otherwise
    /// use the plaintext backend.
    pub fn open_with_optional_key(path: &Path, key: Option<[u8; 32]>) -> anyhow::Result<Self> {
        match key {
            Some(key) => Self::open_encrypted(path, key),
            None => Self::open(path),
        }
    }

    /// Open a named virtual volume within a shared `rockbot.data` container.
    pub fn open_volume(
        disk_path: &Path,
        volume_name: &str,
        capacity: u64,
        key: Option<[u8; 32]>,
    ) -> anyhow::Result<Self> {
        let backend = VolumeBackend::open(disk_path, volume_name, capacity, key)?;
        let db = Database::builder().create_with_backend(backend)?;
        let store = Self { db };
        store.initialize_tables()?;
        Ok(store)
    }

    pub fn default_disk_path(base_dir: &Path) -> std::path::PathBuf {
        base_dir.join(Self::DEFAULT_DATA_FILE)
    }

    fn initialize_tables(&self) -> anyhow::Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let _ = write_txn.open_table(tables::ENDPOINTS)?;
            let _ = write_txn.open_table(tables::CREDENTIALS)?;
            let _ = write_txn.open_table(tables::PERMISSIONS)?;
            let _ = write_txn.open_table(tables::KV_STORE)?;
            let _ = write_txn.open_table(tables::SESSIONS)?;
            let _ = write_txn.open_table(tables::SESSION_MESSAGES)?;
            let _ = write_txn.open_table(tables::CRON_JOBS)?;
            let _ = write_txn.open_table(tables::ROUTE_BINDINGS)?;
            let _ = write_txn.open_table(tables::PKI_INDEX)?;
            let _ = write_txn.open_table(tables::AGENTS)?;
            let _ = write_txn.open_table(tables::AGENT_DOCUMENTS)?;
            let _ = write_txn.open_table(tables::AGENT_OBJECTS)?;
            let _ = write_txn.open_table(tables::TOPOLOGY_NODES)?;
            let _ = write_txn.open_table(tables::TOPOLOGY_EDGES)?;
            let _ = write_txn.open_table(tables::TOPOLOGY_EDGES_FROM)?;
            let _ = write_txn.open_table(tables::TOPOLOGY_EDGES_TO)?;
            let _ = write_txn.open_table(tables::ZONES)?;
            let _ = write_txn.open_table(tables::ZONE_MEMBERS)?;
            let _ = write_txn.open_table(tables::BLACKBOARDS)?;
            let _ = write_txn.open_table(tables::BLACKBOARD_ACL)?;
            let _ = write_txn.open_table(tables::OWNERSHIP_EVENTS)?;
            let _ = write_txn.open_table(tables::AGENT_VDISKS)?;
            let _ = write_txn.open_table(tables::REPLICATION_META)?;
            let _ = write_txn.open_table(tables::NODE_KEYS)?;
            let _ = write_txn.open_table(tables::VAULT_OBJECTS)?;
            let _ = write_txn.open_table(tables::VAULT_PROVIDER_GRANTS)?;
            let _ = write_txn.open_table(tables::VAULT_NODE_GRANTS)?;
            let _ = write_txn.open_table(tables::VAULT_POLICIES)?;
            let _ = write_txn.open_table(tables::VAULT_META)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Generic &str / &[u8] table methods
    // -------------------------------------------------------------------------

    /// Insert or update `key` → `value` in `table`.
    pub fn put(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        key: &str,
        value: &[u8],
    ) -> anyhow::Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut t = write_txn.open_table(table)?;
            t.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Retrieve the value for `key` from `table`, returning `None` if absent.
    pub fn get(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        key: &str,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(table)?;
        match t.get(key)? {
            Some(guard) => Ok(Some(guard.value().to_vec())),
            None => Ok(None),
        }
    }

    /// Delete `key` from `table`. Returns `true` if the key existed.
    pub fn delete(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        key: &str,
    ) -> anyhow::Result<bool> {
        let write_txn = self.db.begin_write()?;
        let existed = {
            let mut t = write_txn.open_table(table)?;
            let removed = t.remove(key)?;
            let found = removed.is_some();
            drop(removed);
            found
        };
        write_txn.commit()?;
        Ok(existed)
    }

    /// Return all key/value pairs in `table` in sorted order.
    pub fn list(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(table)?;
        let mut out = Vec::new();
        for result in t.iter()? {
            let (k, v) = result?;
            out.push((k.value().to_owned(), v.value().to_vec()));
        }
        Ok(out)
    }

    /// Return key/value pairs in `table` whose keys fall in `[start, end)`.
    pub fn range(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        start: &str,
        end: &str,
    ) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(table)?;
        let mut out = Vec::new();
        for result in t.range(start..end)? {
            let (k, v) = result?;
            out.push((k.value().to_owned(), v.value().to_vec()));
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // String-value table methods (e.g. VAULT_META)
    // -------------------------------------------------------------------------

    /// Insert or update `key` → `value` (both `&str`) in `table`.
    pub fn put_str(
        &self,
        table: TableDefinition<'static, &str, &str>,
        key: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut t = write_txn.open_table(table)?;
            t.insert(key, value)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Retrieve the string value for `key` from `table`.
    pub fn get_str(
        &self,
        table: TableDefinition<'static, &str, &str>,
        key: &str,
    ) -> anyhow::Result<Option<String>> {
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(table)?;
        match t.get(key)? {
            Some(guard) => Ok(Some(guard.value().to_owned())),
            None => Ok(None),
        }
    }

    // -------------------------------------------------------------------------
    // KV convenience methods (namespace\0key composite key in KV_STORE)
    // -------------------------------------------------------------------------

    /// Store a value under a composite `namespace\0key` in `KV_STORE`.
    pub fn kv_put(&self, namespace: &str, key: &str, value: &[u8]) -> anyhow::Result<()> {
        let composite = format!("{namespace}\0{key}");
        self.put(tables::KV_STORE, &composite, value)
    }

    /// Retrieve a value by `namespace` + `key` from `KV_STORE`.
    pub fn kv_get(&self, namespace: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let composite = format!("{namespace}\0{key}");
        self.get(tables::KV_STORE, &composite)
    }

    /// Delete a value by `namespace` + `key` from `KV_STORE`.
    pub fn kv_delete(&self, namespace: &str, key: &str) -> anyhow::Result<bool> {
        let composite = format!("{namespace}\0{key}");
        self.delete(tables::KV_STORE, &composite)
    }

    /// List all keys in `KV_STORE` under `namespace`.
    pub fn kv_list(&self, namespace: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let prefix = format!("{namespace}\0");
        let prefix_end = format!("{namespace}\x01"); // one byte past '\0' scan
        let mut pairs = self.range(tables::KV_STORE, &prefix, &prefix_end)?;
        // Strip the namespace prefix from returned keys.
        for (k, _) in &mut pairs {
            if let Some(bare) = k.strip_prefix(&prefix) {
                *k = bare.to_owned();
            }
        }
        Ok(pairs)
    }

    // -------------------------------------------------------------------------
    // Agent convenience methods (AGENTS table)
    // -------------------------------------------------------------------------

    /// Store an agent instance, serialized as JSON.
    pub fn store_agent(
        &self,
        id: &str,
        agent: &rockbot_config::AgentInstance,
    ) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(agent)?;
        self.put(tables::AGENTS, id, &bytes)
    }

    /// Load an agent instance by ID.
    pub fn load_agent(&self, id: &str) -> anyhow::Result<Option<rockbot_config::AgentInstance>> {
        match self.get(tables::AGENTS, id)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// List all stored agent instances.
    pub fn list_agents(&self) -> anyhow::Result<Vec<rockbot_config::AgentInstance>> {
        let pairs = self.list(tables::AGENTS)?;
        pairs
            .into_iter()
            .map(|(_, bytes)| serde_json::from_slice(&bytes).map_err(Into::into))
            .collect()
    }

    /// Delete an agent by ID. Returns true if it existed.
    pub fn delete_agent(&self, id: &str) -> anyhow::Result<bool> {
        self.delete(tables::AGENTS, id)
    }

    /// Store any serde value as JSON in the provided byte table.
    pub fn put_json<T: serde::Serialize>(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        key: &str,
        value: &T,
    ) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(value)?;
        self.put(table, key, &bytes)
    }

    /// Load a JSON-serialized serde value by key.
    pub fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
        key: &str,
    ) -> anyhow::Result<Option<T>> {
        match self.get(table, key)? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// List all JSON-serialized rows in a table.
    pub fn list_json<T: serde::de::DeserializeOwned>(
        &self,
        table: TableDefinition<'static, &str, &[u8]>,
    ) -> anyhow::Result<Vec<(String, T)>> {
        self.list(table)?
            .into_iter()
            .map(|(key, bytes)| Ok((key, serde_json::from_slice(&bytes)?)))
            .collect()
    }

    pub(crate) fn replace_bytes_tables(
        &self,
        replacements: &[(BytesTableDefinition, Vec<(String, Vec<u8>)>)],
    ) -> anyhow::Result<()> {
        let write_txn = self.db.begin_write()?;
        for (table, entries) in replacements {
            let mut t = write_txn.open_table(*table)?;
            let keys: Vec<String> = t
                .iter()?
                .map(|result| {
                    let (key, _) = result?;
                    Ok::<_, redb::Error>(key.value().to_owned())
                })
                .collect::<Result<_, _>>()?;
            for key in keys {
                let _ = t.remove(key.as_str())?;
            }
            for (key, value) in entries {
                t.insert(key.as_str(), value.as_slice())?;
            }
        }
        write_txn.commit()?;
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tempfile::tempdir;

    fn open_store() -> (Store, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let store = Store::open(&path).unwrap();
        (store, dir)
    }

    #[test]
    fn put_get_delete() {
        let (store, _dir) = open_store();
        store.put(tables::CREDENTIALS, "foo", b"bar").unwrap();
        let v = store.get(tables::CREDENTIALS, "foo").unwrap();
        assert_eq!(v.as_deref(), Some(b"bar".as_ref()));

        let existed = store.delete(tables::CREDENTIALS, "foo").unwrap();
        assert!(existed);

        let v2 = store.get(tables::CREDENTIALS, "foo").unwrap();
        assert!(v2.is_none());
    }

    #[test]
    fn list_and_range() {
        let (store, _dir) = open_store();
        store.put(tables::ENDPOINTS, "a", b"1").unwrap();
        store.put(tables::ENDPOINTS, "b", b"2").unwrap();
        store.put(tables::ENDPOINTS, "c", b"3").unwrap();

        let all = store.list(tables::ENDPOINTS).unwrap();
        assert_eq!(all.len(), 3);

        let sub = store.range(tables::ENDPOINTS, "a", "c").unwrap();
        // "a" and "b" (exclusive end)
        assert_eq!(sub.len(), 2);
    }

    #[test]
    fn put_str_get_str() {
        let (store, _dir) = open_store();
        store.put_str(tables::VAULT_META, "version", "1").unwrap();
        let v = store.get_str(tables::VAULT_META, "version").unwrap();
        assert_eq!(v.as_deref(), Some("1"));
    }

    #[test]
    fn kv_namespace_isolation() {
        let (store, _dir) = open_store();
        store.kv_put("ns1", "key", b"val1").unwrap();
        store.kv_put("ns2", "key", b"val2").unwrap();

        let v1 = store.kv_get("ns1", "key").unwrap();
        let v2 = store.kv_get("ns2", "key").unwrap();
        assert_eq!(v1.as_deref(), Some(b"val1".as_ref()));
        assert_eq!(v2.as_deref(), Some(b"val2".as_ref()));

        let list = store.kv_list("ns1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].0, "key");
    }

    #[test]
    fn agent_store_roundtrip() {
        let (store, _dir) = open_store();
        let agent = rockbot_config::AgentInstance {
            id: "test-agent".to_string(),
            primary: false,
            workspace: None,
            model: Some("test-model".to_string()),
            max_tool_calls: None,
            temperature: Some(0.3),
            max_tokens: Some(16000),
            parent_id: None,
            creator_agent_id: None,
            owner_agent_id: None,
            zone_id: None,
            system_prompt: None,
            enabled: true,
            mcp_servers: std::collections::HashMap::new(),
            config: std::collections::HashMap::new(),
            max_context_tokens: 128000,
            guardrails: vec![],
            reflection_enabled: false,
            breakpoint_tools: vec![],
            planning_mode: "never".to_string(),
            expose_as_tool: None,
            episodic_memory: false,
            workflow: None,
            llm_timeout_secs: 45,
            tool_timeout_secs: 120,
        };

        store.store_agent("test-agent", &agent).unwrap();
        let loaded = store.load_agent("test-agent").unwrap().unwrap();
        assert_eq!(loaded.id, "test-agent");
        assert_eq!(loaded.model.as_deref(), Some("test-model"));

        let all = store.list_agents().unwrap();
        assert_eq!(all.len(), 1);

        let existed = store.delete_agent("test-agent").unwrap();
        assert!(existed);
        assert!(store.load_agent("test-agent").unwrap().is_none());
    }

    #[test]
    fn encrypted_open_and_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("enc.redb");
        let key = [0xABu8; 32];
        {
            let store = Store::open_encrypted(&path, key).unwrap();
            store
                .put(tables::CREDENTIALS, "secret", b"password")
                .unwrap();
        }
        // Re-open with same key.
        let store2 = Store::open_encrypted(&path, key).unwrap();
        let v = store2.get(tables::CREDENTIALS, "secret").unwrap();
        assert_eq!(v.as_deref(), Some(b"password".as_ref()));
    }
}
