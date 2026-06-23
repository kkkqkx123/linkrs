//! CSR Freezing Operations
//!
//! Converts mutable delta CSR to immutable segments and maintains segment indices.
//! Freezing is the process of taking visible edges from the mutable CSR and converting
//! them to immutable segments for better cache locality and query performance.

use super::core::EdgeTableCore;
use super::segment::{CsrSegment, DeletionInfo, SEPARATE_EDGE_ID_STORAGE_THRESHOLD};
use super::merge;
use crate::core::types::{EdgeId, Timestamp};
use crate::core::StorageResult;
use crate::storage::edge::{CsrVariant, Csr, Nbr, CsrBase};
use std::collections::HashMap;

impl EdgeTableCore {
    /// Freeze CSR only (convert mutable delta to immutable segment).
    ///
    /// Converts visible edges (ts <= query_ts) to immutable CSR and records
    /// timestamp ranges for time-travel queries and MVCC support.
    /// Clears mutable delta after freezing.
    /// Does NOT perform physical compaction.
    /// Uses incremental index updates for efficiency.
    pub fn freeze_csr_only(&mut self, ts: Timestamp) -> usize {
        // Freeze out direction
        let out_segments_before = self.out_segments.len();
        let out_result = Self::freeze_delta(
            &mut self.out_csr,
            &mut self.out_segments,
            ts,
            &self.mvcc.pending_segment_deletions,
            &self.mvcc.segment_tombstones,
        );
        let out_segments_after = self.out_segments.len();

        // Freeze in direction
        let in_segments_before = self.in_segments.len();
        let in_result = Self::freeze_delta(
            &mut self.in_csr,
            &mut self.in_segments,
            ts,
            &self.mvcc.pending_segment_deletions,
            &self.mvcc.segment_tombstones,
        );
        let in_segments_after = self.in_segments.len();

        self.mvcc.segment_tombstones.extend(self.mvcc.pending_segment_deletions.drain());

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

        // Auto-trigger merge if segment count exceeds threshold (or emergency merge if max exceeded)
        self.auto_merge_segments(ts);

        total_frozen
    }

    /// Freeze delta CSR to immutable segment
    fn freeze_delta(
        delta: &mut CsrVariant,
        segments: &mut Vec<CsrSegment>,
        ts: Timestamp,
        pending_deletions: &HashMap<EdgeId, Timestamp>,
        segment_tombstones: &HashMap<EdgeId, Timestamp>,
    ) -> merge::FreezeDeltaResult {
        let entries: Vec<_> = delta
            .iter(ts)
            .map(|(src, nbr)| {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                (src_u32, nbr)
            })
            .collect();

        if entries.is_empty() {
            delta.clear();
            return merge::FreezeDeltaResult {
                frozen_count: 0,
                edge_ids: Vec::new(),
                csr_position_to_edge_ids_index: Vec::new(),
            };
        }

        let max_vid = entries
            .iter()
            .map(|(src, nbr)| {
                let nbr_id = nbr.neighbor.as_int64().unwrap_or(0) as usize;
                std::cmp::max(*src as usize, nbr_id)
            })
            .max()
            .unwrap_or(0);
        let vertex_capacity = delta.vertex_capacity();
        assert!(
            max_vid < vertex_capacity,
            "Vertex ID {} exceeds capacity {}",
            max_vid,
            vertex_capacity
        );

        let create_ts_min = entries
            .iter()
            .map(|(_, nbr)| nbr.create_ts)
            .min()
            .unwrap_or(0);
        let create_ts_max = entries
            .iter()
            .map(|(_, nbr)| nbr.create_ts)
            .max()
            .unwrap_or(0);

        let mut deleted_count = 0u32;
        let (delete_ts_min, delete_ts_max) = entries
            .iter()
            .filter_map(|(_, nbr)| {
                if let Some(&ts) = pending_deletions.get(&nbr.edge_id) {
                    deleted_count += 1;
                    return Some(ts);
                }
                if let Some(&ts) = segment_tombstones.get(&nbr.edge_id) {
                    deleted_count += 1;
                    return Some(ts);
                }
                None
            })
            .fold((u32::MAX, 0), |(min, max), ts| {
                (std::cmp::min(min, ts), std::cmp::max(max, ts))
            });

        let csr = Csr::from_nbr_entries(&entries, vertex_capacity);
        let frozen = entries.len();

        let deletion_info = DeletionInfo::with_count(delete_ts_min, delete_ts_max, deleted_count);
        let mut segment = CsrSegment::new(
            csr,
            create_ts_min,
            create_ts_max,
            deletion_info,
        );

        if frozen >= SEPARATE_EDGE_ID_STORAGE_THRESHOLD {
            segment.edge_ids = Some(entries.iter().map(|(_, nbr)| nbr.edge_id).collect());
        }

        segments.push(segment);
        delta.clear();

        merge::FreezeDeltaResult {
            frozen_count: frozen,
            edge_ids: Vec::new(),
            csr_position_to_edge_ids_index: Vec::new(),
        }
    }

    /// Rebuild segment indices after modifications. Called after freeze or merge operations.
    /// This maintains the timestamp-based index for binary search optimization.
    pub fn rebuild_segment_indices(&mut self) {
        self.out_segment_index.clear();
        for (idx, segment) in self.out_segments.iter().enumerate() {
            self.out_segment_index.push((segment.create_ts_min, idx));
        }
        self.out_segment_index.sort_by_key(|k| std::cmp::Reverse(k.0));

        self.in_segment_index.clear();
        for (idx, segment) in self.in_segments.iter().enumerate() {
            self.in_segment_index.push((segment.create_ts_min, idx));
        }
        self.in_segment_index.sort_by_key(|k| std::cmp::Reverse(k.0));
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
        let pos = self.out_segment_index.binary_search_by_key(
            &std::cmp::Reverse(new_ts),
            |k| std::cmp::Reverse(k.0),
        );

        let insert_pos = match pos {
            Ok(idx) => idx,      // Exact match - insert before
            Err(idx) => idx,     // Not found - insert at err position
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
        let pos = self.in_segment_index.binary_search_by_key(
            &std::cmp::Reverse(new_ts),
            |k| std::cmp::Reverse(k.0),
        );

        let insert_pos = match pos {
            Ok(idx) => idx,      // Exact match - insert before
            Err(idx) => idx,     // Not found - insert at err position
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
        let min_snapshot_ts = self.mvcc.get_min_active_snapshot_ts();

        // Check out-direction
        if self.out_segments.len() >= self.config.segment_merge_threshold {
            let to_merge_count = self.out_segments.len()
                .saturating_sub(self.config.merge_keep_newest);
            if to_merge_count > 1 {
                let merge_indices: Vec<usize> = (0..to_merge_count).collect();
                let merged = merge::merge_selected_segments_with_deletion_filter(
                    &mut self.out_segments,
                    merge_indices.clone(),
                    ts,
                    Some(min_snapshot_ts),
                );
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
            let to_merge_count = self.in_segments.len()
                .saturating_sub(self.config.merge_keep_newest);
            if to_merge_count > 1 {
                let merge_indices: Vec<usize> = (0..to_merge_count).collect();
                let merged = merge::merge_selected_segments_with_deletion_filter(
                    &mut self.in_segments,
                    merge_indices.clone(),
                    ts,
                    Some(min_snapshot_ts),
                );
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
