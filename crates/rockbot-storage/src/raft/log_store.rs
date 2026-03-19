//! Raft log storage backed by redb.

use std::fmt::Debug;
use std::io;
use std::ops::RangeBounds;
use std::sync::{Arc, Mutex};

use openraft::alias::{EntryOf, LogIdOf, VoteOf};
use openraft::storage::{IOFlushed, RaftLogReader, RaftLogStorage};
use openraft::{LogState, OptionalSend};

use super::TypeConfig;
use crate::tables;
use crate::Store;

const VOTE_KEY: &str = "raft/internal/vote";
const COMMITTED_KEY: &str = "raft/internal/committed";
const LAST_PURGED_KEY: &str = "raft/internal/last_purged";
const LOG_PREFIX: &str = "raft/log/";

struct LogStoreInner {
    vote: Option<VoteOf<TypeConfig>>,
    committed: Option<LogIdOf<TypeConfig>>,
    last_purged: Option<LogIdOf<TypeConfig>>,
    log: Vec<EntryOf<TypeConfig>>,
}

pub struct LogStore {
    store: Arc<Store>,
    inner: Arc<Mutex<LogStoreInner>>,
}

impl LogStore {
    pub fn new(store: Arc<Store>) -> io::Result<Self> {
        let inner = Self::load_inner(&store)?;
        Ok(Self {
            store,
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    fn load_inner(store: &Store) -> io::Result<LogStoreInner> {
        let vote = store
            .get_json(tables::REPLICATION_META, VOTE_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let committed = store
            .get_json(tables::REPLICATION_META, COMMITTED_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let last_purged = store
            .get_json(tables::REPLICATION_META, LAST_PURGED_KEY)
            .map_err(|e| io::Error::other(e.to_string()))?;

        let mut log: Vec<_> = store
            .list_json::<EntryOf<TypeConfig>>(tables::REPLICATION_META)
            .map_err(|e| io::Error::other(e.to_string()))?
            .into_iter()
            .filter(|(key, _)| key.starts_with(LOG_PREFIX))
            .map(|(_, entry)| entry)
            .collect();
        log.sort_by_key(|entry| entry.log_id.index);

        Ok(LogStoreInner {
            vote,
            committed,
            last_purged,
            log,
        })
    }

    fn persist_json<T: serde::Serialize>(&self, key: &str, value: &T) -> io::Result<()> {
        self.store
            .put_json(tables::REPLICATION_META, key, value)
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn delete_meta_key(&self, key: &str) -> io::Result<()> {
        self.store
            .delete(tables::REPLICATION_META, key)
            .map(|_| ())
            .map_err(|e| io::Error::other(e.to_string()))
    }

    fn log_key(index: u64) -> String {
        format!("{LOG_PREFIX}{index:020}")
    }
}

impl RaftLogReader<TypeConfig> for LogStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<EntryOf<TypeConfig>>, io::Error> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let entries: Vec<_> = inner
            .log
            .iter()
            .filter(|e| range.contains(&e.log_id.index))
            .cloned()
            .collect();
        Ok(entries)
    }

    async fn read_vote(&mut self) -> Result<Option<VoteOf<TypeConfig>>, io::Error> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(inner.vote)
    }
}

impl RaftLogStorage<TypeConfig> for LogStore {
    type LogReader = Self;

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, io::Error> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let last = inner.log.last().map(|e| e.log_id);
        Ok(LogState {
            last_purged_log_id: inner.last_purged,
            last_log_id: last.or(inner.last_purged),
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        LogStore {
            store: self.store.clone(),
            inner: self.inner.clone(),
        }
    }

    async fn save_vote(&mut self, vote: &VoteOf<TypeConfig>) -> Result<(), io::Error> {
        self.persist_json(VOTE_KEY, vote)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.vote = Some(*vote);
        Ok(())
    }

    async fn save_committed(
        &mut self,
        committed: Option<LogIdOf<TypeConfig>>,
    ) -> Result<(), io::Error> {
        if let Some(value) = committed {
            self.persist_json(COMMITTED_KEY, &value)?;
        } else {
            self.delete_meta_key(COMMITTED_KEY)?;
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.committed = committed;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogIdOf<TypeConfig>>, io::Error> {
        let inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(inner.committed)
    }

    async fn append<I>(
        &mut self,
        entries: I,
        callback: IOFlushed<TypeConfig>,
    ) -> Result<(), io::Error>
    where
        I: IntoIterator<Item = EntryOf<TypeConfig>> + OptionalSend,
        I::IntoIter: OptionalSend,
    {
        let entries: Vec<_> = entries.into_iter().collect();
        for entry in &entries {
            self.persist_json(&Self::log_key(entry.log_id.index), entry)?;
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        for entry in entries {
            inner.log.push(entry);
        }
        callback.io_completed(Ok(()));
        Ok(())
    }

    async fn truncate_after(
        &mut self,
        last_log_id: Option<LogIdOf<TypeConfig>>,
    ) -> Result<(), io::Error> {
        let keys_to_remove: Vec<String> = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| io::Error::other(e.to_string()))?;
            inner
                .log
                .iter()
                .filter(|entry| match last_log_id {
                    Some(id) => entry.log_id.index > id.index,
                    None => true,
                })
                .map(|entry| Self::log_key(entry.log_id.index))
                .collect()
        };
        for key in keys_to_remove {
            self.delete_meta_key(&key)?;
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        match last_log_id {
            Some(id) => inner.log.retain(|e| e.log_id.index <= id.index),
            None => inner.log.clear(),
        }
        Ok(())
    }

    async fn purge(&mut self, log_id: LogIdOf<TypeConfig>) -> Result<(), io::Error> {
        let keys_to_remove: Vec<String> = {
            let inner = self
                .inner
                .lock()
                .map_err(|e| io::Error::other(e.to_string()))?;
            inner
                .log
                .iter()
                .filter(|entry| entry.log_id.index <= log_id.index)
                .map(|entry| Self::log_key(entry.log_id.index))
                .collect()
        };
        for key in keys_to_remove {
            self.delete_meta_key(&key)?;
        }
        self.persist_json(LAST_PURGED_KEY, &log_id)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.log.retain(|e| e.log_id.index > log_id.index);
        inner.last_purged = Some(log_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use openraft::EntryPayload;
    use openraft::LogId;
    use openraft::Vote;
    use openraft::vote::RaftLeaderIdExt;
    use tempfile::tempdir;
    use crate::raft::StoreRequest;

    fn open_store() -> Arc<Store> {
        let dir = tempdir().unwrap();
        let path = dir.keep().join("raft.redb");
        Arc::new(Store::open(&path).unwrap())
    }

    fn make_entry(index: u64) -> EntryOf<TypeConfig> {
        EntryOf::<TypeConfig> {
            log_id: LogId::new(
                <TypeConfig as openraft::RaftTypeConfig>::LeaderId::new_committed(1, 1),
                index,
            ),
            payload: EntryPayload::Normal(StoreRequest::Put {
                table: "kv_store".to_string(),
                key: format!("k{index}"),
                value: format!("v{index}").into_bytes(),
            }),
        }
    }

    #[tokio::test]
    async fn log_store_persists_vote_and_logs() {
        let store = open_store();
        let mut log_store = LogStore::new(store.clone()).unwrap();

        log_store.save_vote(&Vote::new(3, 2)).await.unwrap();
        log_store
            .save_committed(Some(make_entry(1).log_id))
            .await
            .unwrap();
        log_store
            .append(vec![make_entry(1), make_entry(2)], IOFlushed::noop())
            .await
            .unwrap();

        let mut reopened = LogStore::new(store).unwrap();
        assert_eq!(Some(Vote::new(3, 2)), reopened.read_vote().await.unwrap());
        assert_eq!(
            Some(make_entry(1).log_id),
            reopened.read_committed().await.unwrap()
        );
        assert_eq!(2, reopened.try_get_log_entries(0..=10).await.unwrap().len());
    }
}
