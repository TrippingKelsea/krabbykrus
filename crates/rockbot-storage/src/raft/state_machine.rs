//! Raft state machine: applies committed log entries to the redb store.

use std::io::{self, Cursor, Read};
use std::sync::Arc;

use futures_util::StreamExt;
use openraft::alias::{LogIdOf, SnapshotMetaOf, SnapshotOf, StoredMembershipOf};
use openraft::storage::{EntryResponder, RaftSnapshotBuilder, RaftStateMachine};
use openraft::{EntryPayload, OptionalSend, StoredMembership};
use redb::TableHandle;

use super::{StoreRequest, StoreResponse, TypeConfig};
use crate::tables;
use crate::{BytesTableDefinition, Store};

const LAST_APPLIED_KEY: &str = "raft/internal/state_machine/last_applied";
const LAST_MEMBERSHIP_KEY: &str = "raft/internal/state_machine/last_membership";
const SNAPSHOT_META_KEY: &str = "raft/internal/snapshot/meta";
const SNAPSHOT_DATA_KEY: &str = "raft/internal/snapshot/data";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SnapshotPayload {
    last_applied: Option<LogIdOf<TypeConfig>>,
    last_membership: StoredMembershipOf<TypeConfig>,
    tables: Vec<TableSnapshot>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TableSnapshot {
    name: String,
    rows: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Clone)]
struct PersistedSnapshot {
    meta: SnapshotMetaOf<TypeConfig>,
    data: Vec<u8>,
}

pub struct StateMachine {
    store: Arc<Store>,
    last_applied: Option<LogIdOf<TypeConfig>>,
    last_membership: StoredMembershipOf<TypeConfig>,
    current_snapshot: Option<PersistedSnapshot>,
}

impl StateMachine {
    pub fn new(store: Arc<Store>) -> io::Result<Self> {
        let last_applied = store
            .get_json(tables::REPLICATION_META, LAST_APPLIED_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let last_membership = store
            .get_json(tables::REPLICATION_META, LAST_MEMBERSHIP_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?
            .unwrap_or_else(StoredMembership::default);
        let current_snapshot = Self::load_snapshot(&store)?;

        Ok(Self {
            store,
            last_applied,
            last_membership,
            current_snapshot,
        })
    }

    fn apply_request(&self, req: &StoreRequest) -> Result<StoreResponse, io::Error> {
        match req {
            StoreRequest::Put { table, key, value } => {
                let td = bytes_table_def(table)?;
                self.store
                    .put(td, key, value)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                Ok(StoreResponse::Ok)
            }
            StoreRequest::Delete { table, key } => {
                let td = bytes_table_def(table)?;
                let existed = self
                    .store
                    .delete(td, key)
                    .map_err(|e| io::Error::other(e.to_string()))?;
                if existed {
                    Ok(StoreResponse::Ok)
                } else {
                    Ok(StoreResponse::NotFound)
                }
            }
        }
    }

    fn persist_applied_state(&self) -> io::Result<()> {
        if let Some(last_applied) = self.last_applied {
            self.store
                .put_json(tables::REPLICATION_META, LAST_APPLIED_KEY, &last_applied)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        self.store
            .put_json(
                tables::REPLICATION_META,
                LAST_MEMBERSHIP_KEY,
                &self.last_membership,
            )
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn load_snapshot(store: &Store) -> io::Result<Option<PersistedSnapshot>> {
        let meta = store
            .get_json(tables::REPLICATION_META, SNAPSHOT_META_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let data = store
            .get(tables::REPLICATION_META, SNAPSHOT_DATA_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;
        match (meta, data) {
            (Some(meta), Some(data)) => Ok(Some(PersistedSnapshot { meta, data })),
            _ => Ok(None),
        }
    }

    fn persist_snapshot(
        &mut self,
        meta: SnapshotMetaOf<TypeConfig>,
        data: Vec<u8>,
    ) -> io::Result<()> {
        self.store
            .put_json(tables::REPLICATION_META, SNAPSHOT_META_KEY, &meta)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.store
            .put(tables::REPLICATION_META, SNAPSHOT_DATA_KEY, &data)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.current_snapshot = Some(PersistedSnapshot { meta, data });
        Ok(())
    }

    fn snapshot_tables() -> Vec<(&'static str, BytesTableDefinition)> {
        vec![
            (tables::ENDPOINTS.name(), tables::ENDPOINTS),
            (tables::CREDENTIALS.name(), tables::CREDENTIALS),
            (tables::PERMISSIONS.name(), tables::PERMISSIONS),
            (tables::KV_STORE.name(), tables::KV_STORE),
            (tables::SESSIONS.name(), tables::SESSIONS),
            (tables::SESSION_MESSAGES.name(), tables::SESSION_MESSAGES),
            (tables::CRON_JOBS.name(), tables::CRON_JOBS),
            (tables::ROUTE_BINDINGS.name(), tables::ROUTE_BINDINGS),
            (tables::PKI_INDEX.name(), tables::PKI_INDEX),
            (tables::AGENTS.name(), tables::AGENTS),
            (tables::AGENT_DOCUMENTS.name(), tables::AGENT_DOCUMENTS),
            (tables::AGENT_OBJECTS.name(), tables::AGENT_OBJECTS),
            (tables::TOPOLOGY_NODES.name(), tables::TOPOLOGY_NODES),
            (tables::TOPOLOGY_EDGES.name(), tables::TOPOLOGY_EDGES),
            (
                tables::TOPOLOGY_EDGES_FROM.name(),
                tables::TOPOLOGY_EDGES_FROM,
            ),
            (tables::TOPOLOGY_EDGES_TO.name(), tables::TOPOLOGY_EDGES_TO),
            (tables::ZONES.name(), tables::ZONES),
            (tables::ZONE_MEMBERS.name(), tables::ZONE_MEMBERS),
            (tables::BLACKBOARDS.name(), tables::BLACKBOARDS),
            (tables::BLACKBOARD_ACL.name(), tables::BLACKBOARD_ACL),
            (tables::OWNERSHIP_EVENTS.name(), tables::OWNERSHIP_EVENTS),
            (tables::AGENT_VDISKS.name(), tables::AGENT_VDISKS),
            (tables::NODE_KEYS.name(), tables::NODE_KEYS),
            (tables::VAULT_OBJECTS.name(), tables::VAULT_OBJECTS),
            (
                tables::VAULT_PROVIDER_GRANTS.name(),
                tables::VAULT_PROVIDER_GRANTS,
            ),
            (tables::VAULT_NODE_GRANTS.name(), tables::VAULT_NODE_GRANTS),
            (tables::VAULT_POLICIES.name(), tables::VAULT_POLICIES),
        ]
    }

    fn snapshot_id(last_applied: Option<LogIdOf<TypeConfig>>) -> String {
        match last_applied {
            Some(log_id) => format!("{}-{}", log_id.leader_id, log_id.index),
            None => "empty-0".to_string(),
        }
    }
}

impl RaftStateMachine<TypeConfig> for StateMachine {
    type SnapshotBuilder = Self;

    async fn applied_state(
        &mut self,
    ) -> Result<(Option<LogIdOf<TypeConfig>>, StoredMembershipOf<TypeConfig>), io::Error> {
        Ok((self.last_applied, self.last_membership.clone()))
    }

    async fn apply<Strm>(&mut self, mut entries: Strm) -> Result<(), io::Error>
    where
        Strm: futures_util::Stream<Item = Result<EntryResponder<TypeConfig>, io::Error>>
            + Unpin
            + OptionalSend,
    {
        while let Some(item) = entries.next().await {
            let (entry, responder) = item?;
            let log_id = entry.log_id;
            self.last_applied = Some(log_id);

            let response = match &entry.payload {
                EntryPayload::Blank => StoreResponse::Ok,
                EntryPayload::Normal(req) => self.apply_request(req)?,
                EntryPayload::Membership(m) => {
                    self.last_membership = StoredMembership::new(Some(log_id), m.clone());
                    StoreResponse::Ok
                }
            };
            self.persist_applied_state()?;

            if let Some(r) = responder {
                r.send(response);
            }
        }
        Ok(())
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        StateMachine::new(self.store.clone()).expect("raft snapshot builder should initialize")
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<<TypeConfig as openraft::RaftTypeConfig>::SnapshotData, io::Error> {
        Ok(Cursor::new(Vec::new()))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMetaOf<TypeConfig>,
        mut snapshot: <TypeConfig as openraft::RaftTypeConfig>::SnapshotData,
    ) -> Result<(), io::Error> {
        let mut bytes = Vec::new();
        snapshot.read_to_end(&mut bytes)?;
        let payload: SnapshotPayload =
            serde_json::from_slice(&bytes).map_err(|e| io::Error::other(e.to_string()))?;

        let replacements: Vec<_> = Self::snapshot_tables()
            .into_iter()
            .map(|(name, table)| {
                let rows = payload
                    .tables
                    .iter()
                    .find(|table_snapshot| table_snapshot.name == name)
                    .map(|table_snapshot| table_snapshot.rows.clone())
                    .unwrap_or_default();
                (table, rows)
            })
            .collect();
        self.store
            .replace_bytes_tables(&replacements)
            .map_err(|e| io::Error::other(e.to_string()))?;

        self.last_applied = payload.last_applied;
        self.last_membership = payload.last_membership;
        self.persist_applied_state()?;
        self.persist_snapshot(meta.clone(), bytes)?;
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<SnapshotOf<TypeConfig>>, io::Error> {
        Ok(self
            .current_snapshot
            .clone()
            .map(|snapshot| SnapshotOf::<TypeConfig> {
                meta: snapshot.meta,
                snapshot: Cursor::new(snapshot.data),
            }))
    }
}

impl RaftSnapshotBuilder<TypeConfig> for StateMachine {
    async fn build_snapshot(&mut self) -> Result<SnapshotOf<TypeConfig>, io::Error> {
        let payload = SnapshotPayload {
            last_applied: self.last_applied,
            last_membership: self.last_membership.clone(),
            tables: Self::snapshot_tables()
                .into_iter()
                .map(|(name, table)| {
                    let rows = self
                        .store
                        .list(table)
                        .map_err(|e| io::Error::other(e.to_string()))?;
                    Ok::<_, io::Error>(TableSnapshot {
                        name: name.to_string(),
                        rows,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        let data = serde_json::to_vec(&payload).map_err(|e| io::Error::other(e.to_string()))?;
        let meta = SnapshotMetaOf::<TypeConfig> {
            last_log_id: self.last_applied,
            last_membership: self.last_membership.clone(),
            snapshot_id: Self::snapshot_id(self.last_applied),
        };
        self.persist_snapshot(meta.clone(), data.clone())?;
        Ok(SnapshotOf::<TypeConfig> {
            meta,
            snapshot: Cursor::new(data),
        })
    }
}

fn bytes_table_def(name: &str) -> Result<BytesTableDefinition, io::Error> {
    match name {
        "endpoints" => Ok(tables::ENDPOINTS),
        "credentials" => Ok(tables::CREDENTIALS),
        "permissions" => Ok(tables::PERMISSIONS),
        "kv_store" => Ok(tables::KV_STORE),
        "sessions" => Ok(tables::SESSIONS),
        "session_messages" => Ok(tables::SESSION_MESSAGES),
        "cron_jobs" => Ok(tables::CRON_JOBS),
        "route_bindings" => Ok(tables::ROUTE_BINDINGS),
        "pki_index" => Ok(tables::PKI_INDEX),
        "agents" => Ok(tables::AGENTS),
        "agent_documents" => Ok(tables::AGENT_DOCUMENTS),
        "agent_objects" => Ok(tables::AGENT_OBJECTS),
        "topology_nodes" => Ok(tables::TOPOLOGY_NODES),
        "topology_edges" => Ok(tables::TOPOLOGY_EDGES),
        "topology_edges_from" => Ok(tables::TOPOLOGY_EDGES_FROM),
        "topology_edges_to" => Ok(tables::TOPOLOGY_EDGES_TO),
        "zones" => Ok(tables::ZONES),
        "zone_members" => Ok(tables::ZONE_MEMBERS),
        "blackboards" => Ok(tables::BLACKBOARDS),
        "blackboard_acl" => Ok(tables::BLACKBOARD_ACL),
        "ownership_events" => Ok(tables::OWNERSHIP_EVENTS),
        "agent_vdisks" => Ok(tables::AGENT_VDISKS),
        "node_keys" => Ok(tables::NODE_KEYS),
        "vault_objects" => Ok(tables::VAULT_OBJECTS),
        "vault_provider_grants" => Ok(tables::VAULT_PROVIDER_GRANTS),
        "vault_node_grants" => Ok(tables::VAULT_NODE_GRANTS),
        "vault_policies" => Ok(tables::VAULT_POLICIES),
        other => Err(io::Error::other(format!("unknown table: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use openraft::storage::RaftStateMachine;
    use openraft::vote::RaftLeaderIdExt;
    use tempfile::tempdir;

    fn open_store() -> Arc<Store> {
        let dir = tempdir().unwrap();
        let path = dir.keep().join("raft-sm.redb");
        Arc::new(Store::open(&path).unwrap())
    }

    #[tokio::test]
    async fn state_machine_builds_and_reloads_snapshots() {
        let store = open_store();
        store
            .put(tables::AGENTS, "agent-1", br#"{"id":"agent-1"}"#)
            .unwrap();
        store
            .put(tables::BLACKBOARDS, "board-1", br#"{"name":"board-1"}"#)
            .unwrap();

        let mut sm = StateMachine::new(store.clone()).unwrap();
        sm.last_applied = Some(LogIdOf::<TypeConfig>::new(
            <TypeConfig as openraft::RaftTypeConfig>::LeaderId::new_committed(2, 1),
            5,
        ));
        sm.persist_applied_state().unwrap();

        let snapshot = sm.build_snapshot().await.unwrap();
        let snapshot_bytes = snapshot.snapshot.into_inner();

        store.delete(tables::AGENTS, "agent-1").unwrap();
        store.delete(tables::BLACKBOARDS, "board-1").unwrap();

        let mut restored = StateMachine::new(store.clone()).unwrap();
        restored
            .install_snapshot(&snapshot.meta, Cursor::new(snapshot_bytes))
            .await
            .unwrap();

        assert!(store.get(tables::AGENTS, "agent-1").unwrap().is_some());
        assert!(store.get(tables::BLACKBOARDS, "board-1").unwrap().is_some());

        let mut reopened = StateMachine::new(store).unwrap();
        let current = reopened.get_current_snapshot().await.unwrap().unwrap();
        assert_eq!(snapshot.meta.snapshot_id, current.meta.snapshot_id);
        assert_eq!(snapshot.meta.last_log_id, current.meta.last_log_id);
    }
}
