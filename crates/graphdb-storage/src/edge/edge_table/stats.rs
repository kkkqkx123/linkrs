//! Statistics structures for observability and monitoring.
//!
//! Provides statistics for tombstones and deletions to help track
//! single-segment edge table behavior.

use graphdb_core::types::Timestamp;

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
}

impl TombstoneStats {
    /// Estimate memory usage: EdgeId(u64) + Timestamp(u32) = 12 bytes per entry
    pub fn estimate_memory(count: usize) -> usize {
        count * std::mem::size_of::<(u64, u32)>()
    }
}

/// Statistics about deletions in the single CSR for observability.
///
/// Tracks deletion patterns to help identify when the table has significant
/// deletion activity, useful for deciding when to compact.
#[derive(Debug, Clone, Default)]
pub struct DeletionStats {
    /// Total edges deleted and still tracked
    pub total_deleted_edges: u64,
    /// Total live edges (for percentage calculation)
    pub total_live_edges: u64,
    /// Oldest deletion timestamp
    pub oldest_deletion_ts: Option<Timestamp>,
    /// Newest deletion timestamp
    pub newest_deletion_ts: Option<Timestamp>,
}

impl DeletionStats {
    /// Get deletion percentage as a ratio (0.0 to 1.0)
    pub fn deletion_ratio(&self) -> f64 {
        if self.total_live_edges == 0 {
            0.0
        } else {
            self.total_deleted_edges as f64 / self.total_live_edges as f64
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

#[cfg(test)]
mod tests {
    use super::super::super::{EdgeSchema, EdgeStrategy};
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::edge_table::core::EdgeStore;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;

    fn create_edge_table() -> EdgeStore {
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
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    fn create_edge_table_with_props() -> EdgeStore {
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
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_deletion_ratio() {
        let mut stats = DeletionStats::default();

        assert_eq!(stats.deletion_ratio(), 0.0);
        assert_eq!(stats.deletion_percentage(), 0.0);
        assert!(!stats.is_significant());

        stats.total_live_edges = 100;
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

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 50)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 150)
            .unwrap();

        table.delete_edge(0, 1, 0, 200).unwrap();
        table.delete_edge(0, 2, 0, 250).unwrap();
        table.delete_edge(0, 3, 0, 300).unwrap();

        let stats = table.mvcc.tombstone_stats();
        // 3 deletions recorded once each in the authoritative table.
        assert_eq!(stats.count, 3);
        assert!(stats.memory_bytes > 0);
        assert_eq!(stats.oldest_delete_ts, Some(200));
        assert_eq!(stats.newest_delete_ts, Some(300));
    }

    #[test]
    fn test_deletion_stats_tracking() {
        let mut table = create_edge_table_with_props();

        for i in 0..5u64 {
            table
                .insert_edge(
                    0,
                    1,
                    i as i64,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100 + i,
                )
                .unwrap();
        }

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 0);
        assert_eq!(stats.deletion_percentage(), 0.0);

        table.delete_edge(0, 1, 0, 110).unwrap();
        table.delete_edge(0, 1, 1, 111).unwrap();

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 2);
        assert!(stats.deletion_percentage() >= 0.0);
    }

    #[test]
    fn test_deletion_stats_complete_table_deletion() {
        let mut table = create_edge_table();

        for i in 0..3 {
            table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
        }

        for i in 0..3 {
            table.delete_edge(0, 1, i as i64, 110).unwrap();
        }

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 3);
        assert!(stats.total_live_edges >= 0);
    }
}
