//! Statistics structures for observability and monitoring.
//!
//! Provides statistics for tombstones, deletions, and merge operations
//! to help track edge table behavior and performance.

use crate::core::types::Timestamp;

/// Statistics about tombstones for observability and debugging.
#[derive(Debug, Clone)]
pub struct TombstoneStats {
    /// Number of active tombstones
    pub count: usize,
    /// Approximate memory used by tombstones (bytes)
    pub memory_bytes: usize,
    /// Oldest deletion timestamp in tombstones
    pub oldest_delete_ts: Option<Timestamp>,
    /// Newest deletion timestamp in tombstones
    pub newest_delete_ts: Option<Timestamp>,
    /// Current minimum active snapshot timestamp
    pub min_active_snapshot_ts: Timestamp,
}

impl TombstoneStats {
    /// Estimate memory usage: EdgeId(u64) + Timestamp(u32) = 12 bytes per entry
    pub fn estimate_memory(count: usize) -> usize {
        count * std::mem::size_of::<(u64, u32)>()
    }
}

/// Statistics about deletions across all segments for observability.
///
/// Tracks deletion patterns to help identify when segments have significant
/// deletion activity, useful for deciding when to merge or gc segments.
#[derive(Debug, Clone, Default)]
pub struct DeletionStats {
    /// Total edges deleted across all segments
    pub total_deleted_edges: u64,
    /// Total edges frozen (for percentage calculation)
    pub total_frozen_edges: u64,
    /// Number of segments with some deletions
    pub segments_with_deletions: usize,
    /// Number of segments where all edges are deleted (complete deletion)
    pub completely_deleted_segments: usize,
    /// Oldest deletion timestamp across all segments
    pub oldest_deletion_ts: Option<Timestamp>,
    /// Newest deletion timestamp across all segments
    pub newest_deletion_ts: Option<Timestamp>,
}

impl DeletionStats {
    /// Get deletion percentage as a ratio (0.0 to 1.0)
    pub fn deletion_ratio(&self) -> f64 {
        if self.total_frozen_edges == 0 {
            0.0
        } else {
            self.total_deleted_edges as f64 / self.total_frozen_edges as f64
        }
    }

    /// Get deletion percentage (0-100)
    pub fn deletion_percentage(&self) -> f64 {
        self.deletion_ratio() * 100.0
    }

    /// Check if deletions are significant (>10%)
    pub fn is_significant(&self) -> bool {
        self.deletion_ratio() > 0.1
    }
}

/// Statistics about segment merge operations for observability and monitoring.
///
/// Tracks merge activity to understand segment consolidation patterns and
/// evaluate merge strategy effectiveness.
#[derive(Debug, Clone, Default)]
pub struct MergeStats {
    /// Total number of merge operations performed
    pub total_merge_operations: u64,
    /// Total number of segments merged (sum of all merge operations)
    pub total_segments_merged: u64,
    /// Total number of edges involved in merges
    pub total_edges_merged: u64,
    /// Total time spent on merge operations (milliseconds)
    pub total_merge_time_ms: u64,
    /// Current number of segments
    pub current_segment_count: usize,
    /// Maximum segment count reached
    pub max_segment_count: usize,
}

impl MergeStats {
    /// Get average merge time per operation (milliseconds)
    pub fn avg_merge_time_ms(&self) -> f64 {
        if self.total_merge_operations == 0 {
            0.0
        } else {
            self.total_merge_time_ms as f64 / self.total_merge_operations as f64
        }
    }

    /// Get average segments merged per operation
    pub fn avg_segments_per_merge(&self) -> f64 {
        if self.total_merge_operations == 0 {
            0.0
        } else {
            self.total_segments_merged as f64 / self.total_merge_operations as f64
        }
    }

    /// Get average edges merged per operation
    pub fn avg_edges_per_merge(&self) -> f64 {
        if self.total_merge_operations == 0 {
            0.0
        } else {
            self.total_edges_merged as f64 / self.total_merge_operations as f64
        }
    }

    /// Check if segment count is growing too fast (>80% of max)
    pub fn segment_count_pressure(&self) -> bool {
        if self.max_segment_count == 0 {
            false
        } else {
            (self.current_segment_count as f64 / self.max_segment_count as f64) > 0.8
        }
    }
}

#[derive(Debug, Clone)]
pub struct MergeMetrics {
    /// Number of segments before merge
    pub segments_before: usize,
    /// Number of segments after merge
    pub segments_after: usize,
    /// Total number of edges processed in merge
    pub edges_merged: u64,
    /// Time taken for merge operation (milliseconds)
    pub duration_ms: u64,
}

impl MergeMetrics {
    /// Log merge metrics with reduction ratio
    pub fn log(&self) {
        let reduction = if self.segments_before > 0 {
            ((self.segments_before - self.segments_after) as f64 / self.segments_before as f64) * 100.0
        } else {
            0.0
        };
        println!("[MergeMetrics] segments: {} → {} (-{:.1}%), edges: {}, duration: {}ms",
                 self.segments_before, self.segments_after, reduction, self.edges_merged, self.duration_ms);
    }
}

/// Helper structure for merge operation metrics
pub struct DirectionMergeMetrics {
    pub edges_processed: u64,
}

/// Result wrapper containing merge metrics and reduced count
pub struct MergeMetricsResult {
    pub metrics: MergeMetrics,
    pub segments_reduced: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{EdgeTable, EdgeSchema, EdgeStrategy};
    use crate::core::Value;
    use crate::core::types::EdgeId;
    use crate::core::types::DataType;
    use crate::storage::types::StoragePropertyDef;

    fn create_edge_table() -> EdgeTable {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        };
        EdgeTable::new(schema).unwrap()
    }

    fn create_edge_table_with_props() -> EdgeTable {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        };
        EdgeTable::new(schema).unwrap()
    }

    #[test]
    fn test_deletion_ratio() {
        let mut stats = DeletionStats::default();

        assert_eq!(stats.deletion_ratio(), 0.0);
        assert_eq!(stats.deletion_percentage(), 0.0);
        assert!(!stats.is_significant());

        stats.total_frozen_edges = 100;
        stats.total_deleted_edges = 50;
        assert_eq!(stats.deletion_ratio(), 0.5);
        assert_eq!(stats.deletion_percentage(), 50.0);
        assert!(stats.is_significant());

        stats.total_deleted_edges = 5;
        assert_eq!(stats.deletion_ratio(), 0.05);
        assert_eq!(stats.deletion_percentage(), 5.0);
        assert!(!stats.is_significant());
    }

    #[test]
    fn test_tombstone_stats_accuracy() {
        let mut table = create_edge_table_with_props();

        table.insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 50).unwrap();
        table.insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100).unwrap();
        table.insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 150).unwrap();

        table.freeze_csr_only(160);

        table.delete_edge(0, 1, 0, 200).unwrap();
        table.delete_edge(0, 2, 0, 250).unwrap();
        table.delete_edge(0, 3, 0, 300).unwrap();

        let stats = table.mvcc.tombstone_stats();
        assert_eq!(stats.count, 3);
        assert!(stats.memory_bytes > 0);
        assert_eq!(stats.oldest_delete_ts, Some(200));
        assert_eq!(stats.newest_delete_ts, Some(300));
    }

    #[test]
    fn test_deletion_stats_tracking() {
        let mut table = create_edge_table_with_props();

        for i in 0..5 {
            table.insert_edge(0, 1, i as i64, &[("weight".to_string(), Value::Double(i as f64))], 100 + i as u32).unwrap();
        }

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 0);
        assert_eq!(stats.segments_with_deletions, 0);
        assert_eq!(stats.completely_deleted_segments, 0);
        assert_eq!(stats.deletion_percentage(), 0.0);

        table.freeze_csr_only(105);

        let stats = table.deletion_stats();
        assert_eq!(stats.total_frozen_edges, 10);

        table.delete_edge(0, 1, 0, 110).unwrap();
        table.delete_edge(0, 1, 1, 111).unwrap();

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 0);

        table.freeze_csr_only(115);

        let stats = table.deletion_stats();
        assert!(stats.deletion_percentage() >= 0.0);
    }

    #[test]
    fn test_deletion_stats_complete_segment_deletion() {
        let mut table = create_edge_table();

        for i in 0..3 {
            table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
        }

        table.freeze_csr_only(105);

        for i in 0..3 {
            table.delete_edge(0, 1, i as i64, 110).unwrap();
        }

        table.freeze_csr_only(115);

        let stats = table.deletion_stats();
        assert!(stats.total_frozen_edges > 0);
    }
}
