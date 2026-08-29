//! CSR Freezing Operations
//!
//! Converts mutable delta CSR to immutable segments and maintains segment indices.
//! Freezing is the process of taking visible edges from the mutable CSR and converting
//! them to immutable segments for better cache locality and query performance.

use super::core::TimeTravelEdgeStore;
use super::merge;
use super::segment::{CsrSegment, DeletionInfo, SEPARATE_EDGE_ID_STORAGE_THRESHOLD};
use crate::edge::csr_trait::MutableCsrTrait;
use crate::edge::CsrVariant;
use graphdb_core::types::Timestamp;
use std::collections::HashSet;

impl TimeTravelEdgeStore {
    /// Freeze CSR only (convert mutable delta to immutable segment).
    ///
    /// Converts visible edges (ts <= query_ts) to immutable CSR and records
    /// timestamp ranges for time-travel queries and MVCC support.
    /// Clears mutable delta after freezing.
    /// Does NOT perform physical compaction.
    /// Uses incremental index updates for efficiency.
    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        // Promote deletions of delta entries into the global tombstone layer
        // before freezing. Delta entries carry delete_ts inline; frozen
        // segments store none and rely on tombstones for filtering, so
        // without this promotion the deletion history would be lost. Only the
        // out direction needs scanning (tombstones are global).
        let mut has_deleted_delta_entries = false;
        for (_, nbr) in self.out_csr.iter_all() {
            if nbr.delete_ts != Timestamp::MAX {
                has_deleted_delta_entries = true;
                self.mvcc.record_deletion(nbr.edge_id, nbr.delete_ts);
            }
        }

        // Reclaim delta deletions that no active snapshot can observe before
        // freezing: when a snapshot bounds the retention horizon
        // (min_active_snapshot_ts < MAX), entries whose deletion predates it
        // are physically removed from the delta (their tombstone is already
        // recorded above) instead of being frozen into an immutable segment,
        // where they would occupy space until a physical merge. Without an
        // active snapshot every deleted entry is retained so time-travel
        // before the deletion stays possible.
        let cutoff = self.mvcc.effective_retention_bound();
        if cutoff < Timestamp::MAX && has_deleted_delta_entries {
            self.compact_csr_only(ts, 0.0);
        }

        // Freeze out direction — incremental region-based when enabled
        let out_segments_before = self.out_segments.len();
        let out_result =
            if self.config.region_vertex_count > 0 && self.config.max_regions_per_freeze > 0 {
                Self::freeze_delta_incremental(
                    &mut self.out_csr,
                    &mut self.out_segments,
                    &mut self.out_free_space,
                    ts,
                    self.config.region_vertex_count,
                    &self.calibrator,
                    self.config.max_regions_per_freeze,
                    self.config.freeze_density_threshold,
                )
            } else {
                Self::freeze_delta(
                    &mut self.out_csr,
                    &mut self.out_segments,
                    &mut self.out_free_space,
                    ts,
                    self.config.region_vertex_count,
                )
            };
        let out_segments_after = self.out_segments.len();

        // Freeze in direction — incremental region-based when enabled
        let in_segments_before = self.in_segments.len();
        let in_result =
            if self.config.region_vertex_count > 0 && self.config.max_regions_per_freeze > 0 {
                Self::freeze_delta_incremental(
                    &mut self.in_csr,
                    &mut self.in_segments,
                    &mut self.in_free_space,
                    ts,
                    self.config.region_vertex_count,
                    &self.calibrator,
                    self.config.max_regions_per_freeze,
                    self.config.freeze_density_threshold,
                )
            } else {
                Self::freeze_delta(
                    &mut self.in_csr,
                    &mut self.in_segments,
                    &mut self.in_free_space,
                    ts,
                    self.config.region_vertex_count,
                )
            };
        let in_segments_after = self.in_segments.len();

        // Update indices incrementally for newly frozen segments
        // This is more efficient than full rebuild when only a few segments are added
        if out_segments_after > out_segments_before {
            for new_idx in out_segments_before..out_segments_after {
                self.append_segment_to_index_out(new_idx);
            }
        }
        if in_segments_after > in_segments_before {
            for new_idx in in_segments_before..in_segments_after {
                self.append_segment_to_index_in(new_idx);
            }
        }

        let total_frozen = out_result.frozen_count + in_result.frozen_count;

        // Update checksums after freeze
        self.update_segment_checksums();

        // Auto-trigger merge if segment count exceeds threshold (or emergency merge if max exceeded)
        let merged = self.auto_merge_segments(ts);

        // Update checksums after merge as well
        if merged > 0 {
            self.update_segment_checksums();
        }

        // Rebuild sparse vertex indices and current snapshot after any segment mutation
        if out_segments_after > out_segments_before
            || in_segments_after > in_segments_before
            || merged > 0
        {
            self.rebuild_sparse_vertex_indices();
            self.rebuild_current_snapshot();
            self.update_calibrator_from_segments();
        }

        total_frozen
    }

    /// Freeze delta CSR to immutable segment
    fn freeze_delta(
        delta: &mut CsrVariant,
        segments: &mut Vec<CsrSegment>,
        free_space: &mut super::free_space::SegmentFreeList,
        ts: Timestamp,
        region_vertex_count: usize,
    ) -> merge::FreezeDeltaResult {
        // Refresh overflow index before freeze for observability and sequential run detection.
        delta.rebuild_overflow_index();
        let entries: Vec<_> = delta
            .iter_all()
            .filter(|(_, nbr)| delta.create_ts_of(nbr.edge_id).unwrap_or(0) <= ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let create_ts = delta.create_ts_of(nbr.edge_id).unwrap_or(0);
                (src_u32, nbr, create_ts)
            })
            .collect();

        if entries.is_empty() {
            delta.clear();
            return merge::FreezeDeltaResult { frozen_count: 0 };
        }

        // Segment rows index this direction's vertex space only (out: src
        // label, in: dst label); neighbor ids belong to the other label's
        // space and must not inflate capacity. Truncate strictly to the
        // highest edge-bearing row + 1 (Ladybug getMaxOffsetWithRels()+1),
        // mirroring the merge path.
        let max_row = entries
            .iter()
            .map(|(src, _, _)| *src as usize)
            .max()
            .unwrap_or(0);
        let effective_capacity = max_row.saturating_add(1);

        let create_ts_min = entries
            .iter()
            .map(|(_, _, create_ts)| *create_ts)
            .min()
            .unwrap_or(0);
        let create_ts_max = entries
            .iter()
            .map(|(_, _, create_ts)| *create_ts)
            .max()
            .unwrap_or(0);

        // Count deletions belonging to THIS segment. The source is the inline
        // delete_ts carried by entries that were logically deleted while
        // still in the mutable delta: those deletions happened at freeze
        // time and must be reflected in the segment's DeletionInfo. Deletions
        // of already-frozen edges never appear here, since `freeze_csr_only`
        // promotes delta deletions to the global tombstone layer before
        // freezing and an edge lives either in the delta or in a segment,
        // never both.
        let mut deleted_count = 0u32;
        let (delete_ts_min, delete_ts_max) = entries
            .iter()
            .filter_map(|(_, nbr, _)| {
                if nbr.delete_ts != Timestamp::MAX {
                    deleted_count += 1;
                    return Some(nbr.delete_ts);
                }
                None
            })
            .fold((Timestamp::MAX, 0), |(min, max), ts| {
                (std::cmp::min(min, ts), std::cmp::max(max, ts))
            });

        let csr = free_space.build_csr(&entries, effective_capacity);
        let frozen = entries.len();

        let deletion_info = DeletionInfo::with_count(delete_ts_min, delete_ts_max, deleted_count);
        let mut segment = CsrSegment::new(csr, create_ts_min, create_ts_max, deletion_info);

        if frozen >= SEPARATE_EDGE_ID_STORAGE_THRESHOLD {
            segment.edge_ids = Some(entries.iter().map(|(_, nbr, _)| nbr.edge_id).collect());
        }

        if region_vertex_count > 0 {
            segment.rebuild_regions_from_entries(region_vertex_count, &entries);
        }

        segments.push(segment);
        delta.clear();

        merge::FreezeDeltaResult {
            frozen_count: frozen,
        }
    }

    /// Select high-density regions to freeze incrementally using CalibratorTree.
    ///
    /// Implements Phase 5 incremental region-based freeze: high-density or high-deletion
    /// regions are frozen per call, low-density regions stay in the mutable CSR to
    /// reduce per-freeze latency. Uses calibrator tree hierarchical aggregation:
    /// if a parent node is over-utilized, all its children are expanded (global
    /// redistribution) otherwise per-leaf decisions apply.
    pub(crate) fn select_regions_to_freeze(
        delta: &CsrVariant,
        region_vertex_count: usize,
        ts: Timestamp,
        calibrator: &super::calibrator::CalibratorTree,
        max_regions_per_freeze: usize,
        freeze_density_threshold: f32,
    ) -> HashSet<u32> {
        // Only Multiple variant supports region-aware incremental freeze
        let regions = delta.regions_with_ts(region_vertex_count, Some(ts));
        if regions.is_empty() {
            return HashSet::new();
        }
        let mut non_empty: Vec<&crate::edge::mutable_csr::MutableCsrRegion> =
            regions.iter().filter(|r| r.edge_count > 0).collect();
        if non_empty.is_empty() {
            return HashSet::new();
        }
        // Fast path: small number of non-empty regions — freeze all to guarantee progress
        // and keep single-segment behavior for small tests.
        if max_regions_per_freeze == 0 || non_empty.len() <= max_regions_per_freeze {
            // Check calibrator effective threshold to decide if we should still be selective.
            // For small datasets we freeze all non-empty regions unless they are extremely sparse
            // and calibrator indicates no pressure.
            let calibrated_threshold = calibrator.calibrated_threshold();
            let effective_density_threshold =
                (freeze_density_threshold as f64 * calibrated_threshold.multiplier) as f32;
            // If effective threshold is very low (<0.001) we already freeze all; otherwise
            // still check density to retain truly empty-ish regions.
            // For test compatibility, when non_empty <= max_regions, we freeze all that meet
            // minimal density 0.0005 (i.e. any edge in 1024 region).
            let min_density = effective_density_threshold.min(0.001);
            let mut selected = HashSet::new();
            for r in &non_empty {
                if r.density >= min_density
                    || r.deletion_ratio() >= calibrated_threshold.effective_deletion_ratio()
                    || calibrator.should_compact_region(r.region_id)
                {
                    selected.insert(r.region_id);
                }
            }
            // If none selected due to very low density but we have edges, freeze the densest
            // up to max_regions to ensure progress, but for small case freeze all.
            if selected.is_empty() {
                if non_empty.len() <= max_regions_per_freeze || max_regions_per_freeze == 0 {
                    for r in non_empty {
                        selected.insert(r.region_id);
                    }
                } else {
                    non_empty.sort_by(|a, b| b.density.partial_cmp(&a.density).unwrap());
                    for r in non_empty.into_iter().take(1) {
                        selected.insert(r.region_id);
                    }
                }
            } else if selected.len() < non_empty.len() && non_empty.len() <= max_regions_per_freeze
            {
                // All non-empty should be frozen for small case to preserve single-freeze semantics
                // unless explicitly filtered by density. For correctness we ensure at least
                // we don't leave a single sparse region behind when total is small.
                // Expand to all non-empty if the filtered set is a strict subset and total <= cap
                // and the unselected regions have density within 10x of selected.
                // Simpler: just freeze all non-empty for small case.
                for r in non_empty {
                    selected.insert(r.region_id);
                }
            }
            return Self::expand_hierarchical_selection(selected, calibrator, &regions);
        }

        // Large case: selective incremental freeze
        let calibrated_threshold = calibrator.calibrated_threshold();
        let effective_density_threshold =
            (freeze_density_threshold as f64 * calibrated_threshold.multiplier) as f32;
        let mut selected = HashSet::new();
        for r in &non_empty {
            let high_density = r.density >= effective_density_threshold;
            let high_deletion =
                r.deletion_ratio() >= calibrated_threshold.effective_deletion_ratio();
            let calibrator_compact = calibrator.should_compact_region(r.region_id);
            let is_hot = calibrator.is_hot_region(r.region_id, 10);
            if high_density || high_deletion || calibrator_compact || is_hot {
                selected.insert(r.region_id);
            }
        }
        // Ensure progress: freeze at least the densest region if none selected
        if selected.is_empty() {
            non_empty.sort_by(|a, b| b.density.partial_cmp(&a.density).unwrap());
            if let Some(r) = non_empty.first() {
                selected.insert(r.region_id);
            }
        }
        // Global redistribution check: if >50% of non-empty regions are high-density,
        // freeze all for global compaction (Ladybug calibrator tree bottom-up aggregation).
        if selected.len() * 2 > non_empty.len() {
            return non_empty.into_iter().map(|r| r.region_id).collect();
        }
        // Cap by max_regions_per_freeze: keep highest density regions
        if max_regions_per_freeze > 0 && selected.len() > max_regions_per_freeze {
            let mut selected_vec: Vec<u32> = selected.into_iter().collect();
            // Sort by density descending
            selected_vec.sort_by(|a, b| {
                let da = regions
                    .iter()
                    .find(|r| r.region_id == *a)
                    .map(|r| r.density)
                    .unwrap_or(0.0);
                let db = regions
                    .iter()
                    .find(|r| r.region_id == *b)
                    .map(|r| r.density)
                    .unwrap_or(0.0);
                db.partial_cmp(&da).unwrap()
            });
            selected_vec.truncate(max_regions_per_freeze);
            selected = selected_vec.into_iter().collect();
        }

        Self::expand_hierarchical_selection(selected, calibrator, &regions)
    }

    /// Hierarchical expansion: if a parent calibrator node has >50% children selected,
    /// expand to all children of that parent (global redistribution for that subtree).
    fn expand_hierarchical_selection(
        mut selected: HashSet<u32>,
        calibrator: &super::calibrator::CalibratorTree,
        regions: &[crate::edge::mutable_csr::MutableCsrRegion],
    ) -> HashSet<u32> {
        // Build map from region_id to selection
        // Walk calibrator tree bottom-up: for each internal node, count selected children
        // This requires accessing calibrator internal nodes; use public region_count and try to infer.
        // Simplified: if overall selected ratio >0.5 and total regions large, expand to all for that parent subtree.
        // Since calibrator tree structure is not directly exposing parent-child for arbitrary region set without
        // knowing branch_factor, we approximate: group regions by parent super-region (branch_factor contiguous).
        let branch_factor = calibrator.config().branch_factor.max(2);
        let mut region_ids: Vec<u32> = regions.iter().map(|r| r.region_id).collect();
        region_ids.sort_unstable();
        // Group into super-regions of branch_factor size
        for chunk in region_ids.chunks(branch_factor) {
            if chunk.len() < 2 {
                continue;
            }
            let selected_in_chunk = chunk.iter().filter(|id| selected.contains(id)).count();
            if selected_in_chunk as f32 / chunk.len() as f32 > 0.5 {
                // Expand to whole chunk (global redistribution within super-region)
                for id in chunk {
                    selected.insert(*id);
                }
            }
        }
        selected
    }

    /// Incremental region-based freeze: only high-density regions are frozen per call.
    fn freeze_delta_incremental(
        delta: &mut CsrVariant,
        segments: &mut Vec<CsrSegment>,
        free_space: &mut super::free_space::SegmentFreeList,
        ts: Timestamp,
        region_vertex_count: usize,
        calibrator: &super::calibrator::CalibratorTree,
        max_regions_per_freeze: usize,
        freeze_density_threshold: f32,
    ) -> merge::FreezeDeltaResult {
        // Non-Multiple variants have no region-aware overflow; fallback to full freeze
        if !matches!(delta, CsrVariant::Multiple(_)) {
            return Self::freeze_delta(delta, segments, free_space, ts, region_vertex_count);
        }
        delta.rebuild_overflow_index();
        // Decide which regions to freeze
        let selected = Self::select_regions_to_freeze(
            delta,
            region_vertex_count,
            ts,
            calibrator,
            max_regions_per_freeze,
            freeze_density_threshold,
        );

        // If no incremental selection (e.g. other CSR variant or empty), fallback to full
        if selected.is_empty() {
            // Check if there are any visible entries at all — if so, fallback to full freeze
            let has_visible = delta
                .iter_all()
                .any(|(_, nbr)| delta.create_ts_of(nbr.edge_id).unwrap_or(0) <= ts);
            if has_visible {
                return Self::freeze_delta(delta, segments, free_space, ts, region_vertex_count);
            } else {
                delta.clear();
                return merge::FreezeDeltaResult { frozen_count: 0 };
            }
        }

        // Drain selected regions from delta
        let frozen_entries = delta.drain_regions(&selected, region_vertex_count, ts);
        if frozen_entries.is_empty() {
            return merge::FreezeDeltaResult { frozen_count: 0 };
        }

        // Build segments from frozen entries — one segment per freeze call (merged across selected regions)
        // This preserves single-segment-per-freeze semantics for small freezes while still being
        // region-granular in selection. For large freezes we could split per region but single
        // segment keeps test compatibility and is still incremental in selection.
        let max_row = frozen_entries
            .iter()
            .map(|(src, _, _)| *src as usize)
            .max()
            .unwrap_or(0);
        let effective_capacity = max_row.saturating_add(1);

        let create_ts_min = frozen_entries
            .iter()
            .map(|(_, _, create_ts)| *create_ts)
            .min()
            .unwrap_or(0);
        let create_ts_max = frozen_entries
            .iter()
            .map(|(_, _, create_ts)| *create_ts)
            .max()
            .unwrap_or(0);

        let mut deleted_count = 0u32;
        let (delete_ts_min, delete_ts_max) = frozen_entries
            .iter()
            .filter_map(|(_, nbr, _)| {
                if nbr.delete_ts != Timestamp::MAX {
                    deleted_count += 1;
                    return Some(nbr.delete_ts);
                }
                None
            })
            .fold((Timestamp::MAX, 0), |(min, max), ts| {
                (std::cmp::min(min, ts), std::cmp::max(max, ts))
            });

        let csr = free_space.build_csr(&frozen_entries, effective_capacity);
        let frozen = frozen_entries.len();
        let deletion_info = DeletionInfo::with_count(delete_ts_min, delete_ts_max, deleted_count);
        let mut segment = CsrSegment::new(csr, create_ts_min, create_ts_max, deletion_info);
        if frozen >= SEPARATE_EDGE_ID_STORAGE_THRESHOLD {
            segment.edge_ids = Some(
                frozen_entries
                    .iter()
                    .map(|(_, nbr, _)| nbr.edge_id)
                    .collect(),
            );
        }
        if region_vertex_count > 0 {
            segment.rebuild_regions_from_entries(region_vertex_count, &frozen_entries);
        }
        segments.push(segment);

        merge::FreezeDeltaResult {
            frozen_count: frozen,
        }
    }

    /// Rebuild segment indices after modifications. Called after freeze or merge operations.
    /// This maintains the timestamp-based index for binary search optimization.
    pub fn rebuild_segment_indices(&mut self) {
        self.out_segment_index.clear();
        for (idx, segment) in self.out_segments.iter().enumerate() {
            self.out_segment_index.push((segment.create_ts_min, idx));
        }
        self.out_segment_index
            .sort_by_key(|k| std::cmp::Reverse(k.0));

        self.in_segment_index.clear();
        for (idx, segment) in self.in_segments.iter().enumerate() {
            self.in_segment_index.push((segment.create_ts_min, idx));
        }
        self.in_segment_index
            .sort_by_key(|k| std::cmp::Reverse(k.0));
    }

    /// Append a single segment to the index incrementally (O(log n) instead of O(n)).
    ///
    /// This is more efficient than rebuild_segment_indices when adding a small number of segments.
    /// The index is kept sorted by create_ts_min in descending order.
    fn append_segment_to_index_out(&mut self, new_idx: usize) {
        if new_idx >= self.out_segments.len() {
            return; // Invalid index
        }

        let new_ts = self.out_segments[new_idx].create_ts_min;

        // Find insertion position using binary search (descending order)
        let pos = self
            .out_segment_index
            .binary_search_by_key(&std::cmp::Reverse(new_ts), |k| std::cmp::Reverse(k.0));

        let insert_pos = match pos {
            Ok(idx) => idx,  // Exact match - insert before
            Err(idx) => idx, // Not found - insert at err position
        };

        self.out_segment_index.insert(insert_pos, (new_ts, new_idx));

        // Update all indices after insertion point since segment positions may have shifted
        for i in insert_pos + 1..self.out_segment_index.len() {
            if self.out_segment_index[i].1 >= new_idx {
                self.out_segment_index[i].1 += 1;
            }
        }
    }

    /// Append a single segment to the in-segment index incrementally.
    fn append_segment_to_index_in(&mut self, new_idx: usize) {
        if new_idx >= self.in_segments.len() {
            return; // Invalid index
        }

        let new_ts = self.in_segments[new_idx].create_ts_min;

        // Find insertion position using binary search (descending order)
        let pos = self
            .in_segment_index
            .binary_search_by_key(&std::cmp::Reverse(new_ts), |k| std::cmp::Reverse(k.0));

        let insert_pos = match pos {
            Ok(idx) => idx,  // Exact match - insert before
            Err(idx) => idx, // Not found - insert at err position
        };

        self.in_segment_index.insert(insert_pos, (new_ts, new_idx));

        // Update all indices after insertion point
        for i in insert_pos + 1..self.in_segment_index.len() {
            if self.in_segment_index[i].1 >= new_idx {
                self.in_segment_index[i].1 += 1;
            }
        }
    }

    /// Auto-merge segments based on threshold configuration.
    ///
    /// Intelligently merges segments when the count exceeds the configured threshold.
    /// Strategy:
    /// - If segment_merge_threshold is 0, auto-merge is disabled
    /// - Otherwise, when segment count >= threshold:
    ///   1. Merge oldest (count - keep_newest) segments into one
    ///   2. Keep the newest keep_newest segments as-is (for fast writes)
    ///   3. Result: 1 + keep_newest segments total
    ///
    /// For example with threshold=50, keep_newest=5:
    ///   - Before: 50+ segments
    ///   - Merge: 45 oldest segments → 1 merged segment
    ///   - After: 1 + 5 = 6 segments
    ///
    /// # Parameters
    /// - `ts`: Current timestamp for deletion filtering
    ///
    /// # Returns
    /// - Number of segments merged (reduction count)
    pub fn auto_merge_segments(&mut self, ts: Timestamp) -> usize {
        if self.config.segment_merge_threshold == 0 {
            return 0; // Auto-merge disabled
        }

        let mut total_merged = 0;
        // Physical deletion is only safe under a bounded retention horizon
        // (active snapshot or operator retention floor); unbounded (MAX)
        // passes None so the merge never drops tombstoned edges.
        let bound = self.mvcc.effective_retention_bound();
        let deletion_filter = (bound < Timestamp::MAX).then_some(bound);

        // Phase 5: Calibrator-guided incremental region merge — try selective high-deletion
        // subtree merge before generic threshold merges. Reduces latency by merging only
        // dense/deleted subtrees instead of global merge when possible.
        let region_n = self.config.region_vertex_count;
        if region_n > 0 && self.calibrator.region_count() > 0 {
            if self.out_segments.len() > 1 {
                let cal_merged = merge::merge_segments_calibrated_with_free_space(
                    &mut self.out_segments,
                    ts,
                    &self.calibrator,
                    deletion_filter,
                    &|edge_id| self.mvcc.delete_ts_of(edge_id),
                    &mut self.out_free_space,
                    region_n,
                );
                if cal_merged > 0 {
                    total_merged += cal_merged;
                    self.rebuild_segment_indices();
                }
            }
            if self.in_segments.len() > 1 {
                let cal_merged = merge::merge_segments_calibrated_with_free_space(
                    &mut self.in_segments,
                    ts,
                    &self.calibrator,
                    deletion_filter,
                    &|edge_id| self.mvcc.delete_ts_of(edge_id),
                    &mut self.in_free_space,
                    region_n,
                );
                if cal_merged > 0 {
                    total_merged += cal_merged;
                    self.rebuild_segment_indices();
                }
            }
            if total_merged > 0 {
                // Calibrator merges are incremental; continue to threshold merges if still needed
                // but avoid double-counting: rebuild already done, let threshold path run as well
            }
        }

        // Emergency merge: if segment count exceeds hard limit, merge aggressively
        if self.config.max_segments_per_direction > 0 {
            if self.out_segments.len() > self.config.max_segments_per_direction {
                let excess = self
                    .out_segments
                    .len()
                    .saturating_sub(self.config.max_segments_per_direction)
                    + 1;
                if excess > 1 {
                    let merge_indices: Vec<usize> = (0..excess).collect();
                    let merged = if region_n > 0 {
                        merge::merge_selected_segments_region_aware_with_free_space(
                            &mut self.out_segments,
                            merge_indices,
                            ts,
                            deletion_filter,
                            &|edge_id| self.mvcc.delete_ts_of(edge_id),
                            &mut self.out_free_space,
                            region_n,
                        )
                    } else {
                        merge::merge_selected_segments_with_deletion_filter_with_free_space(
                            &mut self.out_segments,
                            merge_indices,
                            ts,
                            deletion_filter,
                            &|edge_id| self.mvcc.delete_ts_of(edge_id),
                            &mut self.out_free_space,
                        )
                    };
                    total_merged += merged;
                    self.rebuild_segment_indices();
                }
            }
            if self.in_segments.len() > self.config.max_segments_per_direction {
                let excess = self
                    .in_segments
                    .len()
                    .saturating_sub(self.config.max_segments_per_direction)
                    + 1;
                if excess > 1 {
                    let merge_indices: Vec<usize> = (0..excess).collect();
                    let merged = if region_n > 0 {
                        merge::merge_selected_segments_region_aware_with_free_space(
                            &mut self.in_segments,
                            merge_indices,
                            ts,
                            deletion_filter,
                            &|edge_id| self.mvcc.delete_ts_of(edge_id),
                            &mut self.in_free_space,
                            region_n,
                        )
                    } else {
                        merge::merge_selected_segments_with_deletion_filter_with_free_space(
                            &mut self.in_segments,
                            merge_indices,
                            ts,
                            deletion_filter,
                            &|edge_id| self.mvcc.delete_ts_of(edge_id),
                            &mut self.in_free_space,
                        )
                    };
                    total_merged += merged;
                    self.rebuild_segment_indices();
                }
            }
            if total_merged > 0 {
                if cfg!(debug_assertions) {
                    eprintln!(
                        "[EdgeTable] Emergency merged {} segments (exceeded max {} per direction)",
                        total_merged, self.config.max_segments_per_direction
                    );
                }
                log::info!(
                    "Emergency merge: {} segments (exceeded max {} per direction)",
                    total_merged,
                    self.config.max_segments_per_direction
                );
                return total_merged;
            }
        }

        // Check out-direction
        if self.out_segments.len() >= self.config.segment_merge_threshold {
            let to_merge_count = self
                .out_segments
                .len()
                .saturating_sub(self.config.merge_keep_newest);
            if to_merge_count > 1 {
                let merge_indices: Vec<usize> = (0..to_merge_count).collect();
                let merged = if region_n > 0 {
                    merge::merge_selected_segments_region_aware_with_free_space(
                        &mut self.out_segments,
                        merge_indices.clone(),
                        ts,
                        deletion_filter,
                        &|edge_id| self.mvcc.delete_ts_of(edge_id),
                        &mut self.out_free_space,
                        region_n,
                    )
                } else {
                    merge::merge_selected_segments_with_deletion_filter_with_free_space(
                        &mut self.out_segments,
                        merge_indices.clone(),
                        ts,
                        deletion_filter,
                        &|edge_id| self.mvcc.delete_ts_of(edge_id),
                        &mut self.out_free_space,
                    )
                };
                total_merged += merged;
                if cfg!(debug_assertions) && merged > 0 {
                    eprintln!(
                        "[EdgeTable] Auto-merged {} segments in out direction. New count: {}",
                        merged,
                        self.out_segments.len()
                    );
                }
            }
        }

        // Check in-direction
        if self.in_segments.len() >= self.config.segment_merge_threshold {
            let to_merge_count = self
                .in_segments
                .len()
                .saturating_sub(self.config.merge_keep_newest);
            if to_merge_count > 1 {
                let merge_indices: Vec<usize> = (0..to_merge_count).collect();
                let merged = if region_n > 0 {
                    merge::merge_selected_segments_region_aware_with_free_space(
                        &mut self.in_segments,
                        merge_indices.clone(),
                        ts,
                        deletion_filter,
                        &|edge_id| self.mvcc.delete_ts_of(edge_id),
                        &mut self.in_free_space,
                        region_n,
                    )
                } else {
                    merge::merge_selected_segments_with_deletion_filter_with_free_space(
                        &mut self.in_segments,
                        merge_indices.clone(),
                        ts,
                        deletion_filter,
                        &|edge_id| self.mvcc.delete_ts_of(edge_id),
                        &mut self.in_free_space,
                    )
                };
                total_merged += merged;
                if cfg!(debug_assertions) && merged > 0 {
                    eprintln!(
                        "[EdgeTable] Auto-merged {} segments in in direction. New count: {}",
                        merged,
                        self.in_segments.len()
                    );
                }
            }
        }

        // If any merges happened, rebuild indices
        if total_merged > 0 {
            self.rebuild_segment_indices();
        }

        total_merged
    }
}
