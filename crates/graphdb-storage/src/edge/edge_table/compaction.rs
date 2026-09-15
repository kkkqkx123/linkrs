//! Compaction: CSR rebuilds and property slot reclamation.
//!
//! All cutoffs derive from one per-pass `MvccWatermarks` capture plus margin;
//! eligibility uses `Visibility::is_gc_eligible`.

use super::core::EdgeStore;
use super::stats::DeletionStats;
use crate::edge::csr_trait::CsrBase;
use graphdb_core::types::Timestamp;

impl EdgeStore {
    /// Compact the single-segment CSR, sharing one pass cutoff across all
    /// sub-systems. Deletions are promoted into the tombstone layer.
    pub fn compact_csr_only_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        reserve_ratio: f32,
    ) -> usize {
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        let mut removed_edges = std::collections::HashSet::new();
        self.out_csr
            .compact_with_ts_reporting(cutoff, reserve_ratio, &mut |edge_id, delete_ts| {
                removed_edges.insert(edge_id);
                self.mvcc.record_deletion(edge_id, delete_ts);
            });
        self.in_csr
            .compact_with_ts_reporting(cutoff, reserve_ratio, &mut |edge_id, delete_ts| {
                removed_edges.insert(edge_id);
                self.mvcc.record_deletion(edge_id, delete_ts);
            });
        removed_edges.len()
    }

    /// Compact the CSR when fragmentation exceeds `threshold`, sharing the
    /// pass cutoff. Used before flush to reduce memory usage.
    pub fn maybe_compact_for_flush_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        threshold: f32,
    ) {
        const RESERVE_RATIO: f32 = 0.25;
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        let out_stats = self.out_csr.fragmentation_stats();
        let in_stats = self.in_csr.fragmentation_stats();
        if out_stats
            .as_ref()
            .is_some_and(|s| s.should_compact(threshold))
            || self.out_csr.fragmentation_ratio() >= threshold
        {
            self.out_csr.compact_with_ts_reporting(
                cutoff,
                RESERVE_RATIO,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
            );
        }
        if in_stats
            .as_ref()
            .is_some_and(|s| s.should_compact(threshold))
            || self.in_csr.fragmentation_ratio() >= threshold
        {
            self.in_csr.compact_with_ts_reporting(
                cutoff,
                RESERVE_RATIO,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
            );
        }
    }

    /// Compact properties by reclaiming slots of unreferenced dead records.
    ///
    /// Live rows keep their positions, so edge-to-row mappings never need
    /// remapping.
    pub fn compact_properties_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        self.compact_properties(bound);
    }

    pub fn compact_properties(&mut self, bound: Timestamp) {
        let mut valid_edge_ids = std::collections::HashSet::new();
        for (edge_id, _pos) in self.properties.edge_mappings() {
            // Authoritative visibility, not the tombstone table alone: a
            // tombstone reclaimed by an earlier GC round must not resurrect
            // its edge as live here.
            if self.mvcc.is_edge_visible(*edge_id, bound) {
                valid_edge_ids.insert(*edge_id);
            }
        }

        let reclaimed = self.properties.reclaim_slots(&valid_edge_ids, bound);
        if reclaimed > 0 {
            log::debug!("Property slot reclaim recycled {} row(s)", reclaimed);
        }
    }

    /// Get deletion statistics for the single-segment table.
    pub fn deletion_stats(&self) -> DeletionStats {
        let tombstone_ts = self.mvcc.tombstones.values().copied();
        let oldest = tombstone_ts.clone().min();
        let newest = tombstone_ts.max();
        DeletionStats {
            total_live_edges: self.out_csr.edge_count(),
            total_deleted_edges: self.mvcc.tombstones.len() as u64,
            oldest_deletion_ts: oldest,
            newest_deletion_ts: newest,
        }
    }
}
