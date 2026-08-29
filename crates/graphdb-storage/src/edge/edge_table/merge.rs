//! Segment merge strategies: LSM-tiered, adaptive, in-place, and aggressive merging.
//!
//! Provides multiple merge algorithms optimized for different scenarios:
//! - LSM-tiered: layer-based organization (L0-L3+)
//! - Adaptive: prioritizes old and high-deletion segments
//! - In-place: balances time-gaps and size constraints
//! - Aggressive: size-only, used when segment limit exceeded

use super::super::{Csr, CsrBase, EdgeId, ImmutableNbr, Nbr};
use super::free_space::SegmentFreeList;
use super::segment::{CsrSegment, DeletionInfo, SEPARATE_EDGE_ID_STORAGE_THRESHOLD};
use super::stats::DirectionMergeMetrics;
use graphdb_core::types::Timestamp;

/// Result of freezing delta CSR to segments
#[derive(Debug)]
pub struct FreezeDeltaResult {
    pub frozen_count: usize,
}

/// Merge selected segments with physical deletion of tombstoned edges
///
/// If min_active_snapshot_ts is provided, edges whose tombstone delete_ts is
/// at or before that timestamp are not included in the merged segment
/// (physical deletion). `edge_delete_ts` resolves the per-edge deletion
/// timestamp (None when the edge is live); the decision is made per edge so
/// live edges of a partially-deleted segment are never dropped.
///
/// The merged `DeletionInfo` is rebuilt from the deletions that remain
/// observable (delete_ts > min_active_snapshot_ts), instead of subtracting
/// counts from the segment-level info.
pub fn merge_selected_segments_with_deletion_filter_with_free_space(
    segments: &mut Vec<CsrSegment>,
    indices: Vec<usize>,
    current_ts: Timestamp,
    min_active_snapshot_ts: Option<Timestamp>,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    free_space: &mut SegmentFreeList,
) -> usize {
    if indices.len() <= 1 {
        return 0;
    }

    let mut sorted_indices = indices;
    sorted_indices.sort_by(|a, b| b.cmp(a));
    let merge_count = sorted_indices.len();

    let mut min_create_ts = Timestamp::MAX;
    let mut max_create_ts = 0u64;
    let mut merged_deletion_info = DeletionInfo::NoDeletes;
    let mut physically_deleted_count = 0u32;
    let mut remaining_deleted_count = 0u32;
    let mut remaining_del_min = Timestamp::MAX;
    let mut remaining_del_max = 0u64;

    for idx in &sorted_indices {
        let seg = &segments[*idx];
        min_create_ts = min_create_ts.min(seg.create_ts_min);
        max_create_ts = max_create_ts.max(seg.create_ts_max);
        merged_deletion_info = merged_deletion_info.merge(&seg.deletion_info);
    }

    let apply_deletion_filter = min_active_snapshot_ts.is_some();
    let retention_bound = min_active_snapshot_ts.unwrap_or(Timestamp::MAX);

    let mut max_vertex = 0usize;
    for idx in &sorted_indices {
        let csr = segments[*idx].csr.read();
        max_vertex = max_vertex.max(csr.vertex_capacity());
    }
    max_vertex = max_vertex.max(1024);
    let mut counts = vec![0u32; max_vertex];

    for idx in &sorted_indices {
        let seg = &segments[*idx];
        let csr = seg.csr.read();
        let seg_vc = csr.vertex_capacity();

        for vid in 0..seg_vc {
            let edges = csr.edges_of(vid as u32);
            if edges.is_empty() {
                continue;
            }
            let mut valid = 0u32;
            let mut del_min = Timestamp::MAX;
            let mut del_max = 0u64;
            let mut del_count = 0u32;

            for (pos, nbr) in edges.iter().enumerate() {
                let edge_id = seg.recover_edge_id(nbr, pos);
                if apply_deletion_filter {
                    if let Some(delete_ts) = edge_delete_ts(edge_id) {
                        if delete_ts <= retention_bound {
                            physically_deleted_count += 1;
                            continue;
                        }
                        del_count += 1;
                        del_min = del_min.min(delete_ts);
                        del_max = del_max.max(delete_ts);
                    }
                }
                valid += 1;
            }

            counts[vid] += valid;
            remaining_deleted_count += del_count;
            remaining_del_min = remaining_del_min.min(del_min);
            remaining_del_max = remaining_del_max.max(del_max);
        }
    }

    let final_deletion_info = if apply_deletion_filter {
        if physically_deleted_count > 0 {
            log::debug!(
                "Physical deletion removed {} edges older than the min active snapshot",
                physically_deleted_count
            );
        }
        DeletionInfo::with_count(
            remaining_del_min,
            remaining_del_max,
            remaining_deleted_count,
        )
    } else {
        merged_deletion_info
    };

    let mut total_valid = 0u32;
    for i in 0..max_vertex {
        total_valid += counts[i];
    }

    if total_valid == 0 {
        let removed_segments: Vec<_> = sorted_indices
            .into_iter()
            .map(|idx| segments.remove(idx))
            .collect();
        for segment in removed_segments {
            free_space.recycle_csr(segment.into_csr());
        }
        return 0;
    }

    let total_valid = total_valid as usize;

    let mut merged_csr = free_space
        .take_reusable_csr(Csr::required_memory_size(max_vertex, total_valid))
        .unwrap_or_default();

    let mut merged_edge_ids: Vec<EdgeId> = Vec::new();
    let collect_edge_ids = total_valid >= SEPARATE_EDGE_ID_STORAGE_THRESHOLD;
    if collect_edge_ids {
        merged_edge_ids.reserve_exact(total_valid);
    }

    merged_csr.rebuild_with_counts(&counts, |offsets, edges| {
        let mut current_pos = offsets[..max_vertex].to_vec();

        for idx in &sorted_indices {
            let seg = &segments[*idx];
            let csr_r = seg.csr.read();
            let seg_vc = csr_r.vertex_capacity();

            for vid in 0..seg_vc {
                let segment_edges = csr_r.edges_of(vid as u32);
                if segment_edges.is_empty() {
                    continue;
                }
                for (pos, nbr) in segment_edges.iter().enumerate() {
                    let edge_id = seg.recover_edge_id(nbr, pos);
                    if apply_deletion_filter {
                        if let Some(delete_ts) = edge_delete_ts(edge_id) {
                            if delete_ts <= retention_bound {
                                continue;
                            }
                        }
                    }
                    let pos_out = current_pos[vid] as usize;
                    edges[pos_out] = ImmutableNbr::with_timestamp_and_prop(
                        nbr.endpoint,
                        nbr.rank,
                        nbr.edge_id,
                        nbr.timestamp,
                        nbr.prop_offset,
                    );
                    current_pos[vid] += 1;
                    if collect_edge_ids {
                        merged_edge_ids.push(edge_id);
                    }
                }
            }
        }
    });

    let removed_segments: Vec<_> = sorted_indices
        .into_iter()
        .map(|idx| segments.remove(idx))
        .collect();
    for segment in removed_segments {
        free_space.recycle_csr(segment.into_csr());
    }

    let mut merged_segment = CsrSegment::with_creation_ts(
        merged_csr,
        min_create_ts,
        max_create_ts,
        final_deletion_info,
        current_ts,
    );
    if collect_edge_ids {
        merged_segment.edge_ids = Some(merged_edge_ids);
    }
    segments.push(merged_segment);

    merge_count
}

/// Region-aware wrapper: same as `merge_selected_segments_with_deletion_filter_with_free_space`
/// but also rebuilds `CsrSegment.regions` when `region_vertex_count > 0`.
pub fn merge_selected_segments_region_aware_with_free_space(
    segments: &mut Vec<CsrSegment>,
    indices: Vec<usize>,
    current_ts: Timestamp,
    min_active_snapshot_ts: Option<Timestamp>,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    free_space: &mut SegmentFreeList,
    region_vertex_count: usize,
) -> usize {
    let merged = merge_selected_segments_with_deletion_filter_with_free_space(
        segments,
        indices,
        current_ts,
        min_active_snapshot_ts,
        edge_delete_ts,
        free_space,
    );
    if merged > 0 && region_vertex_count > 0 {
        if let Some(seg) = segments.last_mut() {
            seg.rebuild_regions(region_vertex_count, edge_delete_ts);
        }
    }
    merged
}

/// Region-aware in-place physical merge: groups by time/size thresholds and
/// merges each group with deletion filtering, rebuilding regions for the
/// resulting segments.
pub fn merge_in_place_region_aware_with_free_space(
    segments: &mut Vec<CsrSegment>,
    time_threshold: Timestamp,
    size_threshold: usize,
    min_active_snapshot_ts: Timestamp,
    free_space: &mut SegmentFreeList,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    region_vertex_count: usize,
) -> DirectionMergeMetrics {
    let metrics = merge_in_place_physical_with_free_space(
        segments,
        time_threshold,
        size_threshold,
        min_active_snapshot_ts,
        free_space,
        edge_delete_ts,
    );
    if region_vertex_count > 0 {
        // Rebuild regions for segments produced by the physical merge.
        // The physical merge appends new segments at the end; rebuild the
        // tail portion that was just created. We conservative rebuild all
        // segments to keep metadata consistent after groups merging.
        for seg in segments.iter_mut() {
            // Rebuild only if regions already enabled or if segment lacks them.
            if seg.region_vertex_count != region_vertex_count || seg.regions.is_empty() {
                seg.rebuild_regions(region_vertex_count, edge_delete_ts);
            }
        }
    }
    metrics
}

/// Calibrator-guided incremental region merge: select segments with high deletion density
/// per calibrator tree, merging only those to reduce latency vs global merge.
///
/// Implements Phase 5 calibrator tree hierarchical detection: walks the tree bottom-up,
/// merging only high-deletion subtrees. If the global deletion ratio is low, no merge
/// is triggered; if high, only the dense subtrees are merged incrementally.
pub fn merge_segments_calibrated_with_free_space(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    calibrator: &super::calibrator::CalibratorTree,
    min_active_snapshot_ts: Option<Timestamp>,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    free_space: &mut SegmentFreeList,
    region_vertex_count: usize,
) -> usize {
    if segments.len() <= 1 || region_vertex_count == 0 {
        return 0;
    }
    let threshold = calibrator.calibrated_threshold().effective_deletion_ratio();
    // Select segments where at least one region exceeds the calibrated threshold
    // or is flagged by calibrator as needing compaction.
    let mut selected: Vec<usize> = Vec::new();
    for (idx, seg) in segments.iter().enumerate() {
        let mut needs_merge = false;
        if seg.regions.is_empty() {
            // Fallback to segment-level deletion ratio
            if seg.deletion_ratio() >= threshold {
                needs_merge = true;
            }
        } else {
            for meta in &seg.regions {
                let ratio = if meta.edge_count == 0 {
                    0.0
                } else {
                    meta.deleted_count as f64 / meta.edge_count as f64
                };
                if ratio >= threshold || calibrator.should_compact_region(meta.region_id) {
                    needs_merge = true;
                    break;
                }
            }
        }
        // Also consider hot regions
        if !needs_merge {
            for meta in &seg.regions {
                if calibrator.is_hot_region(meta.region_id, 10) && meta.deleted_count > 0 {
                    needs_merge = true;
                    break;
                }
            }
        }
        if needs_merge {
            selected.push(idx);
        }
    }

    if selected.len() <= 1 {
        return 0;
    }

    // Hierarchical expansion: if >50% of segments in a calibrator subtree are selected,
    // expand to all segments in that subtree (global redistribution for dense subtree)
    // For simplicity, if >50% of all segments selected, merge all selected (already).
    // If global stats indicate very high deletion, merge all segments regardless of per-region.
    let global = calibrator.global_stats();
    if global.deletion_ratio() >= threshold * 1.5 {
        // High global pressure — merge all segments with deletion filtering
        selected = (0..segments.len()).collect();
    }

    // Cap incremental merge to avoid latency spike: at most half the segments per call
    let max_merge = (segments.len() / 2).max(2);
    if selected.len() > max_merge {
        // Keep the most deleted segments (sort by deletion ratio descending)
        selected.sort_by(|a, b| {
            let ra = segments[*a].deletion_ratio();
            let rb = segments[*b].deletion_ratio();
            rb.partial_cmp(&ra).unwrap()
        });
        selected.truncate(max_merge);
        selected.sort_unstable();
    }

    if selected.len() <= 1 {
        return 0;
    }

    merge_selected_segments_region_aware_with_free_space(
        segments,
        selected,
        current_ts,
        min_active_snapshot_ts,
        edge_delete_ts,
        free_space,
        region_vertex_count,
    )
}

/// Incremental region-level merge inside segments: for segments with many regions,
/// only merge the high-deletion regions' edges, keeping low-deletion regions' edges
/// in place. This reduces copy amplification for large segments.
///
/// Currently implemented as segment-level selection (above); per-region intra-segment
/// compaction is a future optimization. This wrapper preserves the API for incremental
/// region merge and ensures region metadata is rebuilt after merge.
pub fn merge_regions_incremental_with_free_space(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    calibrator: &super::calibrator::CalibratorTree,
    min_active_snapshot_ts: Option<Timestamp>,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
    free_space: &mut SegmentFreeList,
    region_vertex_count: usize,
) -> usize {
    merge_segments_calibrated_with_free_space(
        segments,
        current_ts,
        calibrator,
        min_active_snapshot_ts,
        edge_delete_ts,
        free_space,
        region_vertex_count,
    )
}

/// LSM-style tiered merge strategy
///
/// Organizes segments into levels based on size and merges within/across levels:
/// - L0: < 1MB (fresh from freeze)
/// - L1: 1-8MB
/// - L2: 8-32MB
/// - L3+: > 32MB
///
/// LSM-style tiered merge that reuses retired CSR allocations.
pub fn merge_lsm_tiered_with_free_space(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    free_space: &mut SegmentFreeList,
) -> usize {
    use crate::engine::config::LSMSegmentLevel;

    let mut total_merged = 0usize;

    if segments.is_empty() {
        return 0;
    }

    let mut levels: std::collections::BTreeMap<LSMSegmentLevel, Vec<usize>> =
        std::collections::BTreeMap::new();

    for (idx, segment) in segments.iter().enumerate() {
        let size = segment.estimated_bytes();
        let level = LSMSegmentLevel::for_size(size);
        levels.entry(level).or_default().push(idx);
    }

    for (level, indices) in &levels {
        if indices.len() >= level.merge_trigger_count() {
            // Debug logging with LSM level information
            let (min_size, max_size) = level.size_range();
            let target_size = level.merge_target_size();
            log::debug!(
                "LSM tier {:?}: size_range: {}-{}MB, merge_target: {}MB, segments: {}",
                level,
                min_size / (1024 * 1024),
                max_size / (1024 * 1024),
                target_size / (1024 * 1024),
                indices.len()
            );

            let merged = merge_selected_segments_with_deletion_filter_with_free_space(
                segments,
                indices.clone(),
                current_ts,
                None,
                &|_| None,
                free_space,
            );
            total_merged += merged;
        }
    }

    total_merged
}

/// Adaptive merge: prioritizes old and high-deletion segments
/// Adaptive merge that reuses retired CSR allocations.
pub fn merge_adaptive_with_free_space(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    max_segment_age: Timestamp,
    deletion_threshold: f64,
    max_segment_size_bytes: usize,
    free_space: &mut SegmentFreeList,
) -> usize {
    if max_segment_size_bytes == 0 {
        return 0;
    }
    merge_adaptive_impl(
        segments,
        current_ts,
        max_segment_age,
        deletion_threshold,
        max_segment_size_bytes,
        free_space,
    )
}

/// Implementation of adaptive merge for a single direction
fn merge_adaptive_impl(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    max_segment_age: Timestamp,
    deletion_threshold: f64,
    size_threshold: usize,
    free_space: &mut SegmentFreeList,
) -> usize {
    if segments.len() <= 1 {
        return 0;
    }

    let mut scored_segments: Vec<_> = segments
        .iter()
        .enumerate()
        .map(|(idx, seg)| {
            let age = seg.age(current_ts);
            let deletion_ratio = seg.deletion_ratio();

            let age_score = if age > max_segment_age {
                100.0
            } else {
                (age as f64 / max_segment_age as f64) * 100.0
            };

            let deletion_score = if deletion_ratio > deletion_threshold {
                (deletion_ratio / 0.5) * 100.0
            } else {
                deletion_ratio * 100.0
            };

            let score = (age_score + deletion_score) / 2.0;
            (idx, score, seg.csr.read().edge_count())
        })
        .collect();

    scored_segments.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut to_merge = Vec::new();
    let mut current_size = 0usize;

    for (idx, _score, edge_count) in scored_segments {
        let estimated_size = (current_size / 30) + (edge_count as usize);

        if !to_merge.is_empty() && estimated_size > size_threshold {
            break;
        }

        to_merge.push(idx);
        current_size += edge_count as usize * 30;
    }

    if to_merge.len() <= 1 {
        return 0;
    }

    to_merge.sort();

    let mut merged_entries: Vec<(u32, Nbr, Timestamp)> = Vec::new();
    let mut min_create_ts = Timestamp::MAX;
    let mut max_create_ts = 0u64;
    let mut merged_deletion_info = DeletionInfo::NoDeletes;
    let mut to_remove = Vec::new();

    for idx in &to_merge {
        let seg = &segments[*idx];
        min_create_ts = min_create_ts.min(seg.create_ts_min);
        max_create_ts = max_create_ts.max(seg.create_ts_max);
        merged_deletion_info = merged_deletion_info.merge(&seg.deletion_info);

        for (edge_position, (src, immutable_nbr)) in seg.csr.read().iter().enumerate() {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let edge_id = seg.recover_edge_id(immutable_nbr, edge_position);
            let nbr = Nbr::with_prop_offset(
                immutable_nbr.endpoint,
                immutable_nbr.rank,
                edge_id,
                immutable_nbr.prop_offset,
            );
            merged_entries.push((src_u32, nbr, immutable_nbr.timestamp));
        }

        to_remove.push(*idx);
    }

    if !merged_entries.is_empty() {
        let vertex_capacity = merged_entries
            .iter()
            .map(|(src, _, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let removed_segments: Vec<_> = to_remove
            .into_iter()
            .rev()
            .map(|idx| segments.remove(idx))
            .collect();
        for segment in removed_segments {
            free_space.recycle_csr(segment.into_csr());
        }

        let merged_csr = free_space.build_csr(&merged_entries, vertex_capacity);
        let merged_segment = CsrSegment::with_creation_ts(
            merged_csr,
            min_create_ts,
            max_create_ts,
            merged_deletion_info,
            current_ts,
        );

        segments.push(merged_segment);
        to_merge.len()
    } else {
        0
    }
}

/// Merge segments with time and size thresholds
/// Merge segments in place while reusing retired CSR allocations.
pub fn merge_in_place_with_free_space(
    segments: &mut Vec<CsrSegment>,
    time_threshold: Timestamp,
    size_threshold: usize,
    free_space: &mut SegmentFreeList,
) -> DirectionMergeMetrics {
    if segments.len() <= 1 {
        return DirectionMergeMetrics { edges_processed: 0 };
    }

    let mut merged = Vec::new();
    let mut current_entries: Vec<(u32, Nbr, Timestamp)> = Vec::new();
    let mut total_edges = 0u64;
    let mut current_create_ts_min = segments[0].create_ts_min;
    let mut current_create_ts_max = segments[0].create_ts_max;
    let mut current_deletion_info = segments[0].deletion_info;

    for segment in segments.drain(..) {
        let time_gap = if segment.create_ts_min > current_create_ts_max {
            segment.create_ts_min - current_create_ts_max
        } else {
            segment.create_ts_min.saturating_sub(current_create_ts_max)
        };

        let (segment_edge_count, bytes_per_edge) = {
            let csr = segment.csr.read();
            (csr.edge_count() as usize, csr.bytes_per_edge())
        };
        let total_edge_count = current_entries.len() + segment_edge_count;
        let estimated_size = total_edge_count * bytes_per_edge;
        let size_ok = estimated_size <= size_threshold;

        if time_gap <= time_threshold && size_ok && !current_entries.is_empty() {
            append_segment_entries(&segment, &mut current_entries);
            current_create_ts_min = current_create_ts_min.min(segment.create_ts_min);
            current_create_ts_max = current_create_ts_max.max(segment.create_ts_max);
            current_deletion_info = current_deletion_info.merge(&segment.deletion_info);
        } else {
            if !current_entries.is_empty() {
                let vertex_capacity = current_entries
                    .iter()
                    .map(|(src, _, _)| *src as usize + 1)
                    .max()
                    .unwrap_or(1024)
                    .max(1024);

                let merged_csr = free_space.build_csr(&current_entries, vertex_capacity);
                total_edges += merged_csr.edge_count();
                merged.push(CsrSegment::new(
                    merged_csr,
                    current_create_ts_min,
                    current_create_ts_max,
                    current_deletion_info,
                ));
                current_entries.clear();
            }

            append_segment_entries(&segment, &mut current_entries);
            current_create_ts_min = segment.create_ts_min;
            current_create_ts_max = segment.create_ts_max;
            current_deletion_info = segment.deletion_info;
        }

        free_space.recycle_csr(segment.into_csr());
    }

    if !current_entries.is_empty() {
        let vertex_capacity = current_entries
            .iter()
            .map(|(src, _, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let merged_csr = free_space.build_csr(&current_entries, vertex_capacity);
        total_edges += merged_csr.edge_count();
        merged.push(CsrSegment::new(
            merged_csr,
            current_create_ts_min,
            current_create_ts_max,
            current_deletion_info,
        ));
    }

    *segments = merged;

    // Log deletion percentages for observability
    for (idx, segment) in segments.iter().enumerate() {
        let del_pct = segment
            .deletion_info
            .deletion_percentage(segment.csr.read().edge_count());
        if del_pct > 0 {
            log::debug!("Merged segment[{}] deletion percentage: {}%", idx, del_pct);
        }
    }

    DirectionMergeMetrics {
        edges_processed: total_edges,
    }
}

/// Merge segments with time and size thresholds, physically dropping edges
/// deleted at or before `min_active_snapshot_ts`.
///
/// Grouping mirrors `merge_in_place_with_free_space`; only groups with more
/// than one segment are merged. Groups are processed from the tail so the
/// indices of the remaining groups stay valid.
pub fn merge_in_place_physical_with_free_space(
    segments: &mut Vec<CsrSegment>,
    time_threshold: Timestamp,
    size_threshold: usize,
    min_active_snapshot_ts: Timestamp,
    free_space: &mut SegmentFreeList,
    edge_delete_ts: &dyn Fn(EdgeId) -> Option<Timestamp>,
) -> DirectionMergeMetrics {
    if segments.len() <= 1 {
        return DirectionMergeMetrics { edges_processed: 0 };
    }

    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group: Vec<usize> = Vec::new();
    let mut current_create_ts_max = segments[0].create_ts_max;
    let mut current_size = 0usize;

    for (idx, segment) in segments.iter().enumerate() {
        let (edge_count, bytes_per_edge) = {
            let csr = segment.csr.read();
            (csr.edge_count() as usize, csr.bytes_per_edge())
        };

        if !current_group.is_empty() {
            let time_gap = segment.create_ts_min.saturating_sub(current_create_ts_max);
            let estimated_size = current_size + edge_count * bytes_per_edge;
            if time_gap > time_threshold || estimated_size > size_threshold {
                groups.push(std::mem::take(&mut current_group));
                current_size = 0;
                current_create_ts_max = segment.create_ts_max;
            } else {
                current_create_ts_max = current_create_ts_max.max(segment.create_ts_max);
            }
        } else {
            current_create_ts_max = segment.create_ts_max;
        }

        current_size += edge_count * bytes_per_edge;
        current_group.push(idx);
    }
    if !current_group.is_empty() {
        groups.push(current_group);
    }

    let mut total_edges = 0u64;
    let mut merged_segments = 0usize;
    let mut merged_group_count = 0usize;
    for group in groups.into_iter().rev() {
        if group.len() <= 1 {
            continue;
        }
        for &idx in &group {
            total_edges += segments[idx].csr.read().edge_count();
        }
        merged_segments += merge_selected_segments_with_deletion_filter_with_free_space(
            segments,
            group,
            Timestamp::MAX,
            Some(min_active_snapshot_ts),
            edge_delete_ts,
            free_space,
        );
        merged_group_count += 1;
    }

    if merged_segments > 0 || merged_group_count > 0 {
        log::debug!(
            "Physical merge: {} groups produced {} new segment(s) (min active snapshot ts={})",
            merged_group_count,
            merged_segments,
            min_active_snapshot_ts
        );
    }

    DirectionMergeMetrics {
        edges_processed: total_edges,
    }
}

fn append_segment_entries(segment: &CsrSegment, entries: &mut Vec<(u32, Nbr, Timestamp)>) {
    for (edge_position, (src, immutable_nbr)) in segment.csr.read().iter().enumerate() {
        let src_u32 = src.as_int64().unwrap_or(0) as u32;
        let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
        let nbr = Nbr::with_prop_offset(
            immutable_nbr.endpoint,
            immutable_nbr.rank,
            edge_id,
            immutable_nbr.prop_offset,
        );
        entries.push((src_u32, nbr, immutable_nbr.timestamp));
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::engine::config::LSMSegmentLevel;

    fn create_test_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        }
    }

    #[test]
    fn test_aggressive_merge_triggered_at_max_segments() {
        let config = EdgeTableConfig {
            max_segments_per_direction: 3,
            ..Default::default()
        };
        let max_segments = config.max_segments_per_direction;
        let schema = create_test_schema();
        let mut table = TimeTravelEdgeStore::with_config(schema, config).unwrap();

        for t in 0..5u64 {
            for src in 0..10 {
                table
                    .insert_edge(src as u32, src as u32 + 1, t as i64, &[], t)
                    .unwrap();
            }
            table.freeze_csr_only(t);
        }

        let total_segments = table.out_segments.len() + table.in_segments.len();
        assert!(
            total_segments <= max_segments * 2,
            "Total segments {} should not exceed max limit {}",
            total_segments,
            max_segments * 2
        );
    }

    #[test]
    fn test_aggressive_merge_preserves_correctness() {
        let config = EdgeTableConfig {
            max_segments_per_direction: 2,
            ..Default::default()
        };
        let schema = create_test_schema();
        let mut table = TimeTravelEdgeStore::with_config(schema, config).unwrap();

        for t in 0..4u64 {
            for src in 0..5 {
                let dst = src + 1;
                table
                    .insert_edge(src as u32, dst as u32, t as i64, &[], t)
                    .unwrap();
            }
            table.freeze_csr_only(t);
        }

        let snapshot = table.export_snapshot(Timestamp::MAX).unwrap();
        for src in 0..5 {
            let edges = snapshot.get_out_edges(src as u32);
            assert!(
                !edges.is_empty(),
                "Snapshot should contain edges from {}",
                src
            );
        }

        let total_edges: usize = table
            .out_segments
            .iter()
            .map(|s| s.csr.read().edge_count() as usize)
            .sum();
        assert!(
            total_edges > 0,
            "Segments should contain edges after aggressive merge"
        );
    }

    #[test]
    fn test_merge_metrics_basic() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for i in 0..5u64 {
            table
                .insert_edge(i as u32, i as u32 + 1, 0, &[], 100 + i)
                .unwrap();
        }
        table.freeze_csr_only(105);

        for i in 5..10u64 {
            table
                .insert_edge(i as u32, i as u32 + 1, 0, &[], 110 + i)
                .unwrap();
        }
        table.freeze_csr_only(120);

        let result = table.merge_segments_with_config(50, 8 * 1024 * 1024);
        let metrics = result.metrics;

        assert!(metrics.segments_before > 0);
        assert!(metrics.segments_after <= metrics.segments_before);
        assert!(metrics.edges_merged > 0);
        assert!(metrics.duration_ms < 1_000_000);
    }

    #[test]
    fn test_merge_metrics_edge_count_accuracy() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        let edge_count = 20u64;
        for i in 0..edge_count {
            let src = (i % 5) as u32;
            let dst = ((i / 5) + 5) as u32;
            table.insert_edge(src, dst, 0, &[], 100 + i).unwrap();
        }
        table.freeze_csr_only(100 + edge_count);

        for i in 0..10u64 {
            let src = ((i + 10) % 5) as u32;
            let dst = (20 + i) as u32;
            table.insert_edge(src, dst, 0, &[], 200 + i).unwrap();
        }
        table.freeze_csr_only(210);

        let result = table.merge_segments_with_config(500, 8 * 1024 * 1024);
        let metrics = result.metrics;

        assert!(
            metrics.edges_merged >= 20,
            "Should have merged at least 20 edges, got {}",
            metrics.edges_merged
        );
    }

    #[test]
    fn test_merge_metrics_performance_tracking() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for i in 0..100u64 {
            let src = (i % 20) as u32;
            let dst = (100 + (i / 20) * 20 + i % 20) as u32;
            table.insert_edge(src, dst, 0, &[], 1000 + i).unwrap();
        }
        table.freeze_csr_only(1100);

        for i in 0..50u64 {
            let src = ((i + 5) % 20) as u32;
            let dst = (500 + i) as u32;
            table.insert_edge(src, dst, 0, &[], 2000 + i).unwrap();
        }
        table.freeze_csr_only(2050);

        let result = table.merge_segments_with_config(100, 8 * 1024 * 1024);
        let metrics = result.metrics;

        assert!(metrics.segments_before > 0);
        assert!(metrics.edges_merged > 0);
        assert!(metrics.duration_ms < 1000);
    }

    #[test]
    fn test_lsm_tiered_merge() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for batch in 0..5u64 {
            for i in 0..10 {
                table
                    .insert_edge(0, 1, (batch * 100 + i) as i64, &[], 100 + batch)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch);
        }

        let initial_count = table.out_segments.len() + table.in_segments.len();
        assert!(initial_count > 0);

        let _merged = table.merge_segments_lsm_tiered(120);

        let final_count = table.out_segments.len() + table.in_segments.len();
        assert!(
            final_count <= initial_count,
            "LSM tiering should not increase segment count"
        );
    }

    #[test]
    fn test_lsm_segment_level_classification() {
        assert_eq!(LSMSegmentLevel::for_size(500_000), LSMSegmentLevel::L0);
        assert_eq!(
            LSMSegmentLevel::for_size(5 * 1024 * 1024),
            LSMSegmentLevel::L1
        );
        assert_eq!(
            LSMSegmentLevel::for_size(16 * 1024 * 1024),
            LSMSegmentLevel::L2
        );
        assert_eq!(
            LSMSegmentLevel::for_size(50 * 1024 * 1024),
            LSMSegmentLevel::L3Plus
        );

        assert_eq!(LSMSegmentLevel::L0.merge_trigger_count(), 4);
        assert_eq!(LSMSegmentLevel::L1.merge_trigger_count(), 3);
        assert_eq!(LSMSegmentLevel::L2.merge_trigger_count(), 2);
        assert_eq!(LSMSegmentLevel::L3Plus.merge_trigger_count(), 2);

        assert!(LSMSegmentLevel::L0.merge_target_size() < LSMSegmentLevel::L1.merge_target_size());
        assert!(LSMSegmentLevel::L1.merge_target_size() < LSMSegmentLevel::L2.merge_target_size());
        assert!(
            LSMSegmentLevel::L2.merge_target_size() < LSMSegmentLevel::L3Plus.merge_target_size()
        );
    }

    #[test]
    fn test_merge_stats_tracking() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for batch in 0..3u64 {
            for i in 0..5 {
                table
                    .insert_edge(0, 1, (batch * 10 + i) as i64, &[], 100 + batch)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch);
        }

        let initial_count = table.out_segments.len() + table.in_segments.len();
        assert!(initial_count > 0);

        let _merged = table.merge_segments_adaptive(120, 10, 0.5, 8 * 1024 * 1024);

        let final_count = table.out_segments.len() + table.in_segments.len();
        assert!(final_count <= initial_count);
    }

    #[test]
    fn test_adaptive_merge_strategy() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for batch in 0..3u64 {
            for i in 0..5 {
                table
                    .insert_edge(0, 1, (batch * 10 + i) as i64, &[], 100 + batch)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch);
        }

        let initial_segments = table.out_segments.len() + table.in_segments.len();
        assert!(initial_segments > 0);

        let _merged = table.merge_segments_adaptive(120, 10, 0.5, 8 * 1024 * 1024);

        let final_segments = table.out_segments.len() + table.in_segments.len();
        assert!(
            final_segments <= initial_segments,
            "Merge should reduce or maintain segment count"
        );
    }

    #[test]
    fn test_physical_deletion_preserves_live_edges() {
        use super::super::segment::DeletionInfo;

        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // One segment with 3 edges: 1 deleted, 2 live.
        for i in 0..3u64 {
            table
                .insert_edge(i as u32, i as u32 + 10, 0, &[], 100 + i)
                .unwrap();
        }
        table.freeze_csr_only(105);

        // A second segment so the merge has something to combine.
        for i in 3..5u64 {
            table
                .insert_edge(i as u32, i as u32 + 10, 0, &[], 200 + i)
                .unwrap();
        }
        table.freeze_csr_only(210);

        // Delete one edge of the first segment; the snapshot is registered
        // AFTER the deletion so the edge is removable.
        assert!(table.delete_edge(0, 10, 0, 300).unwrap());
        table.mvcc.register_active_snapshot(400);

        let result = table.merge_segments_with_config_and_deletion_filter(
            10_000,
            8 * 1024 * 1024,
            Some(400),
        );
        assert!(
            result.segments_reduced > 0,
            "expected segments to merge, got reduced={}",
            result.segments_reduced
        );

        // Live edges survive the merge.
        assert!(table.has_edge(1, 11, 0, 400));
        assert!(table.has_edge(2, 12, 0, 400));
        assert!(table.has_edge(3, 13, 0, 400));
        assert!(table.has_edge(4, 14, 0, 400));

        // The deleted edge was physically removed, and no deletions remain.
        assert!(!table.has_edge(0, 10, 0, 400));
        let out_total: u64 = table
            .out_segments
            .iter()
            .map(|s| s.csr.read().edge_count())
            .sum();
        assert_eq!(out_total, 4);
        assert!(
            matches!(table.out_segments[0].deletion_info, DeletionInfo::NoDeletes),
            "merged segment must report no remaining deletions"
        );
    }

    #[test]
    fn test_physical_deletion_with_active_snapshot() {
        use super::super::segment::DeletionInfo;

        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        for i in 0..3u64 {
            table
                .insert_edge(i as u32, i as u32 + 10, 0, &[], 100 + i)
                .unwrap();
        }
        table.freeze_csr_only(105);
        for i in 3..5u64 {
            table
                .insert_edge(i as u32, i as u32 + 10, 0, &[], 200 + i)
                .unwrap();
        }
        table.freeze_csr_only(210);

        // Snapshot registered BEFORE the deletion: delete_ts > min active
        // snapshot ts, so the deleted edge must be preserved.
        table.mvcc.register_active_snapshot(150);
        assert!(table.delete_edge(0, 10, 0, 300).unwrap());

        let result = table.merge_segments_with_config_and_deletion_filter(
            10_000,
            8 * 1024 * 1024,
            Some(150),
        );
        assert!(
            result.segments_reduced > 0,
            "expected segments to merge, got reduced={}",
            result.segments_reduced
        );

        // The deleted edge is kept; its deletion is still tracked.
        let out_total: u64 = table
            .out_segments
            .iter()
            .map(|s| s.csr.read().edge_count())
            .sum();
        assert_eq!(out_total, 5);
        assert!(
            matches!(
                table.out_segments[0].deletion_info,
                DeletionInfo::HasDeletes {
                    min_ts: 300,
                    max_ts: 300,
                    deleted_count: 1
                }
            ),
            "merged segment must keep the deletion info: {:?}",
            table.out_segments[0].deletion_info
        );

        // Time travel still works across the merge.
        assert!(table.has_edge(0, 10, 0, 200));
        assert!(!table.has_edge(0, 10, 0, 400));
    }

    #[test]
    fn test_merge_respects_thresholds() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // 4 batches separated by large time gaps: no group passes the time
        // threshold, so the deletion-filter merge must not collapse all
        // segments into one.
        for batch in 0..4u64 {
            for i in 0..2u64 {
                let src = batch * 10 + i;
                table
                    .insert_edge(src as u32, src as u32 + 1, 0, &[], 100 + batch * 1000 + i)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch * 1000);
        }
        let segments_before = table.out_segments.len() + table.in_segments.len();
        assert_eq!(segments_before, 8);

        table.mvcc.register_active_snapshot(5000);
        let result = table.merge_segments_with_config_and_deletion_filter(
            10,              // tiny time threshold
            8 * 1024 * 1024, // large size threshold
            Some(5000),
        );

        // Gaps between batches are ~1000 >> 10: nothing may be merged.
        assert_eq!(result.segments_reduced, 0);
        assert_eq!(table.out_segments.len() + table.in_segments.len(), 8);
    }

    #[test]
    fn test_physical_merge_reclaims_fully_dead_segments() {
        let schema = create_test_schema();
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        // Two frozen segments, then delete every edge they hold.
        for batch in 0..2u64 {
            for i in 0..3u64 {
                let src = batch * 10 + i;
                table
                    .insert_edge(src as u32, src as u32 + 1, 0, &[], 100 + batch * 100 + i)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch * 100);
        }
        assert!(table.out_segments.len() >= 2);

        for batch in 0..2u64 {
            for i in 0..3u64 {
                let src = batch * 10 + i;
                assert!(table
                    .delete_edge(src as u32, src as u32 + 1, 0, 500)
                    .unwrap());
            }
        }
        // Snapshot registered after the deletions: every tombstone predates
        // the retention bound, so the merge may drop everything.
        table.mvcc.register_active_snapshot(600);

        let result = table.merge_segments_with_config_and_deletion_filter(
            10_000,
            8 * 1024 * 1024,
            Some(600),
        );

        // A fully-dead group produces no merged segment but must not leak:
        // the dead source segments are removed and recycled.
        assert!(
            table.out_segments.is_empty(),
            "fully-dead segments must be reclaimed, {} remain",
            table.out_segments.len()
        );
        assert!(result.segments_reduced > 0);
    }
}
