//! Raft state machine: applies committed log entries to the redb store.

use std::io::{self, Cursor};
use std::sync::Arc;

use futures_util::StreamExt;
use openraft::alias::{LogIdOf, SnapshotMetaOf, SnapshotOf, StoredMembershipOf};
use openraft::storage::{EntryResponder, RaftSnapshotBuilder, RaftStateMachine};
use openraft::{EntryPayload, OptionalSend, StoredMembership};

use super::{StoreRequest, StoreResponse, TypeConfig};
use crate::tables;
use crate::Store;

pub struct StateMachine {
    store: Arc<Store>,
    last_applied: Option<LogIdOf<TypeConfig>>,
    last_membership: StoredMembershipOf<TypeConfig>,
}

impl StateMachine {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            last_applied: None,
            last_membership: StoredMembership::default(),
        }
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

            if let Some(r) = responder {
                r.send(response);
            }
        }
        Ok(())
    }

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        StateMachine::new(self.store.clone())
    }

    async fn begin_receiving_snapshot(
        &mut self,
    ) -> Result<<TypeConfig as openraft::RaftTypeConfig>::SnapshotData, io::Error> {
        Ok(Cursor::new(Vec::new()))
    }

    async fn install_snapshot(
        &mut self,
        _meta: &SnapshotMetaOf<TypeConfig>,
        _snapshot: <TypeConfig as openraft::RaftTypeConfig>::SnapshotData,
    ) -> Result<(), io::Error> {
        // Snapshot install is a future task.
        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<SnapshotOf<TypeConfig>>, io::Error> {
        Ok(None)
    }
}

impl RaftSnapshotBuilder<TypeConfig> for StateMachine {
    async fn build_snapshot(&mut self) -> Result<SnapshotOf<TypeConfig>, io::Error> {
        Err(io::Error::other("snapshots not yet implemented"))
    }
}

fn bytes_table_def(
    name: &str,
) -> Result<redb::TableDefinition<'static, &'static str, &'static [u8]>, io::Error> {
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
        "node_keys" => Ok(tables::NODE_KEYS),
        "vault_objects" => Ok(tables::VAULT_OBJECTS),
        "vault_provider_grants" => Ok(tables::VAULT_PROVIDER_GRANTS),
        "vault_node_grants" => Ok(tables::VAULT_NODE_GRANTS),
        "vault_policies" => Ok(tables::VAULT_POLICIES),
        other => Err(io::Error::other(format!("unknown table: {other}"))),
    }
}
