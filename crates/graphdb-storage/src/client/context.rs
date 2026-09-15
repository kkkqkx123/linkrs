use graphdb_core::types::TransactionId;
use graphdb_core::types::{LabelId, Timestamp};
use graphdb_transaction::TransactionMutationRecorder;

use crate::engine::data_store::EdgeTableKey;
use crate::SnapshotHandle;

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
    /// MVCC snapshot handles for GC coordination - stores (label_id, handle) pairs for vertex tables
    pub mvcc_vertex_snapshot_handles: Vec<(LabelId, SnapshotHandle)>,
    /// Edge table snapshots tracked by timestamp only (no handles needed)
    pub mvcc_edge_snapshot_registered: bool,
    /// Lazily registered vertex labels with their snapshot handles (for unregistration on finalize)
    pub registered_vertex_labels: parking_lot::RwLock<std::collections::HashSet<LabelId>>,
    /// Lazily registered edge partitions (for snapshot unregistration on finalize)
    pub registered_edge_partitions: parking_lot::RwLock<std::collections::HashSet<EdgeTableKey>>,
    /// Undo log entry count at the start of this statement's segment (group
    /// mode only). Used by `finalize_operation` to roll back only the failed
    /// statement's segment when a shared undo log is in use.
    pub auto_commit_group_start: Option<usize>,
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
            mvcc_vertex_snapshot_handles: self.mvcc_vertex_snapshot_handles.clone(),
            mvcc_edge_snapshot_registered: self.mvcc_edge_snapshot_registered,
            registered_vertex_labels: parking_lot::RwLock::new(
                self.registered_vertex_labels.read().clone(),
            ),
            registered_edge_partitions: parking_lot::RwLock::new(
                self.registered_edge_partitions.read().clone(),
            ),
            auto_commit_group_start: self.auto_commit_group_start,
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
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: None,
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
