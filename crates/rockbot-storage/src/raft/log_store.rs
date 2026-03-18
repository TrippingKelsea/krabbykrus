//! Raft log storage backed by redb.

use std::fmt::Debug;
use std::io;
use std::ops::RangeBounds;
use std::sync::{Arc, Mutex};

use openraft::alias::{EntryOf, LogIdOf, VoteOf};
use openraft::storage::{IOFlushed, RaftLogReader, RaftLogStorage};
use openraft::{LogState, OptionalSend};

use super::TypeConfig;
use crate::Store;

struct LogStoreInner {
    vote: Option<VoteOf<TypeConfig>>,
    log: Vec<EntryOf<TypeConfig>>,
}

/// In-memory Raft log storage (redb persistence is a future task).
pub struct LogStore {
    _store: Arc<Store>,
    inner: Arc<Mutex<LogStoreInner>>,
}

impl LogStore {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            _store: store,
            inner: Arc::new(Mutex::new(LogStoreInner {
                vote: None,
                log: Vec::new(),
            })),
        }
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
            last_purged_log_id: None,
            last_log_id: last,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        LogStore {
            _store: self._store.clone(),
            inner: self.inner.clone(),
        }
    }

    async fn save_vote(&mut self, vote: &VoteOf<TypeConfig>) -> Result<(), io::Error> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.vote = Some(*vote);
        Ok(())
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
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| io::Error::other(e.to_string()))?;
        inner.log.retain(|e| e.log_id.index > log_id.index);
        Ok(())
    }
}
