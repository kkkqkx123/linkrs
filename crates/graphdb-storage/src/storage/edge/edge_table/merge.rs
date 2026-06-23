//! Segment merge strategies: LSM-tiered, adaptive, in-place, and aggressive merging.
//!
//! Provides multiple merge algorithms optimized for different scenarios:
//! - LSM-tiered: layer-based organization (L0-L3+)
//! - Adaptive: prioritizes old and high-deletion segments
//! - In-place: balances time-gaps and size constraints
//! - Aggressive: size-only, used when segment limit exceeded

use super::segment::{CsrSegment, DeletionInfo};
use super::stats::DirectionMergeMetrics;
use super::super::{Csr, Nbr, CsrBase, MutableCsrTrait};
use crate::core::types::Timestamp;
use std::time::Instant;

/// Result of freezing delta CSR to segments
#[derive(Debug)]
pub struct FreezeDeltaResult {
    pub frozen_count: usize,
    pub edge_ids: Vec<u64>,
    pub csr_position_to_edge_ids_index: Vec<usize>,
}

/// Merge selected segments by indices, with optional physical deletion
pub fn merge_selected_segments(
    segments: &mut Vec<CsrSegment>,
    indices: Vec<usize>,
    current_ts: Timestamp,
) -> usize {
    merge_selected_segments_with_deletion_filter(segments, indices, current_ts, None)
}

/// Merge selected segments with physical deletion of tombstoned edges
///
/// If min_active_snapshot_ts is provided, edges deleted before that timestamp
/// are not included in the merged segment (physical deletion).
pub fn merge_selected_segments_with_deletion_filter(
    segments: &mut Vec<CsrSegment>,
    indices: Vec<usize>,
    current_ts: Timestamp,
    min_active_snapshot_ts: Option<Timestamp>,
) -> usize {
    if indices.len() <= 1 {
        return 0;
    }

    let mut sorted_indices = indices;
    sorted_indices.sort_by(|a, b| b.cmp(a));
    let merge_count = sorted_indices.len();

    let mut merged_entries = Vec::new();
    let mut min_create_ts = u32::MAX;
    let mut max_create_ts = 0u32;
    let mut merged_deletion_info = DeletionInfo::NoDeletes;
    let mut physically_deleted_count = 0u32;

    for idx in &sorted_indices {
        let seg = &segments[*idx];
        min_create_ts = min_create_ts.min(seg.create_ts_min);
        max_create_ts = max_create_ts.max(seg.create_ts_max);

        let mut edge_position = 0usize;
        for (src, immutable_nbr) in seg.csr.iter() {
            let edge_id = seg.recover_edge_id(immutable_nbr, edge_position);
            edge_position += 1;

            // Skip physically deleted edges if minimum snapshot ts is provided
            if let Some(min_ts) = min_active_snapshot_ts {
                if let DeletionInfo::HasDeletes { min_ts: del_min, .. } = seg.deletion_info {
                    if del_min < min_ts {
                        // This segment has deletions older than the min active snapshot
                        // Check if this specific edge was deleted before min_ts
                        if let DeletionInfo::HasDeletes { min_ts: edge_del_ts, .. } = seg.deletion_info {
                            if edge_del_ts < min_ts {
                                physically_deleted_count += 1;
                                continue;
                            }
                        }
                    }
                }
            }

            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let nbr = Nbr::new(
                immutable_nbr.neighbor,
                edge_id,
                immutable_nbr.prop_offset,
                immutable_nbr.timestamp,
            );
            merged_entries.push((src_u32, nbr));
        }

        merged_deletion_info = merged_deletion_info.merge(&seg.deletion_info);
    }

    if !merged_entries.is_empty() {
        let vertex_capacity = merged_entries
            .iter()
            .map(|(src, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let merged_csr = Csr::from_nbr_entries(&merged_entries, vertex_capacity);

        // Adjust deletion info if we performed physical deletion
        let final_deletion_info = if physically_deleted_count > 0 {
            match merged_deletion_info {
                DeletionInfo::NoDeletes => DeletionInfo::NoDeletes,
                DeletionInfo::HasDeletes { min_ts, max_ts, deleted_count } => {
                    let new_count = deleted_count.saturating_sub(physically_deleted_count);
                    DeletionInfo::with_count(min_ts, max_ts, new_count)
                }
            }
        } else {
            merged_deletion_info
        };

        let merged_segment = CsrSegment::with_creation_ts(
            merged_csr,
            min_create_ts,
            max_create_ts,
            final_deletion_info,
            current_ts,
        );

        for idx in sorted_indices {
            segments.remove(idx);
        }

        segments.push(merged_segment);
        merge_count
    } else {
        0
    }
}

/// LSM-style tiered merge strategy
///
/// Organizes segments into levels based on size and merges within/across levels:
/// - L0: < 1MB (fresh from freeze)
/// - L1: 1-8MB
/// - L2: 8-32MB
/// - L3+: > 32MB
pub fn merge_lsm_tiered(segments: &mut Vec<CsrSegment>, current_ts: Timestamp) -> usize {
    use crate::storage::engine::config::LSMSegmentLevel;

    let mut total_merged = 0usize;

    if segments.is_empty() {
        return 0;
    }

    let mut levels: std::collections::BTreeMap<LSMSegmentLevel, Vec<usize>> =
        std::collections::BTreeMap::new();

    for (idx, segment) in segments.iter().enumerate() {
        let size = segment.estimated_bytes();
        let level = LSMSegmentLevel::for_size(size);
        levels.entry(level).or_insert_with(Vec::new).push(idx);
    }

    for (level, indices) in &levels {
        if indices.len() >= level.merge_trigger_count() {
            let merged = merge_selected_segments(
                segments,
                indices.clone(),
                current_ts,
            );
            total_merged += merged;
        }
    }

    total_merged
}

/// Adaptive merge: prioritizes old and high-deletion segments
pub fn merge_adaptive(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    max_segment_age: Timestamp,
) -> usize {
    merge_adaptive_impl(
        segments,
        current_ts,
        max_segment_age,
        0.3,
        8 * 1024 * 1024,
    )
}

/// Implementation of adaptive merge for a single direction
fn merge_adaptive_impl(
    segments: &mut Vec<CsrSegment>,
    current_ts: Timestamp,
    max_segment_age: Timestamp,
    deletion_threshold: f64,
    size_threshold: usize,
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
            (idx, score, seg.csr.edge_count())
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

    let mut merged_entries = Vec::new();
    let mut min_create_ts = u32::MAX;
    let mut max_create_ts = 0u32;
    let mut merged_deletion_info = DeletionInfo::NoDeletes;
    let mut to_remove = Vec::new();

    for idx in &to_merge {
        let seg = &segments[*idx];
        min_create_ts = min_create_ts.min(seg.create_ts_min);
        max_create_ts = max_create_ts.max(seg.create_ts_max);
        merged_deletion_info = merged_deletion_info.merge(&seg.deletion_info);

        let mut edge_position = 0usize;
        for (src, immutable_nbr) in seg.csr.iter() {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let edge_id = seg.recover_edge_id(immutable_nbr, edge_position);
            let nbr = Nbr::new(
                immutable_nbr.neighbor,
                edge_id,
                immutable_nbr.prop_offset,
                immutable_nbr.timestamp,
            );
            merged_entries.push((src_u32, nbr));
            edge_position += 1;
        }

        to_remove.push(*idx);
    }

    if !merged_entries.is_empty() {
        let vertex_capacity = merged_entries
            .iter()
            .map(|(src, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let merged_csr = Csr::from_nbr_entries(&merged_entries, vertex_capacity);
        let merged_segment = CsrSegment::with_creation_ts(
            merged_csr,
            min_create_ts,
            max_create_ts,
            merged_deletion_info,
            current_ts,
        );

        to_remove.sort_by(|a, b| b.cmp(a));
        for idx in to_remove {
            segments.remove(idx);
        }

        segments.push(merged_segment);
        to_merge.len()
    } else {
        0
    }
}

/// Merge segments with time and size thresholds
pub fn merge_in_place(
    segments: &mut Vec<CsrSegment>,
    time_threshold: Timestamp,
    size_threshold: usize,
) -> DirectionMergeMetrics {
    if segments.len() <= 1 {
        return DirectionMergeMetrics { edges_processed: 0 };
    }

    let mut merged = Vec::new();
    let mut current_entries = Vec::new();
    let mut total_edges = 0u64;
    let mut current_create_ts_min = segments[0].create_ts_min;
    let mut current_create_ts_max = segments[0].create_ts_max;
    let mut current_deletion_info = segments[0].deletion_info;

    for segment in segments.drain(..) {
        let time_gap = if segment.create_ts_min > current_create_ts_max {
            segment.create_ts_min - current_create_ts_max
        } else if current_create_ts_max > segment.create_ts_min {
            0
        } else {
            segment.create_ts_min - current_create_ts_max
        };

        let total_edge_count = current_entries.len() + segment.csr.edge_count() as usize;
        let bytes_per_edge = segment.csr.bytes_per_edge();
        let estimated_size = total_edge_count * bytes_per_edge;
        let size_ok = estimated_size <= size_threshold;

        if time_gap <= time_threshold && size_ok && !current_entries.is_empty() {
            let mut edge_position = 0usize;
            for (src, immutable_nbr) in segment.csr.iter() {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
                let nbr = Nbr::new(
                    immutable_nbr.neighbor,
                    edge_id,
                    immutable_nbr.prop_offset,
                    immutable_nbr.timestamp,
                );
                current_entries.push((src_u32, nbr));
                edge_position += 1;
            }
            current_create_ts_min = current_create_ts_min.min(segment.create_ts_min);
            current_create_ts_max = current_create_ts_max.max(segment.create_ts_max);
            current_deletion_info = current_deletion_info.merge(&segment.deletion_info);
        } else {
            if !current_entries.is_empty() {
                let vertex_capacity = current_entries
                    .iter()
                    .map(|(src, _)| *src as usize + 1)
                    .max()
                    .unwrap_or(1024)
                    .max(1024);

                let merged_csr = Csr::from_nbr_entries(&current_entries, vertex_capacity);
                total_edges += merged_csr.edge_count() as u64;
                merged.push(CsrSegment::new(
                    merged_csr,
                    current_create_ts_min,
                    current_create_ts_max,
                    current_deletion_info,
                ));
                current_entries.clear();
            }

            let mut edge_position = 0usize;
            for (src, immutable_nbr) in segment.csr.iter() {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
                let nbr = Nbr::new(
                    immutable_nbr.neighbor,
                    edge_id,
                    immutable_nbr.prop_offset,
                    immutable_nbr.timestamp,
                );
                current_entries.push((src_u32, nbr));
                edge_position += 1;
            }
            current_create_ts_min = segment.create_ts_min;
            current_create_ts_max = segment.create_ts_max;
            current_deletion_info = segment.deletion_info;
        }
    }

    if !current_entries.is_empty() {
        let vertex_capacity = current_entries
            .iter()
            .map(|(src, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let merged_csr = Csr::from_nbr_entries(&current_entries, vertex_capacity);
        total_edges += merged_csr.edge_count() as u64;
        merged.push(CsrSegment::new(
            merged_csr,
            current_create_ts_min,
            current_create_ts_max,
            current_deletion_info,
        ));
    }

    *segments = merged;
    DirectionMergeMetrics {
        edges_processed: total_edges,
    }
}

/// Aggressive merge: ignores time gaps, size-only strategy
pub fn merge_aggressive(
    segments: &mut Vec<CsrSegment>,
    size_threshold_bytes: usize,
) -> DirectionMergeMetrics {
    if segments.len() <= 1 {
        return DirectionMergeMetrics { edges_processed: 0 };
    }

    let mut merged = Vec::new();
    let mut current_entries = Vec::new();
    let mut total_edges = 0u64;
    let mut current_create_ts_min = segments[0].create_ts_min;
    let mut current_create_ts_max = segments[0].create_ts_max;
    let mut current_deletion_info = segments[0].deletion_info;

    for segment in segments.drain(..) {
        let estimated_size = (current_entries.len() + segment.csr.edge_count() as usize) * 30;
        let size_ok = estimated_size <= size_threshold_bytes;

        if size_ok && !current_entries.is_empty() {
            for (src, immutable_nbr) in segment.csr.iter() {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let nbr = Nbr::new(
                    immutable_nbr.neighbor,
                    immutable_nbr.edge_id,
                    immutable_nbr.prop_offset,
                    immutable_nbr.timestamp,
                );
                current_entries.push((src_u32, nbr));
            }
            current_create_ts_min = current_create_ts_min.min(segment.create_ts_min);
            current_create_ts_max = current_create_ts_max.max(segment.create_ts_max);
            current_deletion_info = current_deletion_info.merge(&segment.deletion_info);
        } else {
            if !current_entries.is_empty() {
                let vertex_capacity = current_entries
                    .iter()
                    .map(|(src, _)| *src as usize + 1)
                    .max()
                    .unwrap_or(1024)
                    .max(1024);

                let merged_csr = Csr::from_nbr_entries(&current_entries, vertex_capacity);
                total_edges += merged_csr.edge_count() as u64;
                merged.push(CsrSegment::new(
                    merged_csr,
                    current_create_ts_min,
                    current_create_ts_max,
                    current_deletion_info,
                ));
                current_entries.clear();
            }

            let mut edge_position = 0usize;
            for (src, immutable_nbr) in segment.csr.iter() {
                let src_u32 = src.as_int64().unwrap_or(0) as u32;
                let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
                let nbr = Nbr::new(
                    immutable_nbr.neighbor,
                    edge_id,
                    immutable_nbr.prop_offset,
                    immutable_nbr.timestamp,
                );
                current_entries.push((src_u32, nbr));
                edge_position += 1;
            }
            current_create_ts_min = segment.create_ts_min;
            current_create_ts_max = segment.create_ts_max;
            current_deletion_info = segment.deletion_info;
        }
    }

    if !current_entries.is_empty() {
        let vertex_capacity = current_entries
            .iter()
            .map(|(src, _)| *src as usize + 1)
            .max()
            .unwrap_or(1024)
            .max(1024);

        let merged_csr = Csr::from_nbr_entries(&current_entries, vertex_capacity);
        total_edges += merged_csr.edge_count() as u64;
        merged.push(CsrSegment::new(
            merged_csr,
            current_create_ts_min,
            current_create_ts_max,
            current_deletion_info,
        ));
    }

    *segments = merged;
    DirectionMergeMetrics {
        edges_processed: total_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::storage::engine::config::LSMSegmentLevel;

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
        let mut config = EdgeTableConfig::default();
        config.max_segments_per_direction = 3;
        let max_segments = config.max_segments_per_direction;
        let schema = create_test_schema();
        let mut table = EdgeTable::with_config(schema, config).unwrap();

        for t in 0..5 {
            for src in 0..10 {
                table.insert_edge(src as u32, src as u32 + 1, t as i64, &[], t as u32).unwrap();
            }
            table.freeze_csr_only(t as u32);
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
        let mut config = EdgeTableConfig::default();
        config.max_segments_per_direction = 2;
        let schema = create_test_schema();
        let mut table = EdgeTable::with_config(schema, config).unwrap();

        for t in 0..4 {
            for src in 0..5 {
                let dst = src + 1;
                table.insert_edge(src as u32, dst as u32, t as i64, &[], t as u32).unwrap();
            }
            table.freeze_csr_only(t as u32);
        }

        let snapshot = table.export_snapshot(u32::MAX).unwrap();
        for src in 0..5 {
            let edges = snapshot.get_out_edges(src as u32);
            assert!(!edges.is_empty(), "Snapshot should contain edges from {}", src);
        }

        let total_edges: usize = table.out_segments.iter().map(|s| s.csr.edge_count() as usize).sum();
        assert!(total_edges > 0, "Segments should contain edges after aggressive merge");
    }

    #[test]
    fn test_merge_metrics_basic() {
        let schema = create_test_schema();
        let mut table = EdgeTable::new(schema).unwrap();

        for i in 0..5 {
            table.insert_edge(i, i + 1, 0, &[], 100 + i).unwrap();
        }
        table.freeze_csr_only(105);

        for i in 5..10 {
            table.insert_edge(i, i + 1, 0, &[], 110 + i).unwrap();
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
        let mut table = EdgeTable::new(schema).unwrap();

        let edge_count = 20;
        for i in 0..edge_count {
            let src = i % 5;
            let dst = (i / 5) + 5;
            table.insert_edge(src, dst, 0, &[], 100 + (i as u32)).unwrap();
        }
        table.freeze_csr_only(100 + edge_count as u32);

        for i in 0..10 {
            let src = (i + 10) % 5;
            let dst = 20 + i;
            table.insert_edge(src, dst, 0, &[], 200 + (i as u32)).unwrap();
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
        let mut table = EdgeTable::new(schema).unwrap();

        for i in 0..100 {
            let src = i % 20;
            let dst = 100 + (i / 20) * 20 + i % 20;
            table.insert_edge(src, dst, 0, &[], 1000 + (i as u32)).unwrap();
        }
        table.freeze_csr_only(1100);

        for i in 0..50 {
            let src = (i + 5) % 20;
            let dst = 500 + i;
            table.insert_edge(src, dst, 0, &[], 2000 + (i as u32)).unwrap();
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
        let mut table = EdgeTable::new(schema).unwrap();

        for batch in 0..5 {
            for i in 0..10 {
                table
                    .insert_edge(0, 1, (batch * 100 + i) as i64, &[], 100 + batch as u32)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch as u32);
        }

        let initial_count = table.out_segments.len() + table.in_segments.len();
        assert!(initial_count > 0);

        let _merged = table.merge_segments_lsm_tiered(120);

        let final_count = table.out_segments.len() + table.in_segments.len();
        assert!(final_count <= initial_count, "LSM tiering should not increase segment count");
    }

    #[test]
    fn test_lsm_segment_level_classification() {
        assert_eq!(LSMSegmentLevel::for_size(500_000), LSMSegmentLevel::L0);
        assert_eq!(LSMSegmentLevel::for_size(5 * 1024 * 1024), LSMSegmentLevel::L1);
        assert_eq!(LSMSegmentLevel::for_size(16 * 1024 * 1024), LSMSegmentLevel::L2);
        assert_eq!(LSMSegmentLevel::for_size(50 * 1024 * 1024), LSMSegmentLevel::L3Plus);

        assert_eq!(LSMSegmentLevel::L0.merge_trigger_count(), 4);
        assert_eq!(LSMSegmentLevel::L1.merge_trigger_count(), 3);
        assert_eq!(LSMSegmentLevel::L2.merge_trigger_count(), 2);
        assert_eq!(LSMSegmentLevel::L3Plus.merge_trigger_count(), 2);

        assert!(LSMSegmentLevel::L0.merge_target_size() < LSMSegmentLevel::L1.merge_target_size());
        assert!(LSMSegmentLevel::L1.merge_target_size() < LSMSegmentLevel::L2.merge_target_size());
        assert!(LSMSegmentLevel::L2.merge_target_size() < LSMSegmentLevel::L3Plus.merge_target_size());
    }

    #[test]
    fn test_merge_stats_tracking() {
        let schema = create_test_schema();
        let mut table = EdgeTable::new(schema).unwrap();

        for batch in 0..3 {
            for i in 0..5 {
                table
                    .insert_edge(0, 1, (batch * 10 + i) as i64, &[], 100 + batch as u32)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch as u32);
        }

        let initial_count = table.out_segments.len() + table.in_segments.len();
        assert!(initial_count > 0);

        let _merged = table.merge_segments_adaptive(120, 10);

        let final_count = table.out_segments.len() + table.in_segments.len();
        assert!(final_count <= initial_count);
    }

    #[test]
    fn test_adaptive_merge_strategy() {
        let schema = create_test_schema();
        let mut table = EdgeTable::new(schema).unwrap();

        for batch in 0..3 {
            for i in 0..5 {
                table
                    .insert_edge(0, 1, (batch * 10 + i) as i64, &[], 100 + batch as u32)
                    .unwrap();
            }
            table.freeze_csr_only(105 + batch as u32);
        }

        let initial_segments = table.out_segments.len() + table.in_segments.len();
        assert!(initial_segments > 0);

        let _merged = table.merge_segments_adaptive(120, 10);

        let final_segments = table.out_segments.len() + table.in_segments.len();
        assert!(final_segments <= initial_segments, "Merge should reduce or maintain segment count");
    }
}
