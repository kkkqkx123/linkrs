use std::sync::Arc;

use crate::core::types::{CommitLsn, DurabilityLevel, EdgeIdentifier, TransactionId, VertexId};

use super::context::TransactionContext;
use super::error::TransactionError;
use super::types::{MutationResult, WriteSet};
use super::undo_log::UndoLogEntry;

/// The immutable information required by a commit participant.
#[derive(Debug, Clone)]
pub struct TransactionCommitDescriptor {
    pub transaction_id: TransactionId,
    pub write_timestamp: u32,
    pub durability: DurabilityLevel,
    pub write_set: WriteSet,
}

/// The immutable information required by an abort participant.
///
/// The context is retained so a storage participant can execute the file-backed
/// undo log without copying or draining it across the crate boundary.
#[derive(Debug, Clone)]
pub struct TransactionAbortDescriptor {
    pub transaction_id: TransactionId,
    pub write_timestamp: u32,
    pub context: Arc<TransactionContext>,
}

/// Receives mutations while a storage operation is executing in a transaction.
pub trait TransactionMutationRecorder: Send + Sync + std::fmt::Debug {
    /// Record all metadata for one already-applied logical mutation.
    fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError>;

    fn record_vertex_write(&self, vertex_id: VertexId);

    fn record_vertex_delete(&self, _vertex_id: VertexId) {}

    fn record_edge_write(&self, edge: EdgeIdentifier);

    fn add_undo_log(&self, entry: UndoLogEntry) -> Result<(), TransactionError>;

    fn record_table_modification(&self, table_name: &str);

    fn record_schema_write(&self, _resource: &str) {}

    fn record_index_write(&self, _resource: &str) {}

    fn record_vertex_read(&self, _vertex_id: VertexId) {}

    fn record_edge_read(&self, _edge: EdgeIdentifier) {}

    fn record_schema_read(&self, _resource: &str) {}
}

pub trait TransactionCommitSink: Send + Sync {
    fn commit_transaction(&self, transaction_id: TransactionId) -> Result<CommitLsn, String>;

    fn abort_transaction(&self, transaction_id: TransactionId) -> Result<(), String>;

    fn commit_transaction_with_descriptor(
        &self,
        descriptor: &TransactionCommitDescriptor,
    ) -> Result<CommitLsn, String> {
        self.commit_transaction(descriptor.transaction_id)
    }

    fn finalize_commit(
        &self,
        _descriptor: &TransactionCommitDescriptor,
        _commit_lsn: CommitLsn,
    ) -> Result<(), String> {
        Ok(())
    }

    fn abort_transaction_with_descriptor(
        &self,
        descriptor: &TransactionAbortDescriptor,
    ) -> Result<(), String> {
        self.abort_transaction(descriptor.transaction_id)
    }
}
