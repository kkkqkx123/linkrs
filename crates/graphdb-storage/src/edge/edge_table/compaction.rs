//! Compaction Operations
//!
//! Handles CSR compaction, property table compaction, and deletion statistics.
//! Compaction removes deleted edges and reclaims memory, while maintaining
//! MVCC visibility guarantees through proper timestamp tracking.

use super::core::TimeTravelEdgeStore;
use super::segment::DeletionInfo;
use super::stats::DeletionStats;
use graphdb_core::types::{CompactConfig, Timestamp};

use crate::edge::CsrBase;

impl TimeTravelEdgeStore {
    /// Compact mutable CSR only - physical removal of deleted edges from delta.
    ///
    /// Removes edges marked as deleted from both out and in mutable CSRs.
    /// This is Layer 1 of the three-layer deletion model.
    ///
    /// # Layer 1: Mutable CSR Deletion
    ///
    /// - Scope: Only operates on out_csr and in_csr (delta CSRs)
    /// - What it does: Physically removes entries whose deletion predates the
    ///   oldest active snapshot (delete_ts < min_active_snapshot_ts) and
    ///   promotes their deletion into the global tombstone layer. When no
    ///   active snapshot exists, deleted entries are kept so time-travel
    ///   queries before the deletion remain possible.
    /// - What it doesn't do: Does NOT freeze segments or merge them
    /// - Result: Immediate space reclamation in memory
    /// - When: Called before freeze to clean up the delta
    ///
    /// # Important Note
    ///
    /// Does NOT handle deletions in immutable segments. Segment deletions are handled by:
    /// - Layer 2: merge_segments_with_config_and_deletion_filter() - physical removal during merge
    /// - Layer 3: compact_properties() - reclaims unused property offsets
    ///
    /// The `ts` parameter is retained for API compatibility but does not gate
    /// removal: the active-snapshot cutoff governs what may be dropped.
    ///
    /// Returns number of edges removed.
    pub fn compact_csr_only(&mut self, _ts: Timestamp, reserve_ratio: f32) -> usize {
        let cutoff = self.mvcc.effective_retention_bound();
        let region_n = self.config.region_vertex_count;
        let removed_out = if region_n > 0 {
            self.out_csr.compact_regions_with_ts_reporting(
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                region_n,
            )
        } else {
            self.out_csr.compact_with_ts_reporting(
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
            )
        };
        let removed_in = if region_n > 0 {
            self.in_csr.compact_regions_with_ts_reporting(
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                region_n,
            )
        } else {
            self.in_csr.compact_with_ts_reporting(
                cutoff,
                reserve_ratio,
                &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
            )
        };
        removed_out + removed_in
    }

    /// Compact mutable CSRs if fragmentation exceeds threshold.
    ///
    /// Uses `FragmentationStats::should_compact` for adaptive threshold decision.
    /// Useful before flushing to disk to reduce memory usage.
    pub fn maybe_compact_for_flush(&mut self, _ts: Timestamp, threshold: f32) {
        const RESERVE_RATIO: f32 = 0.25;
        let cutoff = self.mvcc.effective_retention_bound();
        let out_stats = self.out_csr.fragmentation_stats();
        let in_stats = self.in_csr.fragmentation_stats();
        let out_wasted = self.out_csr.wasted_bytes_estimate();
        let in_wasted = self.in_csr.wasted_bytes_estimate();
        let region_n = self.config.region_vertex_count;
        if out_stats
            .as_ref()
            .is_some_and(|s| s.should_compact(threshold))
            || self.out_csr.fragmentation_ratio() >= threshold
        {
            if region_n > 0 {
                self.out_csr.compact_regions_with_ts_reporting(
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                    region_n,
                );
            } else {
                self.out_csr.compact_with_ts_reporting(
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                );
            }
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
            if region_n > 0 {
                self.in_csr.compact_regions_with_ts_reporting(
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                    region_n,
                );
            } else {
                self.in_csr.compact_with_ts_reporting(
                    cutoff,
                    RESERVE_RATIO,
                    &mut |edge_id, delete_ts| self.mvcc.record_deletion(edge_id, delete_ts),
                );
            }
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
    /// Identifies all valid property offsets referenced by edges in the table,
    /// then reclaims the slots of tombstoned records that no live edge
    /// references. Live rows keep their offsets, so CSR `prop_offset`
    /// pointers never need remapping.
    ///
    /// # Handling of Deleted Edges
    ///
    /// This method correctly skips property records for edges marked as tombstoned
    /// via is_tombstoned(). For edges deleted in immutable segments:
    /// - Logical deletion: Marked in segment.deletion_info (visible via is_tombstoned)
    /// - Physical deletion: Requires segment merge with deletion_filter
    ///
    /// This method only removes properties that are no longer referenced by any edge.
    pub fn compact_properties(&mut self, ts: Timestamp) {
        let mut valid_offsets = std::collections::HashSet::new();

        // Collect valid edge_ids from out CSR delta, then resolve to offsets
        for (_, nbr) in self.out_csr.iter(ts) {
            if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                valid_offsets.insert(offset);
            }
        }

        // Collect valid edge_ids from out segments, then resolve to offsets
        for segment in &self.out_segments {
            let has_dirty_region = segment.regions.iter().any(|r| r.deleted_count > 0);
            if has_dirty_region || segment.regions.is_empty() {
                for (_, nbr) in segment.csr.read().iter() {
                    if nbr.timestamp <= ts
                        && !self.mvcc.is_tombstoned(nbr.edge_id, ts)
                    {
                        if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                            valid_offsets.insert(offset);
                        }
                    }
                }
            } else {
                // All regions clean: no tombstone check needed
                for (_, nbr) in segment.csr.read().iter() {
                    if nbr.timestamp <= ts {
                        if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                            valid_offsets.insert(offset);
                        }
                    }
                }
            }
        }

        // Collect valid edge_ids from in CSR delta, then resolve to offsets
        for (_, nbr) in self.in_csr.iter(ts) {
            if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                valid_offsets.insert(offset);
            }
        }

        // Collect valid edge_ids from in segments, then resolve to offsets
        for segment in &self.in_segments {
            let has_dirty_region = segment.regions.iter().any(|r| r.deleted_count > 0);
            if has_dirty_region || segment.regions.is_empty() {
                for (_, nbr) in segment.csr.read().iter() {
                    if nbr.timestamp <= ts
                        && !self.mvcc.is_tombstoned(nbr.edge_id, ts)
                    {
                        if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                            valid_offsets.insert(offset);
                        }
                    }
                }
            } else {
                for (_, nbr) in segment.csr.read().iter() {
                    if nbr.timestamp <= ts {
                        if let Some(offset) = self.properties.get_offset_by_edge_id(nbr.edge_id) {
                            valid_offsets.insert(offset);
                        }
                    }
                }
            }
        }

        // Reclaim property slots whose rows are dead (tombstoned and no
        // longer referenced by any live edge). Slot reclamation keeps every
        // live row at a stable offset, so CSR `prop_offset` pointers stay
        // valid without any relocation mapping. The retention bound derives
        // from active snapshots / the operator floor: an unbounded bound
        // (MAX) reclaims nothing, preserving time-travel history.
        let bound = self.mvcc.effective_retention_bound();
        let reclaimed = self.properties.reclaim_slots(&valid_offsets, bound);
        if reclaimed > 0 {
            log::debug!("Property slot reclaim recycled {} row(s)", reclaimed);
        }

        // Reclaim before-image version chains that predate the retention
        // bound (part of the Edge version-chain lifecycle; snapshot reads at
        // or after the bound remain consistent).
        self.properties
            .gc_versions(self.mvcc.effective_retention_bound());
    }

    /// Get deletion statistics for all frozen segments.
    ///
    /// Analyzes frozen segments to report:
    /// - Number of segments with deletions
    /// - Segments that are completely deleted
    /// - Total deleted edge count
    /// - Oldest and newest deletion timestamps
    pub fn deletion_stats(&self) -> DeletionStats {
        let mut stats = DeletionStats::default();

        let mut total_edge_count = 0u64;
        let mut total_deleted_count = 0u64;

        for segment in self.out_segments.iter().chain(self.in_segments.iter()) {
            let edge_count = segment.csr.read().edge_count();
            total_edge_count += edge_count;

            match segment.deletion_info {
                DeletionInfo::NoDeletes => {}
                DeletionInfo::HasDeletes {
                    min_ts,
                    max_ts,
                    deleted_count,
                } => {
                    total_deleted_count += deleted_count as u64;
                    stats.segments_with_deletions += 1;

                    if (deleted_count as u64) == edge_count {
                        stats.completely_deleted_segments += 1;
                    }

                    if let Some(ref mut oldest) = stats.oldest_deletion_ts {
                        *oldest = (*oldest).min(min_ts);
                    } else {
                        stats.oldest_deletion_ts = Some(min_ts);
                    }

                    if let Some(ref mut newest) = stats.newest_deletion_ts {
                        *newest = (*newest).max(max_ts);
                    } else {
                        stats.newest_deletion_ts = Some(max_ts);
                    }
                }
            }
        }

        stats.total_frozen_edges = total_edge_count;
        stats.total_deleted_edges = total_deleted_count;

        stats
    }

    /// Get total memory used by all segments in bytes.
    pub fn segments_total_bytes(&self) -> usize {
        self.out_segments
            .iter()
            .map(|s| s.estimated_bytes())
            .sum::<usize>()
            + self
                .in_segments
                .iter()
                .map(|s| s.estimated_bytes())
                .sum::<usize>()
    }

    /// Unified compaction pipeline.
    ///
    /// Single entry point shared by every maintenance trigger (write-path
    /// tiers, background thread, manual/admin invocation):
    ///
    /// compact_csr → freeze → merge → compact_properties → tombstone GC → stats
    ///
    /// The reclamation strength is derived from the retention state instead
    /// of a caller-supplied mode:
    ///
    /// - With a bounded retention horizon (active snapshots or an operator
    ///   retention floor) the merge physically drops edges deleted before the
    ///   bound and tombstones older than it are GC'd.
    /// - Unbounded (`MAX`: no snapshot pins history, no floor configured) the
    ///   merge keeps every edge and GC is skipped, preserving full
    ///   time-travel history.
    ///
    /// Returns number of edges removed from mutable CSR during Layer 1 compaction.
    pub fn compact_and_freeze(&mut self, ts: Timestamp, config: &CompactConfig) -> usize {
        let edge_count = self.edge_count() as usize;
        let reserve_ratio = config.compute_reserve_ratio(edge_count, 0);

        // Layer 1: Remove deleted edges from mutable CSR
        let removed = self.compact_csr_only(ts, reserve_ratio);

        // Freeze mutable CSR to immutable segments
        self.freeze_csr_only(ts);

        // Layer 2: Merge segments. Physical deletion applies only under a
        // bounded retention horizon; otherwise the merge keeps every edge.
        if config.segment_merge_enabled {
            let stats = self.mvcc.tombstone_stats();
            let merge_threshold = config.compute_merge_size_threshold(stats.memory_bytes);
            let bound = self.mvcc.effective_retention_bound();

            let result = if bound < Timestamp::MAX {
                self.merge_segments_with_config_and_deletion_filter(
                    config.segment_merge_threshold,
                    merge_threshold,
                    Some(bound),
                )
            } else {
                self.merge_segments_with_config(config.segment_merge_threshold, merge_threshold)
            };
            if result.metrics.edges_merged > 0 {
                result.metrics.log();
                if result.segments_reduced > 0 {
                    log::debug!("Segments reduced: {}", result.segments_reduced);
                }
            }
            let total_bytes = self.segments_total_bytes();
            log::debug!("Segments total bytes after merge: {}", total_bytes);
        }

        // Layer 3: Compact property table to reclaim unused offsets
        self.compact_properties(ts);

        // GC tombstones — only meaningful under a bounded retention horizon;
        // an unbounded bound would wipe tombstones arbitrary-ts reads need.
        let bound = self.mvcc.effective_retention_bound();
        if bound < Timestamp::MAX {
            self.mvcc.gc_tombstones(bound);
        }

        // Record statistics
        if let Some(stats) = &self.stats_manager {
            let tom_stats = self.mvcc.tombstone_stats();
            stats.record_tombstone_stats(
                tom_stats.count as u64,
                tom_stats.memory_bytes as u64,
                tom_stats.oldest_delete_ts.map(|ts| ts as u32),
                tom_stats.newest_delete_ts.map(|ts| ts as u32),
                self.mvcc.active_snapshots.len() as u64,
            );
        }

        removed
    }
}
