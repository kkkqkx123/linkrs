//! Staged write batch: isolated buffer for uncommitted edge writes.
//!
//! A batch holds inserts and deletes without touching the committed
//! topology, property rows, or visibility records. Committing assigns edge
//! identifiers and moves every entry into the committed structures in order;
//! a failure rolls back only the entries this batch already applied.
//! Dropping a batch discards it without any compensation work.
//!
//! Commit contract, frozen for all callers:
//! - Entries apply in stage order. Callers own batch ordering.
//! - Staged writes stay invisible to every read until commit.
//! - Failure rolls back only entries this batch already applied.
//! - An insert cancelled by a later delete of the same key in the same batch
//!   leaves no tombstone and no visible edge. The monotonic edge-id counter
//!   only guarantees monotonicity without collision; callers must not rely on
//!   exact values across cancel or crash reload.
//!
//! Crash-atomic boundary: one batch is the atomic unit. The commit appends
//! the whole batch to the write-ahead log before applying anything, and the
//! replay is idempotent, so a crash replays to either the whole batch visible
//! or the whole batch invisible. A torn apply never leaves single-direction
//! topology (out without in) or timestamps without topology: the load audit
//! rejects such residue fail-closed. Large fanouts (for example deleting
//! every edge of a high-degree vertex) commit as one batch with the same
//! all-or-nothing guarantee; a mid-batch failure rolls the applied prefix
//! back instead of leaving a half-deleted graph.
//
//! Commit organization: the apply stays one serialized pass under the
//! single-writer discipline, but reservation and observability are
//! group-local. `reserve_topology_for_inserts` sizes each touched row once at
//! the packed density target (`PACKED_CSR_DENSITY = 0.8`, fixed with no
//! tunable set), and `staging_group_plan` plus `write_contention_snapshot`
//! expose the per-owner split and the write skew. The skew snapshot is the
//! contention benchmark gate: partitioned lock-free applies or vertex-level
//! locks are only introduced when it proves a bottleneck with measured data,
//! never speculatively.
//!
//! Capacity contract: one batch is one atomic unit and holds the table lock
//! for the whole apply with prefix rollback on failure, so commit latency
//! grows with batch size. Batches holding tens of thousands of entries are
//! correct; callers with very large fanouts chunk explicitly and treat each
//! chunk as its own atomic unit. The `edge_group_commit_bench` records
//! per-batch commit times across distributions and sizes, and the write-gate
//! bench records the gate-wait share bounding the sharding decision;
//! chunking follows those measurements, never a parallel-write change.

use graphdb_core::types::{EdgeId, Timestamp};
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

/// Position of one staged entry inside its insert/delete store.
///
/// Preserves stage order across the split insert/delete vectors so commit can
/// apply entries sequentially and prevalidation can compute the batch net
/// effect. `slot` indexes into the matching vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StagedOrder {
    pub is_insert: bool,
    pub slot: usize,
}

/// Isolated buffer for one atomic group of edge writes.
///
/// Entries stay invisible to reads until committed. Committing applies entries
/// in stage order or reports the failure with no partial residue from this
/// batch left behind. An insert followed by a delete of the same key cancels
/// without a tombstone; a delete followed by an insert of the same key
/// rebuilds.
#[derive(Debug, Clone, Default)]
pub struct EdgeStagingBatch {
    inserts: Vec<StagedInsert>,
    deletes: Vec<StagedDelete>,
    order: Vec<StagedOrder>,
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
        let slot = self.inserts.len();
        self.inserts.push(StagedInsert {
            src,
            dst,
            rank,
            properties: properties.to_vec(),
            create_ts,
        });
        self.order.push(StagedOrder {
            is_insert: true,
            slot,
        });
    }

    /// Buffer one delete. No shared state is touched.
    pub fn stage_delete(&mut self, src: u32, dst: u32, rank: i64, delete_ts: Timestamp) {
        let slot = self.deletes.len();
        self.deletes.push(StagedDelete {
            src,
            dst,
            rank,
            delete_ts,
        });
        self.order.push(StagedOrder {
            is_insert: false,
            slot,
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

    /// Read-your-write check before commit: does the batch net effect hold an insert
    /// for `(src, dst, rank)`?
    ///
    /// Pre-commit helper only, never a read-path merge. The net effect walks
    /// stage order: a later delete cancels an earlier insert of the same key,
    /// and a later insert rebuilds after a delete, matching commit decisions.
    pub fn contains_insert(&self, src: u32, dst: u32, rank: i64) -> bool {
        let mut net_insert = false;
        for ord in &self.order {
            let (key_src, key_dst, key_rank) = if ord.is_insert {
                let ins = &self.inserts[ord.slot];
                (ins.src, ins.dst, ins.rank)
            } else {
                let del = &self.deletes[ord.slot];
                (del.src, del.dst, del.rank)
            };
            if key_src != src || key_dst != dst || key_rank != rank {
                continue;
            }
            if ord.is_insert {
                net_insert = true;
            } else if net_insert {
                net_insert = false;
            }
        }
        net_insert
    }

    /// Check whether the batch net effect holds a delete for `(src, dst, rank)`.
    ///
    /// Same pre-commit scope as `contains_insert`: no read-path visibility,
    /// only batch net-effect inspection. Cancelled pairs report false.
    pub fn contains_delete(&self, src: u32, dst: u32, rank: i64) -> bool {
        let mut net_insert = false;
        let mut net_delete = false;
        for ord in &self.order {
            let (key_src, key_dst, key_rank) = if ord.is_insert {
                let ins = &self.inserts[ord.slot];
                (ins.src, ins.dst, ins.rank)
            } else {
                let del = &self.deletes[ord.slot];
                (del.src, del.dst, del.rank)
            };
            if key_src != src || key_dst != dst || key_rank != rank {
                continue;
            }
            if ord.is_insert {
                net_insert = true;
                net_delete = false;
            } else if net_insert {
                net_insert = false;
            } else {
                net_delete = true;
            }
        }
        net_delete
    }

    /// Stage order for sequential commit and net-effect prevalidation.
    pub(crate) fn ordered(&self) -> &[StagedOrder] {
        &self.order
    }

    pub(crate) fn take_inserts(&mut self) -> Vec<StagedInsert> {
        std::mem::take(&mut self.inserts)
    }

    pub(crate) fn take_deletes(&mut self) -> Vec<StagedDelete> {
        std::mem::take(&mut self.deletes)
    }

    pub(crate) fn take_order(&mut self) -> Vec<StagedOrder> {
        std::mem::take(&mut self.order)
    }

    /// Clear all buffered entries for caller-side reuse.
    ///
    /// Discards the net effect without touching committed state so a hot
    /// caller can stage the next batch into the same allocation instead of
    /// building a new batch per commit.
    pub fn clear(&mut self) {
        self.inserts.clear();
        self.deletes.clear();
        self.order.clear();
    }
}

/// Reusable commit working buffers owned by the table.
///
/// One commit pass needs an applied-insert list, an insert key index for
/// same-batch cancel detection, and an applied-delete list. Keeping them on
/// the table and clearing instead of reallocating removes three
/// allocations plus repeated hash-table growth from every small commit.
/// Buffers never cross a commit boundary with live contents: each commit
/// takes them empty and returns them empty on every exit path.
/// Applied entry: `(src, dst, rank, edge_id, ts)`.
type AppliedEdge = (u32, u32, i64, EdgeId, Timestamp);

/// Edge identity key inside commit scratch: `(src, dst, rank)`.
type EdgeKey = (u32, u32, i64);

#[derive(Debug, Default)]
pub(crate) struct CommitScratch {
    pub applied_inserts: Vec<AppliedEdge>,
    pub insert_by_key: std::collections::HashMap<EdgeKey, AppliedEdge>,
    pub applied_deletes: Vec<AppliedEdge>,
}

impl CommitScratch {
    pub fn take(&mut self) -> CommitScratch {
        std::mem::take(self)
    }

    pub fn reset(&mut self, inserts: usize, deletes: usize) {
        self.applied_inserts.clear();
        self.applied_inserts.reserve(inserts);
        self.insert_by_key.clear();
        self.insert_by_key.reserve(inserts);
        self.applied_deletes.clear();
        self.applied_deletes.reserve(deletes);
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

    #[test]
    fn staging_contains_follows_net_effect() {
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(0, 1, 0, &[], 100);
        assert!(batch.contains_insert(0, 1, 0));
        assert!(!batch.contains_delete(0, 1, 0));
        batch.stage_delete(0, 1, 0, 150);
        assert!(!batch.contains_insert(0, 1, 0));
        assert!(!batch.contains_delete(0, 1, 0));
        batch.stage_delete(0, 2, 0, 150);
        assert!(batch.contains_delete(0, 2, 0));
        batch.stage_insert(0, 2, 0, &[], 160);
        assert!(batch.contains_insert(0, 2, 0));
        assert!(!batch.contains_delete(0, 2, 0));
    }
}
