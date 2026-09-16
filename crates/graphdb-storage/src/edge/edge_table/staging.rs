//! Staged write batch: isolated buffer for uncommitted edge writes.
//!
//! A batch holds inserts and deletes without touching the committed
//! topology, property rows, or visibility records. Committing assigns edge
//! identifiers and moves every entry into the committed structures in one
//! pass; a failure rolls back only the entries this batch already applied.
//! Dropping a batch discards it without any compensation work.

use graphdb_core::types::Timestamp;
use graphdb_core::Value;

/// One uncommitted edge insert held in a staging batch.
#[derive(Debug, Clone)]
pub struct StagedInsert {
    pub src: u32,
    pub dst: u32,
    pub rank: i64,
    pub properties: Vec<(String, Value)>,
    pub create_ts: Timestamp,
}

/// One uncommitted edge delete held in a staging batch.
#[derive(Debug, Clone, Copy)]
pub struct StagedDelete {
    pub src: u32,
    pub dst: u32,
    pub rank: i64,
    pub delete_ts: Timestamp,
}

/// Isolated buffer for one atomic group of edge writes.
///
/// Entries stay invisible to reads until committed. Committing applies the
/// whole group or reports the failure with no partial residue from this
/// batch left behind.
#[derive(Debug, Clone, Default)]
pub struct EdgeStagingBatch {
    inserts: Vec<StagedInsert>,
    deletes: Vec<StagedDelete>,
}

impl EdgeStagingBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Buffer one insert. No shared state is touched.
    pub fn stage_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        properties: &[(String, Value)],
        create_ts: Timestamp,
    ) {
        self.inserts.push(StagedInsert {
            src,
            dst,
            rank,
            properties: properties.to_vec(),
            create_ts,
        });
    }

    /// Buffer one delete. No shared state is touched.
    pub fn stage_delete(&mut self, src: u32, dst: u32, rank: i64, delete_ts: Timestamp) {
        self.deletes.push(StagedDelete {
            src,
            dst,
            rank,
            delete_ts,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.deletes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inserts.len() + self.deletes.len()
    }

    pub fn insert_count(&self) -> usize {
        self.inserts.len()
    }

    pub fn staged_inserts(&self) -> &[StagedInsert] {
        &self.inserts
    }

    pub fn staged_deletes(&self) -> &[StagedDelete] {
        &self.deletes
    }

    pub fn delete_count(&self) -> usize {
        self.deletes.len()
    }

    /// Largest timestamp in the batch, used for write-path bookkeeping.
    pub fn max_timestamp(&self) -> Option<Timestamp> {
        self.inserts
            .iter()
            .map(|ins| ins.create_ts)
            .chain(self.deletes.iter().map(|del| del.delete_ts))
            .max()
    }

    /// Read-your-write check before commit: does the batch hold an insert
    /// for `(src, dst, rank)`?
    pub fn contains_insert(&self, src: u32, dst: u32, rank: i64) -> bool {
        self.inserts
            .iter()
            .any(|ins| ins.src == src && ins.dst == dst && ins.rank == rank)
    }

    /// Check whether the batch holds a delete for `(src, dst, rank)`.
    pub fn contains_delete(&self, src: u32, dst: u32, rank: i64) -> bool {
        self.deletes
            .iter()
            .any(|del| del.src == src && del.dst == dst && del.rank == rank)
    }

    pub(crate) fn take_inserts(&mut self) -> Vec<StagedInsert> {
        std::mem::take(&mut self.inserts)
    }

    pub(crate) fn take_deletes(&mut self) -> Vec<StagedDelete> {
        std::mem::take(&mut self.deletes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_batch_buffers_without_side_effects() {
        let mut batch = EdgeStagingBatch::new();
        assert!(batch.is_empty());
        batch.stage_insert(0, 1, 0, &[], 100);
        batch.stage_delete(0, 2, 0, 120);
        assert_eq!(batch.len(), 2);
        assert_eq!(batch.insert_count(), 1);
        assert_eq!(batch.delete_count(), 1);
        assert_eq!(batch.max_timestamp(), Some(120));
        assert!(batch.contains_insert(0, 1, 0));
        assert!(!batch.contains_insert(0, 2, 0));
        assert!(batch.contains_delete(0, 2, 0));
    }

    #[test]
    fn staging_batch_discard_is_implicit() {
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(0, 1, 0, &[], 100);
        drop(batch);
    }
}
