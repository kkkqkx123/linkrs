//! Compaction Operations
//!
//! Handles CSR compaction, property table compaction, and deletion statistics.
//! Compaction removes deleted edges and reclaims memory, while maintaining
//! MVCC visibility guarantees through proper timestamp tracking.

use super::core::EdgeTableCore;
use super::stats::DeletionStats;
use super::segment::DeletionInfo;
use crate::core::types::{Timestamp, CompactConfig};
use crate::core::StorageResult;
use crate::storage::edge::{MutableCsrTrait, CsrBase};

/// Compaction mode for the unified compact_and_freeze pipeline.
///
/// Controls which steps are included in the compaction pipeline after the
/// common prefix (compact_csr → freeze → compact_properties → stats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionMode {
    /// Standard: compact + freeze + merge + compact_properties + stats.
    /// Merge uses in-place strategy (no physical deletion, no GC).
    Standard,
    /// Auto GC: Standard + mvcc.gc_tombstones(min_active_snapshot_ts).
    /// Tombstone metadata (not edges) is cleaned up after property compaction.
    AutoGC,
    /// Physical deletion: Standard + merge with deletion filter + gc_tombstones.
    /// Tombstoned edges are physically removed during segment merge.
    PhysicalDeletion,
}

impl EdgeTableCore {
    /// Compact mutable CSR only - physical removal of deleted edges from delta.
    ///
    /// Removes edges marked as deleted from both out and in mutable CSRs.
    /// This is Layer 1 of the three-layer deletion model.
    ///
    /// # Layer 1: Mutable CSR Deletion
    ///
    /// - Scope: Only operates on out_csr and in_csr (delta CSRs)
    /// - What it does: Physically removes entries marked as deleted in mvcc.tombstones
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
    /// Returns number of edges removed.
    pub fn compact_csr_only(&mut self, ts: Timestamp, reserve_ratio: f32) -> usize {
        self.out_csr.compact_with_ts(ts, reserve_ratio)
            + self.in_csr.compact_with_ts(ts, reserve_ratio)
    }

    /// Compact mutable CSRs if fragmentation exceeds threshold.
    ///
    /// Useful before flushing to disk to reduce memory usage.
    pub fn maybe_compact_for_flush(&mut self, ts: Timestamp, threshold: f32) {
        const RESERVE_RATIO: f32 = 0.25;
        if self.out_csr.fragmentation_ratio() > threshold {
            self.out_csr.compact_with_ts(ts, RESERVE_RATIO);
        }
        if self.in_csr.fragmentation_ratio() > threshold {
            self.in_csr.compact_with_ts(ts, RESERVE_RATIO);
        }
    }

    /// Compact properties by removing unused property records.
    ///
    /// Identifies all valid property offsets referenced by edges in the table,
    /// then compacts the property table to reclaim space from unused records.
    /// Automatically applies adaptive compaction strategy based on fragmentation.
    ///
    /// # Handling of Deleted Edges
    ///
    /// This method correctly skips property records for edges marked as tombstoned
    /// via is_tombstoned(). For edges deleted in immutable segments:
    /// - Logical deletion: Marked in segment.deletion_info (visible via is_tombstoned)
    /// - Physical deletion: Requires segment merge with deletion_filter
    ///
    /// This method only removes properties that are no longer referenced by any edge.
    ///
    /// # Fragmentation Management
    ///
    /// - Analyzes fragmentation statistics before compaction
    /// - Triggers automatic compaction when fragmentation > 30% or free list > 1000
    /// - Uses compact_with_relocation for full defragmentation when needed (frag > 50%)
    /// - Uses lightweight compact for minor cleanup when fragmentation is acceptable (30-50%)
    pub fn compact_properties(&mut self, ts: Timestamp) {
        let mut valid_offsets = std::collections::HashSet::new();

        // Collect valid offsets from out CSR delta
        for (_, nbr) in self.out_csr.iter(ts) {
            if nbr.prop_offset > 0 {
                valid_offsets.insert(nbr.prop_offset);
            }
        }

        // Collect valid offsets from out segments
        for segment in &self.out_segments {
            for (_, nbr) in segment.csr.iter() {
                if nbr.timestamp <= ts
                    && !self.mvcc.is_tombstoned(nbr.edge_id, ts)
                    && nbr.prop_offset > 0
                {
                    valid_offsets.insert(nbr.prop_offset);
                }
            }
        }

        // Collect valid offsets from in CSR delta
        for (_, nbr) in self.in_csr.iter(ts) {
            if nbr.prop_offset > 0 {
                valid_offsets.insert(nbr.prop_offset);
            }
        }

        // Collect valid offsets from in segments
        for segment in &self.in_segments {
            for (_, nbr) in segment.csr.iter() {
                if nbr.timestamp <= ts
                    && !self.mvcc.is_tombstoned(nbr.edge_id, ts)
                    && nbr.prop_offset > 0
                {
                    valid_offsets.insert(nbr.prop_offset);
                }
            }
        }

        // Analyze fragmentation to determine compaction strategy
        let prop_stats = self.properties.compaction_stats();

        // Adaptive compaction strategy based on fragmentation level
        match prop_stats.fragmentation_ratio() {
            // High fragmentation: full defragmentation with relocation
            frag if frag > 0.5 => {
                let _offset_mapping = self.properties.compact_with_relocation(&valid_offsets);
            }
            // Medium fragmentation or large free list: standard compaction
            frag if frag > 0.3 || prop_stats.free_list_size > 1000 => {
                self.properties.compact(&valid_offsets);
            }
            // Low fragmentation: skip compaction, just track stats
            _ => {
                // No compaction needed, but could record stats if needed
            }
        }
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
            let edge_count = segment.csr.edge_count();
            total_edge_count += edge_count;

            match segment.deletion_info {
                DeletionInfo::NoDeletes => {}
                DeletionInfo::HasDeletes { min_ts, max_ts, deleted_count } => {
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
        self.out_segments.iter().map(|s| s.estimated_bytes()).sum::<usize>()
            + self.in_segments.iter().map(|s| s.estimated_bytes()).sum::<usize>()
    }

    /// Compact and freeze with physical deletion at specified timestamp.
    ///
    /// Convenience wrapper around [`compact_and_freeze`] with `CompactionMode::PhysicalDeletion`.
    #[deprecated(since = "0.3.0", note = "use compact_and_freeze with CompactionMode::PhysicalDeletion instead")]
    pub fn compact_and_freeze_with_physical_deletion(&mut self, ts: Timestamp, config: &CompactConfig) -> usize {
        self.compact_and_freeze(ts, config, CompactionMode::PhysicalDeletion)
    }

    /// Unified compaction pipeline with configurable mode.
    ///
    /// Provides a single entry point for all compaction variants:
    ///
    /// | Mode | Description | Steps |
    /// |------|-------------|-------|
    /// | `Standard` | Basic inline merge | compact_csr → freeze → merge (in-place) → compact_properties → stats |
    /// | `AutoGC` | + tombstone GC | Standard + gc_tombstones |
    /// | `PhysicalDeletion` | + physical edge removal | compact_csr → freeze → merge (with deletion filter) → compact_properties → gc_tombstones → stats |
    ///
    /// Returns number of edges removed from mutable CSR during Layer 1 compaction.
    ///
    /// # Deletion Lifecycle (PhysicalDeletion mode)
    ///
    /// - **Layer 1** (Mutable CSR): compact_csr_only() removes tombstoned entries from delta
    /// - **Layer 2** (Frozen Segments): merge with deletion filter removes edges deleted before min_active_snapshot_ts
    /// - **Layer 3** (Property Table): compact_properties() reclaims unused property offsets
    pub fn compact_and_freeze(&mut self, ts: Timestamp, config: &CompactConfig, mode: CompactionMode) -> usize {
        let edge_count = self.edge_count() as usize;
        let reserve_ratio = config.compute_reserve_ratio(edge_count, 0);

        // Layer 1: Remove deleted edges from mutable CSR
        let removed = self.compact_csr_only(ts, reserve_ratio);

        // Freeze mutable CSR to immutable segments
        self.freeze_csr_only(ts);

        // Layer 2: Merge segments
        if config.segment_merge_enabled {
            let stats = self.mvcc.tombstone_stats();
            let merge_threshold = config.compute_merge_size_threshold(stats.memory_bytes);

            match mode {
                CompactionMode::PhysicalDeletion => {
                    let min_active_snapshot_ts = self.mvcc.min_active_snapshot_ts;
                    self.merge_segments_with_config_and_deletion_filter(
                        config.segment_merge_threshold,
                        merge_threshold,
                        if min_active_snapshot_ts < u32::MAX {
                            Some(min_active_snapshot_ts)
                        } else {
                            None
                        },
                    );
                }
                _ => {
                    self.merge_segments_with_config(config.segment_merge_threshold, merge_threshold);
                }
            }
        }

        // Layer 3: Compact property table to reclaim unused offsets
        self.compact_properties(ts);

        // GC tombstones (AutoGC and PhysicalDeletion modes)
        if mode == CompactionMode::AutoGC || mode == CompactionMode::PhysicalDeletion {
            let min_ts = self.mvcc.get_min_active_snapshot_ts();
            self.mvcc.gc_tombstones(min_ts);
        }

        // Record statistics
        if let Some(stats) = &self.stats_manager {
            let tom_stats = self.mvcc.tombstone_stats();
            stats.record_tombstone_stats(
                tom_stats.count as u64,
                tom_stats.memory_bytes as u64,
                tom_stats.oldest_delete_ts,
                tom_stats.newest_delete_ts,
                self.mvcc.active_snapshots.len() as u64,
            );
        }

        removed
    }
}
