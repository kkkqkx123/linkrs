use graphdb_core::types::Timestamp;
use graphdb_core::types::TransactionId;
use graphdb_transaction::TransactionMutationRecorder;

use std::sync::Arc;

/// Immutable context bound to one storage operation scope.
#[derive(Debug)]
pub struct StorageOperationContext {
    pub transaction_id: Option<TransactionId>,
    pub read_timestamp: Timestamp,
    pub write_timestamp: Option<Timestamp>,
    pub read_only: bool,
    pub auto_commit: bool,
    pub mutation_recorder: Option<Arc<dyn TransactionMutationRecorder>>,
    /// Undo log entry count at the start of this statement's segment (group
    /// mode only). Used by `finalize_operation` to roll back only the failed
    /// statement's segment when a shared undo log is in use.
    pub auto_commit_group_start: Option<usize>,
    /// Staging buffer journal and index lengths at the start of this
    /// statement (group mode only). Used by `finalize_operation` to roll the
    /// shared transaction buffer back to the statement start.
    pub auto_commit_staging_start: Option<(usize, usize)>,
    /// Shared staged-WAL length at the start of this statement (group mode
    /// only). Used to truncate the group's WAL redo back to the statement.
    pub auto_commit_wal_start: usize,
}

impl PartialEq for StorageOperationContext {
    fn eq(&self, other: &Self) -> bool {
        self.transaction_id == other.transaction_id
            && self.read_timestamp == other.read_timestamp
            && self.write_timestamp == other.write_timestamp
            && self.read_only == other.read_only
            && self.auto_commit == other.auto_commit
    }
}

impl Eq for StorageOperationContext {}

impl Clone for StorageOperationContext {
    fn clone(&self) -> Self {
        Self {
            transaction_id: self.transaction_id,
            read_timestamp: self.read_timestamp,
            write_timestamp: self.write_timestamp,
            read_only: self.read_only,
            auto_commit: self.auto_commit,
            mutation_recorder: self.mutation_recorder.clone(),
            auto_commit_group_start: self.auto_commit_group_start,
            auto_commit_staging_start: self.auto_commit_staging_start,
            auto_commit_wal_start: self.auto_commit_wal_start,
        }
    }
}

impl StorageOperationContext {
    pub fn transaction_with_timestamps(
        transaction_id: TransactionId,
        read_timestamp: Timestamp,
        write_timestamp: Option<Timestamp>,
        read_only: bool,
        auto_commit: bool,
    ) -> Self {
        Self {
            transaction_id: Some(transaction_id),
            read_timestamp,
            write_timestamp,
            read_only,
            auto_commit,
            mutation_recorder: None,
            auto_commit_group_start: None,
            auto_commit_staging_start: None,
            auto_commit_wal_start: 0,
        }
    }

    pub fn with_mutation_recorder(
        mut self,
        recorder: Arc<dyn TransactionMutationRecorder>,
    ) -> Self {
        self.mutation_recorder = Some(recorder);
        self
    }

    /// Timestamp at which MVCC snapshots are registered for this operation.
    ///
    /// Read-only operations pin their read snapshot; auto-commit writes pin
    /// the write timestamp (the statement both reads and writes at it).
    pub fn snapshot_timestamp(&self) -> Option<Timestamp> {
        if self.read_only {
            Some(self.read_timestamp)
        } else {
            self.write_timestamp
        }
    }
}
