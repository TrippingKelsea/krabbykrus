//! Optional OpenRaft integration for multi-node replication.
//!
//! Enabled by the `replication` feature flag.

pub mod log_store;
pub mod network;
pub mod state_machine;

use std::fmt;
use std::io::Cursor;

use serde::{Deserialize, Serialize};

/// A mutation applied to the store via Raft consensus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StoreRequest {
    /// Insert or update a key/value pair in a named table.
    Put {
        table: String,
        key: String,
        value: Vec<u8>,
    },
    /// Delete a key from a named table.
    Delete { table: String, key: String },
}

impl fmt::Display for StoreRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreRequest::Put { table, key, .. } => {
                write!(f, "Put({table}/{key})")
            }
            StoreRequest::Delete { table, key } => {
                write!(f, "Delete({table}/{key})")
            }
        }
    }
}

/// The result returned after a Raft log entry is applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StoreResponse {
    /// The mutation was applied successfully.
    Ok,
    /// The key was not found (for delete operations).
    NotFound,
}

openraft::declare_raft_types!(
    /// Type configuration for the RockBot store Raft cluster.
    pub TypeConfig:
        D = StoreRequest,
        R = StoreResponse,
        NodeId = u64,
        Node = (),
        SnapshotData = Cursor<Vec<u8>>,
        AsyncRuntime = openraft::TokioRuntime,
);
