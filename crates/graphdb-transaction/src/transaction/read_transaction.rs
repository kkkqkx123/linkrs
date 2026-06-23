//! Read Transaction
//!
//! Provides read-only snapshot transaction for MVCC-based graph database.
//! A read transaction sees a consistent snapshot of the database at the time
//! the transaction was started.

use super::wal::{LabelId, Timestamp, VertexId};
use super::mvcc::{VersionManager, VersionManagerError};

/// Released timestamp sentinel value (0 means timestamp has been released)
/// Note: distinct from core's RELEASED_TIMESTAMP (u32::MAX)
pub const RELEASED_TIMESTAMP: Timestamp = 0;

/// Read transaction error
#[derive(Debug, Clone, thiserror::Error)]
pub enum ReadTransactionError {
    #[error("Version manager error: {0}")]
    VersionManagerError(#[from] VersionManagerError),

    #[error("Transaction already released")]
    AlreadyReleased,

    #[error("Label not found: {0}")]
    LabelNotFound(LabelId),

    #[error("Vertex not found: {0}")]
    VertexNotFound(VertexId),
}

/// Read transaction result type
pub type ReadTransactionResult<T> = Result<T, ReadTransactionError>;

/// Read Transaction
///
/// A read-only transaction that provides a consistent snapshot view
/// of the database at a specific timestamp. The transaction acquires
/// a read timestamp from the version manager and releases it when done.
///
/// # Example
///
/// ```rust,ignore
/// let txn = ReadTransaction::new(&graph, &version_manager)?;
/// let vertex = txn.get_vertex(label, vid)?;
/// txn.commit(); // or just drop
/// ```
pub struct ReadTransaction<'a, T: ReadTarget + ?Sized> {
    graph: &'a T,
    version_manager: &'a VersionManager,
    timestamp: Timestamp,
}

/// Target for read operations (will be PropertyGraph in phase 2)
pub trait ReadTarget: Send + Sync {
    fn get_vertex(&self, label: LabelId, vid: VertexId, ts: Timestamp) -> Option<VertexRecord>;
    fn get_vertex_count(&self, label: LabelId, ts: Timestamp) -> usize;
    fn vertex_label_num(&self) -> usize;
    fn edge_label_num(&self) -> usize;
}

/// Vertex record for read operations
#[derive(Debug, Clone)]
pub struct VertexRecord {
    pub vid: VertexId,
    pub label: LabelId,
    pub properties: Vec<(String, Vec<u8>)>,
}

impl<'a, T: ReadTarget + ?Sized> ReadTransaction<'a, T> {
    /// Create a new read transaction
    ///
    /// Acquires a read timestamp from the version manager to establish
    /// a consistent snapshot view.
    pub fn new(graph: &'a T, version_manager: &'a VersionManager) -> ReadTransactionResult<Self> {
        let timestamp = version_manager.acquire_read_timestamp();
        Ok(Self {
            graph,
            version_manager,
            timestamp,
        })
    }

    /// Get the transaction's timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Get a vertex by label and ID
    pub fn get_vertex(&self, label: LabelId, vid: VertexId) -> Option<VertexRecord> {
        self.graph.get_vertex(label, vid, self.timestamp)
    }

    /// Get vertex count for a label
    pub fn get_vertex_count(&self, label: LabelId) -> usize {
        self.graph.get_vertex_count(label, self.timestamp)
    }

    /// Get the number of vertex labels
    pub fn vertex_label_num(&self) -> usize {
        self.graph.vertex_label_num()
    }

    /// Get the number of edge labels
    pub fn edge_label_num(&self) -> usize {
        self.graph.edge_label_num()
    }

    /// Commit the read transaction
    ///
    /// For read transactions, commit simply releases the timestamp.
    /// Returns true if successful.
    pub fn commit(mut self) -> ReadTransactionResult<bool> {
        self.release();
        Ok(true)
    }

    /// Abort the read transaction
    ///
    /// For read transactions, abort is the same as commit - just release the timestamp.
    pub fn abort(mut self) -> ReadTransactionResult<()> {
        self.release();
        Ok(())
    }

    /// Release the read timestamp
    fn release(&mut self) {
        if self.timestamp != RELEASED_TIMESTAMP {
            self.version_manager.release_read_timestamp();
            self.timestamp = RELEASED_TIMESTAMP;
        }
    }
}

impl<'a, T: ReadTarget + ?Sized> Drop for ReadTransaction<'a, T> {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockReadTarget;

    impl ReadTarget for MockReadTarget {
        fn get_vertex(
            &self,
            _label: LabelId,
            _vid: VertexId,
            _ts: Timestamp,
        ) -> Option<VertexRecord> {
            None
        }

        fn get_vertex_count(&self, _label: LabelId, _ts: Timestamp) -> usize {
            0
        }

        fn vertex_label_num(&self) -> usize {
            0
        }

        fn edge_label_num(&self) -> usize {
            0
        }
    }

    #[test]
    fn test_read_transaction_basic() {
        let vm = VersionManager::new();
        let target = MockReadTarget;

        let txn = ReadTransaction::new(&target, &vm).expect("Failed to create read transaction");
        assert!(txn.timestamp() >= 1);

        let _ts = txn.timestamp();
        drop(txn);

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_read_transaction_commit() {
        let vm = VersionManager::new();
        let target = MockReadTarget;

        let txn = ReadTransaction::new(&target, &vm).expect("Failed to create read transaction");
        assert!(txn.commit().expect("Commit failed"));

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_read_transaction_abort() {
        let vm = VersionManager::new();
        let target = MockReadTarget;

        let txn = ReadTransaction::new(&target, &vm).expect("Failed to create read transaction");
        txn.abort().expect("Abort failed");

        assert_eq!(vm.pending_count(), 0);
    }
}
