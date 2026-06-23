//! Snapshot export and time-travel query support.
//!
//! Enables consistent point-in-time snapshots of the edge table for
//! backup, replication, and time-travel queries.

use super::super::{Csr, Nbr, EdgeSchema, LabelId, VertexId, ImmutableNbr, CsrBase};
use super::segment::CsrSegment;
use crate::core::types::{Timestamp, EdgeId};
use crate::core::StorageResult;
use crate::storage::edge::PropertyTable;
use std::collections::HashMap;

/// Exported read-only snapshot of an edge table at a specific timestamp.
///
/// Suitable for:
/// - Backup and restore operations
/// - Time-travel queries (historical data)
/// - Cross-node replication
/// - Snapshot isolation in transactions
#[derive(Debug, Clone)]
pub struct ExportedEdgeSnapshot {
    /// Timestamp of this snapshot
    pub snapshot_ts: Timestamp,
    /// Edge label identifier
    pub label: LabelId,
    /// Read-only outgoing edges
    pub out_csr: Csr,
    /// Read-only incoming edges
    pub in_csr: Csr,
    /// Edge properties (cloned for independence)
    pub properties: PropertyTable,
    /// Edge schema metadata
    pub schema: EdgeSchema,
}

impl ExportedEdgeSnapshot {
    /// Get outgoing edges from a source vertex (snapshot isolation)
    ///
    /// Returns edges as they existed at snapshot_ts.
    /// No timestamp filtering needed - snapshot is already filtered.
    pub fn get_out_edges(&self, src: u32) -> Vec<Nbr> {
        self.out_csr.edges_of(src)
            .iter()
            .map(|edge| Nbr::new(edge.neighbor, edge.edge_id, edge.prop_offset, edge.timestamp))
            .collect()
    }

    /// Get incoming edges to a destination vertex (snapshot isolation)
    ///
    /// Returns edges as they existed at snapshot_ts.
    pub fn get_in_edges(&self, dst: u32) -> Vec<Nbr> {
        self.in_csr.edges_of(dst)
            .iter()
            .map(|edge| Nbr::new(edge.neighbor, edge.edge_id, edge.prop_offset, edge.timestamp))
            .collect()
    }

    /// Get a specific edge in the snapshot (if it exists)
    pub fn get_edge(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        self.out_csr.get_edge(src, dst)
            .map(|edge| Nbr::new(edge.neighbor, edge.edge_id, edge.prop_offset, edge.timestamp))
    }

    /// Check if an edge exists in this snapshot
    pub fn has_edge(&self, src: u32, dst: VertexId) -> bool {
        self.get_edge(src, dst).is_some()
    }

    /// Get edge count for a vertex
    pub fn degree(&self, src: u32) -> usize {
        self.out_csr.edges_of(src).len()
    }
}

/// Snapshot builder supporting MVCC filtering
pub struct SnapshotBuilder {
    /// Dedup map: (src_vid, edge_id) -> (src_vid, nbr)
    edge_map: HashMap<(u32, EdgeId), (u32, Nbr)>,
}

impl SnapshotBuilder {
    /// Create a new snapshot builder
    pub fn new() -> Self {
        Self {
            edge_map: HashMap::new(),
        }
    }

    /// Add edges from a segment
    pub fn add_segment_edges(
        &mut self,
        segment: &CsrSegment,
        ts: Timestamp,
        tombstones: &HashMap<EdgeId, Timestamp>,
    ) {
        if segment.create_ts_min > ts {
            return;
        }

        if segment.deletion_info.all_deleted_before(ts)
            && segment.deletion_info.all_edges_deleted(segment.csr.edge_count()) {
            return;
        }

        let mut edge_position = 0usize;
        for (src, immutable_nbr) in segment.csr.iter() {
            let edge_id = segment.recover_edge_id(immutable_nbr, edge_position);
            edge_position += 1;

            if immutable_nbr.timestamp > ts {
                continue;
            }

            if let Some(&delete_ts) = tombstones.get(&edge_id) {
                if delete_ts <= ts {
                    continue;
                }
            }

            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let nbr = Nbr::new(
                immutable_nbr.neighbor,
                edge_id,
                immutable_nbr.prop_offset,
                immutable_nbr.timestamp,
            );
            self.edge_map.insert((src_u32, edge_id), (src_u32, nbr));
        }
    }

    /// Add edges from mutable CSR delta
    pub fn add_delta_edges(
        &mut self,
        delta_edges: Vec<(u32, Nbr)>,
        ts: Timestamp,
        tombstones: &HashMap<EdgeId, Timestamp>,
    ) {
        for (src_u32, nbr) in delta_edges {
            if nbr.create_ts > ts {
                continue;
            }

            if let Some(&delete_ts) = tombstones.get(&nbr.edge_id) {
                if delete_ts <= ts {
                    continue;
                }
            }

            self.edge_map.insert((src_u32, nbr.edge_id), (src_u32, nbr));
        }
    }

    /// Build CSR from collected edges
    pub fn build_csr(
        edges: Vec<(u32, Nbr)>,
        vertex_capacity: usize,
    ) -> StorageResult<Csr> {
        Ok(Csr::from_nbr_entries(&edges, vertex_capacity))
    }

    /// Get collected edges as sorted vector
    pub fn edges(&self) -> Vec<(u32, Nbr)> {
        let mut edges: Vec<_> = self.edge_map.values().cloned().collect();
        edges.sort_by_key(|(src, _)| *src);
        edges
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::core::types::Timestamp;
    use crate::core::Value;

    fn create_edge_table_with_props() -> super::super::super::EdgeTable {
        let schema = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        };
        super::super::super::EdgeTable::new(schema).unwrap()
    }

    #[test]
    fn test_export_snapshot_basic() {
        let mut table = create_edge_table_with_props();

        let ts1: Timestamp = 100;
        let ts2: Timestamp = 200;

        table.insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], ts1).unwrap();
        table.insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], ts1).unwrap();

        let snapshot = table.export_snapshot(ts1).unwrap();
        assert_eq!(snapshot.snapshot_ts, ts1);
        assert_eq!(snapshot.label, 0);

        let out_edges = snapshot.out_csr.edges_of(0);
        assert_eq!(out_edges.len(), 2);

        table.insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.5))], ts2).unwrap();

        let snapshot_ts1 = table.export_snapshot(ts1).unwrap();
        assert_eq!(snapshot_ts1.out_csr.edges_of(0).len(), 2);

        let snapshot_ts2 = table.export_snapshot(ts2).unwrap();
        assert_eq!(snapshot_ts2.out_csr.edges_of(0).len(), 3);
    }

    #[test]
    fn test_export_snapshot_time_travel() {
        let mut table = create_edge_table_with_props();

        let ts1: Timestamp = 50;
        let ts2: Timestamp = 100;
        let ts3: Timestamp = 150;

        table.insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.0))], ts1).unwrap();
        table.insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.0))], ts2).unwrap();
        table.insert_edge(1, 4, 0, &[("weight".to_string(), Value::Double(3.0))], ts3).unwrap();

        let snap_before_ts1 = table.export_snapshot(ts1 - 1).unwrap();
        assert_eq!(snap_before_ts1.out_csr.edges_of(1).len(), 0);

        let snap_at_ts1 = table.export_snapshot(ts1).unwrap();
        assert_eq!(snap_at_ts1.out_csr.edges_of(1).len(), 1);

        let snap_at_ts2 = table.export_snapshot(ts2).unwrap();
        assert_eq!(snap_at_ts2.out_csr.edges_of(1).len(), 2);

        let snap_at_ts3 = table.export_snapshot(ts3).unwrap();
        assert_eq!(snap_at_ts3.out_csr.edges_of(1).len(), 3);
    }

    #[test]
    fn test_export_snapshot_frozen_consistency() {
        let mut table = create_edge_table_with_props();

        let ts1: Timestamp = 100;
        let ts2: Timestamp = 200;

        table.insert_edge(5, 10, 0, &[("weight".to_string(), Value::Double(1.0))], ts1).unwrap();
        table.insert_edge(5, 11, 0, &[("weight".to_string(), Value::Double(2.0))], ts1).unwrap();

        table.freeze_csr_only(ts1);

        table.insert_edge(5, 12, 0, &[("weight".to_string(), Value::Double(3.0))], ts2).unwrap();

        let snapshot = table.export_snapshot(ts1).unwrap();
        assert_eq!(snapshot.out_csr.edges_of(5).len(), 2);

        let snapshot_ts2 = table.export_snapshot(ts2).unwrap();
        assert_eq!(snapshot_ts2.out_csr.edges_of(5).len(), 3);
    }

    #[test]
    fn test_snapshot_simple_debug() {
        let mut table = create_edge_table_with_props();

        let ts1: Timestamp = 100;

        table.insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], ts1).unwrap();

        let out_edges_before = table.out_edges(0, ts1);
        assert_eq!(out_edges_before.len(), 1);

        let snapshot = table.export_snapshot(ts1).unwrap();

        assert_eq!(snapshot.out_csr.edge_count(), 1);
        assert_eq!(snapshot.out_csr.edges_of(0).len(), 1);

        let edges = snapshot.get_out_edges(0);
        assert_eq!(edges.len(), 1);
    }
}
