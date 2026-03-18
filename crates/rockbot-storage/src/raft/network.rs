//! Raft network layer stub.
//!
//! The actual transport (Noise-encrypted channels) is a future task.
//! This module provides the required trait implementations so the
//! `replication` feature compiles cleanly.

use std::future::Future;

use openraft::alias::{SnapshotOf, VoteOf};
use openraft::anyerror::AnyError;
use openraft::error::{RPCError, ReplicationClosed, StreamingError, Unreachable};
use openraft::network::v2::RaftNetworkV2;
use openraft::network::{RPCOption, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::OptionalSend;

use super::TypeConfig;

type NodeId = <TypeConfig as openraft::RaftTypeConfig>::NodeId;
type Node = <TypeConfig as openraft::RaftTypeConfig>::Node;

/// Placeholder network factory — returns stub connections.
pub struct NetworkFactory;

impl RaftNetworkFactory<TypeConfig> for NetworkFactory {
    type Network = NetworkConnection;

    async fn new_client(&mut self, target: NodeId, _node: &Node) -> Self::Network {
        NetworkConnection { target }
    }
}

/// A stub Raft network connection to a remote node.
pub struct NetworkConnection {
    target: NodeId,
}

impl RaftNetworkV2<TypeConfig> for NetworkConnection {
    async fn append_entries(
        &mut self,
        _rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<TypeConfig>, RPCError<TypeConfig>> {
        Err(RPCError::Unreachable(Unreachable::new(&AnyError::error(
            format!(
                "network transport not yet implemented (node {})",
                self.target
            ),
        ))))
    }

    async fn vote(
        &mut self,
        _rpc: VoteRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<VoteResponse<TypeConfig>, RPCError<TypeConfig>> {
        Err(RPCError::Unreachable(Unreachable::new(&AnyError::error(
            format!(
                "network transport not yet implemented (node {})",
                self.target
            ),
        ))))
    }

    async fn full_snapshot(
        &mut self,
        _vote: VoteOf<TypeConfig>,
        _snapshot: SnapshotOf<TypeConfig>,
        _cancel: impl Future<Output = ReplicationClosed> + OptionalSend + 'static,
        _option: RPCOption,
    ) -> Result<SnapshotResponse<TypeConfig>, StreamingError<TypeConfig>> {
        Err(StreamingError::Unreachable(Unreachable::new(
            &AnyError::error(format!(
                "network transport not yet implemented (node {})",
                self.target
            )),
        )))
    }
}
