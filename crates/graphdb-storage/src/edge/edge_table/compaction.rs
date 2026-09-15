//! Compaction Operations
//!
//! Handles single-segment CSR compaction and property slot reclamation.
//! Compaction physically removes deleted edges that no active snapshot can
//! observe, while MVCC visibility guarantees are preserved through the
//! centralized timestamp and tombstone state.

use super::core::EdgeStore;
use super::stats::DeletionStats;
use crate::edge::csr_trait::CsrBase;
use graphdb_core::types::Timestamp;

impl EdgeStore {
    /// Compact the single-segment CSR: physical removal of deleted edges.
    ///
    /// Removes entries whose deletion predates the active-snapshot cutoff
    /// (`delete_ts < min_active_snapshot_ts`) and promotes their deletion
    /// into the global tombstone layer. With no active snapshot
    /// (`cutoff == MAX`) no deletion is dropped; the CSR is still rebuilt to
    /// reclaim fragmentation.
    ///
    /// Unified watermark variant. Caller supplies the GC frontier captured at
    /// pass start so all sub-systems share the same cutoff.
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

    /// Returns number of distinct edges physically removed.
    ///
    /// Each edge is stored in both the out and in CSR, so the per-CSR removal
    /// counts are summed by edge identity rather than added directly; counting
    /// both would double-report every removed edge.
    pub fn compact_csr_only(&mut self, _ts: Timestamp, reserve_ratio: f32) -> usize {
        let cutoff = self.mvcc.min_active_snapshot_ts;
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

    /// Compact the single-segment CSR if fragmentation exceeds threshold.
    ///
    /// Uses `FragmentationStats::should_compact` for adaptive threshold decision.
    /// Useful before flushing to disk to reduce memory usage.
    pub fn maybe_compact_for_flush(&mut self, _ts: Timestamp, threshold: f32) {
        const RESERVE_RATIO: f32 = 0.25;
        let cutoff = self.mvcc.min_active_snapshot_ts;
        let out_stats = self.out_csr.fragmentation_stats();
        let in_stats = self.in_csr.fragmentation_stats();
        let out_wasted = self.out_csr.wasted_bytes_estimate();
        let in_wasted = self.in_csr.wasted_bytes_estimate();
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
            if let Some(ref stats) = out_stats {
                log::debug!(
                    "Compacted out_csr: fragmentation={:.2}, efficiency={:.2}, reclaimed={} bytes, wasted={}",
                    stats.fragmentation_ratio(),
                    stats.space_efficiency(),
                    stats.reclamation_potential(),
                    out_wasted
                );
            }
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
            if let Some(ref stats) = in_stats {
                log::debug!(
                    "Compacted in_csr: fragmentation={:.2}, efficiency={:.2}, reclaimed={} bytes, wasted={}",
                    stats.fragmentation_ratio(),
                    stats.space_efficiency(),
                    stats.reclamation_potential(),
                    in_wasted
                );
            }
        }
    }

    /// Compact properties by reclaiming slots of unreferenced dead records.
    ///
    /// Identifies all valid property offsets referenced by live edges, then
    /// reclaims the slots of tombstoned records that no live edge
    /// references. Live rows keep their positions, so edge-to-row
    /// mappings never need remapping.
    pub fn compact_properties_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) {
        let bound = watermarks.safe_gc_timestamp_with_margin(margin);
        self.compact_properties(bound);
    }

    pub fn compact_properties(&mut self, bound: Timestamp) {
        // Valid property rows are those whose EdgeId is still live at the GC
        // bound (not tombstoned).
        let mut valid_edge_ids = std::collections::HashSet::new();
        for (edge_id, _pos) in self.properties.edge_mappings() {
            if !self.mvcc.is_tombstoned(*edge_id, bound) {
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
