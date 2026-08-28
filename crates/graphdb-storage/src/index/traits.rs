use graphdb_core::types::{Index, Timestamp, MAX_TIMESTAMP};
use graphdb_core::{StorageError, Value};
use crate::index::types::{EdgeIdentity, GcStats};

pub trait VertexIndexOps: Send + Sync {
    fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn lookup_tag_index(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    fn lookup_tag_index_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError>;

    /// Look up an index value without forcing pending deltas into a new
    /// generation. Pending (unpublished) entries are merged in-memory as
    /// the newest overlay so the result is identical to
    /// [`lookup_tag_index_mvcc`](Self::lookup_tag_index_mvcc) but without the
    /// generation churn. Used by the unique-constraint check on the write path,
    /// where a per-statement publish would defeat delta accumulation.
    fn lookup_tag_index_pending_aware(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_pending_aware_mvcc(space_id, index, value, MAX_TIMESTAMP)
    }

    /// Pending-aware variant of [`lookup_tag_index_mvcc`](Self::lookup_tag_index_mvcc).
    fn lookup_tag_index_pending_aware_mvcc(
        &self,
        space_id: u64,
        index: &Index,
        value: &Value,
        read_ts: Timestamp,
    ) -> Result<Vec<Value>, StorageError> {
        self.lookup_tag_index_mvcc(space_id, index, value, read_ts)
    }

    fn clear_tag_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError>;
}

pub trait EdgeIndexOps: Send + Sync {
    fn update_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        props: &[(String, Value)],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn delete_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_names: &[String],
        write_ts: Timestamp,
    ) -> Result<(), StorageError>;

    fn clear_edge_index(&self, space_id: u64, index_name: &str) -> Result<(), StorageError>;
}

pub trait IndexGcOps: Send + Sync {
    fn gc_tombstones(&self, safe_ts: Timestamp) -> Result<GcStats, StorageError>;
    fn gc_tombstones_incremental(
        &self,
        safe_ts: Timestamp,
        batch_size: usize,
    ) -> Result<GcStats, StorageError>;
    fn tombstone_count(&self) -> usize;
}
