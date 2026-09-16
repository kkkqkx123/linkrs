//! Compaction: per-group CSR rebuilds and property slot reclamation.
//!
//! All cutoffs derive from one per-pass `MvccWatermarks` capture plus margin;
//! eligibility uses `Visibility::is_gc_eligible`. Reclaim passes skip groups
//! without delete history; flush-time rebuilds touch only dirty or
//! fragmented groups. Whole-table fragmentation stays an observation metric.

use super::core::EdgeStore;
use super::stats::DeletionStats;
use crate::edge::csr_trait::{CsrBase, MutableCsrTrait};
use graphdb_core::types::Timestamp;

/// Upper bound of rows reclaimed in one write-path maintenance pass, so a
/// small write never triggers a large rebuild pause.
pub(crate) const MAX_VERTEX_RECLAIM_PER_PASS: usize = 32;

impl EdgeStore {
    /// Whether any group is fragmented enough to justify a rebuild.
    ///
    /// Scans each group with its own capacity/live ratio; the whole-table
    /// ratio stays an observation metric and never triggers collection.
    /// A fragmented clean group still triggers, so a dirty group cannot
    /// drag clean groups into the rebuild set.
    pub fn has_fragmented_group(&self, threshold: f32) -> bool {
        for gid in 0..self.out_csr.group_count() {
            if self
                .out_csr
                .group_variant(gid)
                .is_some_and(|variant| variant.fragmentation_ratio() >= threshold)
            {
                return true;
            }
        }
        for gid in 0..self.in_csr.group_count() {
            if self
                .in_csr
                .group_variant(gid)
                .is_some_and(|variant| variant.fragmentation_ratio() >= threshold)
            {
                return true;
            }
        }
        false
    }

    /// Compact every group, sharing one pass cutoff across all sub-systems.
    /// Deletions are promoted into the tombstone layer. Explicit maintenance
    /// only; the write path uses the bounded reclaim pass below.
    pub fn compact_csr_only_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        reserve_ratio: f32,
    ) -> usize {
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        let mut removed_edges = std::collections::HashSet::new();
        for gid in 0..self.out_csr.group_count() {
            self.out_csr.compact_group_with_reporting(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| {
                    removed_edges.insert(edge_id);
                    self.mvcc.record_deletion(edge_id, delete_ts);
                },
            );
        }
        for gid in 0..self.in_csr.group_count() {
            self.in_csr.compact_group_with_reporting(
                gid,
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| {
                    removed_edges.insert(edge_id);
                    self.mvcc.record_deletion(edge_id, delete_ts);
                },
            );
        }
        removed_edges.len()
    }

    /// Compact one row in place, sharing the pass cutoff.
    ///
    /// Only the given vertex is touched; every other row keeps its offsets
    /// and data. Deletions are promoted into the tombstone layer.
    pub fn compact_vertex_with_watermarks(
        &mut self,
        vid: u32,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
    ) -> usize {
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let mut removed_edges = std::collections::HashSet::new();
        self.out_csr
            .compact_vertex_with_reporting(vid, cutoff, &mut |edge_id, delete_ts| {
                removed_edges.insert(edge_id);
                self.mvcc.record_deletion(edge_id, delete_ts);
            });
        self.in_csr
            .compact_vertex_with_reporting(vid, cutoff, &mut |edge_id, delete_ts| {
                removed_edges.insert(edge_id);
                self.mvcc.record_deletion(edge_id, delete_ts);
            });
        removed_edges.len()
    }

    /// Reclaim rows holding entries eligible at `bound`, visiting at most
    /// `max_vertices` rows.
    ///
    /// Groups without delete history are skipped without per-row
    /// inspection, so the pause stays proportional to the dirty groups
    /// rather than the size of the table.
    pub fn compact_reclaimable_vertices(&mut self, bound: Timestamp, max_vertices: usize) -> usize {
        if bound == Timestamp::MAX || max_vertices == 0 {
            return 0;
        }
        let group_count = self.out_csr.group_count().max(self.in_csr.group_count());
        let group_bits = self.out_csr.group_bits();
        let mut removed_edges = std::collections::HashSet::new();
        let mut visited = 0usize;
        for gid in 0..group_count {
            if visited >= max_vertices {
                break;
            }
            let out_scan =
                gid < self.out_csr.group_count() && self.out_csr.group_needs_reclaim_scan(gid);
            let in_scan =
                gid < self.in_csr.group_count() && self.in_csr.group_needs_reclaim_scan(gid);
            if !out_scan && !in_scan {
                continue;
            }
            let rows = crate::edge::node_group::group_size(group_bits);
            let base = crate::edge::node_group::group_base(gid, group_bits);
            let mut group_done = true;
            let mut any_dead = false;
            for local in 0..rows {
                if visited >= max_vertices {
                    group_done = false;
                    break;
                }
                let vid = base.saturating_add(local as u32);
                if out_scan && gid < self.out_csr.group_count() {
                    let (_, dead, _) = self.out_csr.vertex_census(vid);
                    any_dead |= dead > 0;
                }
                if in_scan && gid < self.in_csr.group_count() {
                    let (_, dead, _) = self.in_csr.vertex_census(vid);
                    any_dead |= dead > 0;
                }
                let mut needs = false;
                if out_scan && gid < self.out_csr.group_count() {
                    needs |= self.out_csr.vertex_needs_compact(vid, bound);
                }
                if !needs && in_scan && gid < self.in_csr.group_count() {
                    needs |= self.in_csr.vertex_needs_compact(vid, bound);
                }
                if !needs {
                    continue;
                }
                visited += 1;
                self.out_csr.compact_vertex_with_reporting(
                    vid,
                    bound,
                    &mut |edge_id, delete_ts| {
                        removed_edges.insert(edge_id);
                        self.mvcc.record_deletion(edge_id, delete_ts);
                    },
                );
                self.in_csr
                    .compact_vertex_with_reporting(vid, bound, &mut |edge_id, delete_ts| {
                        removed_edges.insert(edge_id);
                        self.mvcc.record_deletion(edge_id, delete_ts);
                    });
            }
            // Keep the hint while any tombstone physically remains, even when
            // the current bound covers nothing yet. Clearing early would drop
            // tail groups that a later watermark could still reclaim.
            if group_done && !any_dead {
                if out_scan {
                    self.out_csr.clear_reclaim_hint(gid);
                }
                if in_scan {
                    self.in_csr.clear_reclaim_hint(gid);
                }
            }
        }
        removed_edges.len()
    }

    /// Write-path incremental reclaim pass.
    ///
    /// Returns true when any row was reclaimed. Skipped entirely while no
    /// tombstone exists, so insert-only workloads pay no scan cost.
    pub(crate) fn run_vertex_reclaim_pass(&mut self, bound: Timestamp) -> bool {
        if bound == Timestamp::MAX || self.mvcc.total_tombstone_count() == 0 {
            return false;
        }
        self.compact_reclaimable_vertices(bound, MAX_VERTEX_RECLAIM_PER_PASS) > 0
    }

    /// Rebuild fragmented groups before flush, sharing the pass cutoff.
    /// Clean groups are skipped, so a small dirty set never triggers a
    /// large rebuild pause.
    pub fn maybe_compact_for_flush_with_watermarks(
        &mut self,
        watermarks: &graphdb_transaction::MvccWatermarks,
        margin: Timestamp,
        threshold: f32,
    ) {
        // Row-gap reserve derived from the packed-row density target, so
        // rebuilt rows keep everyday-write gaps instead of packing full.
        const RESERVE_RATIO: f32 = 1.0 - crate::edge::mutable_csr::PACKED_CSR_DENSITY;
        let cutoff = watermarks.safe_gc_timestamp_with_margin(margin);
        for gid in 0..self.out_csr.group_count() {
            let fragmented = self
                .out_csr
                .group_variant(gid)
                .and_then(|variant| variant.fragmentation_stats())
                .is_some_and(|stats| stats.should_compact(threshold));
            if fragmented {
                self.out_csr.compact_group_with_reporting(
                    gid,
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                );
            }
        }
        for gid in 0..self.in_csr.group_count() {
            let fragmented = self
                .in_csr
                .group_variant(gid)
                .and_then(|variant| variant.fragmentation_stats())
                .is_some_and(|stats| stats.should_compact(threshold));
            if fragmented {
                self.in_csr.compact_group_with_reporting(
                    gid,
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                );
            }
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
            self.mark_properties_dirty();
            log::debug!("Property slot reclaim recycled {} row(s)", reclaimed);
        }
    }

    /// Get deletion statistics for the sharded table.
    pub fn deletion_stats(&self) -> DeletionStats {
        let stats = self.mvcc.tombstone_stats();
        DeletionStats {
            total_live_edges: self.out_csr.edge_count(),
            total_deleted_edges: stats.count as u64,
            oldest_deletion_ts: stats.oldest_delete_ts,
            newest_deletion_ts: stats.newest_delete_ts,
        }
    }
}
