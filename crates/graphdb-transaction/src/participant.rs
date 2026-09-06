use std::sync::Arc;

use graphdb_core::types::{
    CommitLsn, DurabilityLevel, EdgeIdentifier, Timestamp, TransactionId, VertexId,
};

use super::context::TransactionContext;
use super::error::TransactionError;
use super::types::{MutationResult, WriteSet};
use super::undo_log::UndoLogEntry;

/// The immutable information required by a commit participant.
///
/// `write_timestamp` is the transaction's start timestamp (used for conflict
/// certification); `commit_timestamp` is allocated at commit time and orders
/// read visibility. It is 0 until the commit timestamp has been allocated,
/// i.e. while the commit is durable but not yet finalized.
#[derive(Debug, Clone)]
pub struct TransactionCommitDescriptor {
    pub transaction_id: TransactionId,
    pub write_timestamp: Timestamp,
    pub commit_timestamp: Timestamp,
    pub durability: DurabilityLevel,
    pub write_set: WriteSet,
    pub read_set: WriteSet,
    pub first_sequence: u64,
    pub entry_count: usize,
    pub intent_count: usize,
    pub journal_range: std::ops::Range<usize>,
}

impl TransactionCommitDescriptor {
    pub fn new(
        transaction_id: TransactionId,
        write_timestamp: Timestamp,
        durability: DurabilityLevel,
        write_set: WriteSet,
    ) -> Self {
        Self {
            transaction_id,
            write_timestamp,
            commit_timestamp: 0,
            durability,
            write_set,
            read_set: WriteSet::new(),
            first_sequence: 0,
            entry_count: 0,
            intent_count: 0,
            journal_range: 0..0,
        }
    }
}

/// The immutable information required by an abort participant.
///
/// The context is retained so a storage participant can execute the file-backed
/// undo log without copying or draining it across the crate boundary.
#[derive(Debug, Clone)]
pub struct TransactionAbortDescriptor {
    pub transaction_id: TransactionId,
    pub write_timestamp: Timestamp,
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

    fn record_schema_write(&self, _resource: &str) -> Result<(), TransactionError> {
        Ok(())
    }

    fn record_index_write(&self, _resource: &str) {}

    fn record_vertex_read(&self, _vertex_id: VertexId) {}

    fn record_edge_read(&self, _edge: EdgeIdentifier) {}

    fn record_schema_read(&self, _resource: &str) {}

    fn record_index_read(&self, _resource: &str) {}
}

/// Three-phase commit contract for [`TransactionCommitSink`] implementors.
///
/// 1. `commit` (WAL durability point): append the transaction's payload to
///    durable storage and return its LSN. May be retried on transient
///    errors; implementations must tolerate duplicate payloads for the same
///    transaction idempotently or rely on the manager's retry budget.
/// 2. `finalize` (storage visibility point): make the durable payload
///    visible to readers. MUST be idempotent: recovery re-drives it after
///    crashes and partial failures, so applying it twice must be equivalent
///    to applying it once.
/// 3. `recover_unfinalized_commits` (startup replay): re-run pending
///    finalizations for LSNs that appear durable but were never finalized.
///    MUST also be idempotent and return the number of recovered commits.
///
/// The transaction manager guarantees ordering: visibility (commit-timestamp
/// allocation and read-frontier advance) only happens after `finalize`
/// succeeds, so readers never observe unfinalized writes.
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

    /// Called during startup recovery (after WAL replay) to finalize commits
    /// that were persisted but left unfinalized due to a prior crash or
    /// partial failure. The implementation should re-run any post-commit
    /// finalization for LSNs that appear in the WAL whose post-commit state
    /// is incomplete.
    ///
    /// Returns the number of recovered commits.
    ///
    /// Default: no-op (safe for sinks that don't require post-commit
    /// finalization or whose finalization is idempotent on replay).
    fn recover_unfinalized_commits(&self) -> Result<usize, String> {
        Ok(0)
    }

    /// Check whether a checkpoint is needed and trigger one if so.
    ///
    /// Called after a successful write transaction commit when
    /// `auto_checkpoint_after_commit` is enabled in the transaction manager
    /// config. The implementation should evaluate WAL size, time since last
    /// checkpoint, or other heuristics and initiate a non-blocking checkpoint
    /// if thresholds are exceeded.
    ///
    /// Default: no-op (safe for sinks that handle checkpointing externally).
    fn auto_checkpoint_if_needed(&self) -> Result<(), String> {
        Ok(())
    }
}
