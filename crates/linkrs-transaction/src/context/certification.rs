//! Transaction context behavior: read and write set recording for conflict certification

use super::TransactionContext;
use crate::error::TransactionError;
use crate::types::*;
use linkrs_core::types::VertexId;
impl TransactionContext {
    /// Get the write set for this transaction
    pub fn get_write_set(&self) -> WriteSet {
        self.write_set.lock().clone()
    }

    /// Check if write set is empty
    pub fn is_write_set_empty(&self) -> bool {
        self.write_set.lock().is_empty()
    }

    /// Get write set size (number of modified entities)
    pub fn write_set_size(&self) -> usize {
        self.write_set.lock().size()
    }

    /// Mark this transaction as having passed write set validation
    pub fn mark_write_validated(&self) {
        self.write_validated.store(true);
    }

    /// Check if this transaction has passed write set validation
    pub fn is_write_validated(&self) -> bool {
        self.write_validated.load()
    }

    /// Check if this transaction's write set conflicts with another
    pub fn has_write_conflict_with(&self, other: &TransactionContext) -> bool {
        let ws1 = self.write_set.lock();
        let ws2 = other.write_set.lock();
        ws1.has_conflict_with(&ws2)
    }

    /// Get the read set captured by this transaction.
    pub fn get_read_set(&self) -> WriteSet {
        self.read_set.lock().clone()
    }

    pub fn record_vertex_read(&self, vid: VertexId) {
        self.read_set.lock().record_vertex(vid);
    }

    pub fn record_edge_read(&self, edge: linkrs_core::types::EdgeIdentifier) {
        self.read_set.lock().record_edge(edge);
    }

    pub fn record_schema_read(&self, resource: &str) {
        self.read_set.lock().record_schema_resource(resource);
    }

    pub fn record_index_read(&self, resource: &str) {
        self.read_set.lock().record_index_resource(resource);
    }

    pub fn record_read_range(&self, range: ReadRange) {
        self.read_set.lock().record_read_range(range);
    }

    /// Record a vertex write in the write set
    pub fn record_vertex_write(&self, vid: VertexId) {
        self.write_set.lock().record_vertex(vid);
    }

    pub fn record_vertex_delete(&self, vid: VertexId) {
        self.write_set.lock().record_vertex_delete(vid);
    }

    /// Record an edge write for conflict certification.
    pub fn record_edge_write(&self, edge: linkrs_core::types::EdgeIdentifier) {
        self.write_set.lock().record_edge(edge);
    }

    pub fn record_schema_write(&self, resource: &str) -> Result<(), TransactionError> {
        self.write_set.lock().record_schema_resource(resource);
        Ok(())
    }

    pub fn record_index_write(&self, resource: &str) {
        self.write_set.lock().record_index_resource(resource);
    }

    /// Conflict-certification probe: whether this transaction locally wrote
    /// `vid`. Own writes are visible through timestamps (the effective read
    /// stamp never trails the write stamp); this probe feeds commit
    /// certification and debugging assertions, not read merging
    /// (see `WriteSet::covers_vertex`).
    pub fn has_local_vertex_write(&self, vid: &VertexId) -> bool {
        self.write_set.lock().covers_vertex(vid)
    }

    /// Edge counterpart of [`Self::has_local_vertex_write`].
    pub fn has_local_edge_write(&self, edge: &linkrs_core::types::EdgeIdentifier) -> bool {
        self.write_set.lock().covers_edge(edge)
    }

    /// Replace the write set after a savepoint rollback.
    pub fn restore_write_set(&self, write_set: WriteSet) {
        *self.write_set.lock() = write_set;
        self.write_validated.store(false);
    }

    pub fn restore_read_set(&self, read_set: WriteSet) {
        *self.read_set.lock() = read_set;
    }

    /// Clear certification state after a partial rollback.
    pub fn clear_write_validation(&self) {
        self.write_validated.store(false);
    }
}
