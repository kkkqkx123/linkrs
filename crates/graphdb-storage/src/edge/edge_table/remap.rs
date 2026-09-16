//! Vertex ID remapping.
//!
//! When a vertex table is compacted, surviving vertices receive new internal
//! IDs (densified). Edge shards index rows by internal IDs and store neighbors
//! as encoded `(internal_id, rank)` keys, so the old-to-new mapping must be
//! propagated here or every edge reference to compacted vertices breaks.
//!
//! Entries are collected per group (including tombstoned ones, preserving
//! snapshot visibility), translated, and routed into fresh shard sets: a
//! translated endpoint may land in a different group than its source.
//! Trailing empty groups are dropped. Rebuilding also drops the
//! pre-compaction group space left behind by deleted vertices.
//!
//! Vertex internal ID spaces are per-label, so an edge table must be given
//! two separate mappings: one for its `src_label` space and one for its
//! `dst_label` space. Out rows and in neighbors live in the src space; in
//! rows and out neighbors live in the dst space.

use super::core::EdgeStore;
use crate::edge::csr_trait::MutableCsrTrait;
use crate::edge::{CsrShardSet, EdgeStrategy, Nbr};
use graphdb_core::types::{Timestamp, VertexId};
use graphdb_core::StorageResult;
use std::collections::HashMap;

/// Translate an encoded `(endpoint_internal_id, rank)` neighbor key using the
/// old-to-new internal ID mapping. Unmapped endpoints are returned unchanged.
pub(crate) fn remap_endpoint_key(key: VertexId, mapping: Option<&HashMap<u32, u32>>) -> VertexId {
    let (endpoint, rank) = EdgeStore::decode_edge_endpoint(key);
    match endpoint.as_int64() {
        Some(id) if id >= 0 => match mapping.and_then(|m| m.get(&(id as u32))).copied() {
            Some(new_id) => EdgeStore::edge_endpoint_key(new_id, rank),
            None => key,
        },
        _ => key,
    }
}

fn remapped_row(id: u32, mapping: Option<&HashMap<u32, u32>>) -> u32 {
    match mapping {
        Some(m) => m.get(&id).copied().unwrap_or(id),
        None => id,
    }
}

/// Rebuild one direction into a fresh shard set with translated rows and
/// neighbors. Tombstoned entries are re-marked so snapshot visibility is
/// preserved. Trailing empty groups are dropped; non-empty strategies keep
/// at least one group.
fn remap_direction(
    old: &CsrShardSet,
    row_mapping: Option<&HashMap<u32, u32>>,
    neighbor_mapping: Option<&HashMap<u32, u32>>,
    strategy: EdgeStrategy,
    group_bits: u32,
    overflow_chunk_edges: usize,
) -> StorageResult<CsrShardSet> {
    let mut rebuilt = CsrShardSet::new(strategy, group_bits, overflow_chunk_edges)?;
    if strategy == EdgeStrategy::None {
        return Ok(rebuilt);
    }
    // Drain per group so a single group never has to hold the whole table;
    // routing decides the target group of each translated entry, which may
    // differ from its source group after densification.
    for gid in 0..old.group_count() {
        let entries: Vec<(u32, Nbr)> = old
            .group_variant(gid)
            .map(|variant| {
                variant
                    .iter_all()
                    .map(|(src, nbr)| {
                        let base = crate::edge::node_group::group_base(gid, old.group_bits());
                        let global = src.as_int64().unwrap_or(0) + base as i64;
                        (global as u32, nbr)
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (src, nbr) in entries {
            let new_src = remapped_row(src, row_mapping);
            let new_neighbor = remap_endpoint_key(nbr.to_vertex_id(), neighbor_mapping);
            let (ep_vid, ep_rank) = new_neighbor.decode_edge_endpoint();
            let new_nbr = Nbr {
                endpoint: ep_vid.as_int64().unwrap_or(0) as u32,
                rank: ep_rank,
                ..nbr
            };
            rebuilt.insert_edge(
                new_src,
                new_nbr.to_vertex_id(),
                new_nbr.edge_id,
                new_nbr.create_ts,
            )?;
            if new_nbr.delete_ts != Timestamp::MAX {
                let _ = rebuilt.delete_edge(new_src, new_nbr.edge_id, new_nbr.delete_ts);
            }
        }
    }
    // Rebuilt rows must reach the next checkpoint: mark every surviving
    // group dirty so incremental flush cannot skip them. Reclaim hints are
    // preserved: tombstoned entries survived the rebuild and the next
    // reclaim pass must inspect their groups once.
    rebuilt.mark_all_dirty();
    rebuilt.truncate_trailing_empty_groups();
    Ok(rebuilt)
}

impl EdgeStore {
    /// Propagate vertex compaction old-to-new internal ID mappings into this
    /// edge table.
    ///
    /// The table references two vertex label ID spaces:
    /// - `src_mapping` applies to out shard rows and in shard neighbor keys
    /// - `dst_mapping` applies to in shard rows and out shard neighbor keys
    ///
    /// Both directions are rebuilt group by group with trailing empty groups
    /// dropped. The property index encodes (src, dst) internal IDs in its keys
    /// and is rebuilt from the remapped data when enabled.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) -> StorageResult<()> {
        if src_mapping.is_none() && dst_mapping.is_none() {
            return Ok(());
        }
        let src_empty = src_mapping.is_none_or(|m| m.is_empty());
        let dst_empty = dst_mapping.is_none_or(|m| m.is_empty());
        if src_empty && dst_empty {
            return Ok(());
        }

        self.out_csr = remap_direction(
            &self.out_csr,
            src_mapping,
            dst_mapping,
            self.schema.oe_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
        )?;
        self.in_csr = remap_direction(
            &self.in_csr,
            dst_mapping,
            src_mapping,
            self.schema.ie_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
        )?;

        // The property index encodes (src, dst) internal IDs in its keys;
        // rebuild it from the remapped data when enabled.
        if self.property_index.is_some() {
            let pool_capacity = self
                .property_index
                .as_ref()
                .map(|idx| idx.pool_capacity())
                .unwrap_or(1024);
            self.build_property_index(pool_capacity)?;
        }

        log::debug!(
            "EdgeTable[{}] remapped vertex IDs (src_mapping={}, dst_mapping={}); out_groups={}, in_groups={}",
            self.label,
            src_mapping.map(|m| m.len()).unwrap_or(0),
            dst_mapping.map(|m| m.len()).unwrap_or(0),
            self.out_csr.group_count(),
            self.in_csr.group_count(),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::Value;

    fn make_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: graphdb_core::types::DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    fn mapping_from_removals(live: &[u32]) -> HashMap<u32, u32> {
        // Simulates a vertex compaction: live ids (sorted) become dense 0..n.
        let mut mapping = HashMap::new();
        for (new_id, &old_id) in live.iter().enumerate() {
            if old_id != new_id as u32 {
                mapping.insert(old_id, new_id as u32);
            }
        }
        mapping
    }

    #[test]
    fn test_remap_rows_and_neighbors() {
        let mut table = make_table();
        // Rows 0..5, gaps at 2 and 4 (deleted vertices).
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(1, 3, 1, &[], 100).unwrap();
        table.insert_edge(3, 5, 2, &[], 100).unwrap();
        table.insert_edge(5, 0, 3, &[], 100).unwrap();

        // Vertex compaction keeps rows {0, 1, 3, 5} → dense {0, 1, 2, 3}.
        let mapping = mapping_from_removals(&[0, 1, 3, 5]);

        table
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .expect("remap should succeed");

        // src 0 -> 0, src 1 -> 1, src 3 -> 2, src 5 -> 3
        assert_eq!(table.out_edges(0, 200).len(), 1);
        assert_eq!(table.out_edges(1, 200).len(), 1);
        assert_eq!(table.out_edges(2, 200).len(), 1);
        assert_eq!(table.out_edges(3, 200).len(), 1);

        // Neighbors remapped: 1 -> 1, 3 -> 2, 5 -> 3, 0 -> 0
        assert!(table.get_edge(0, 1, 0, 200).is_some());
        assert!(table.get_edge(1, 2, 1, 200).is_some());
        assert!(table.get_edge(2, 3, 2, 200).is_some());
        assert!(table.get_edge(3, 0, 3, 200).is_some());

        // Single-group table: one group per direction, trailing groups gone.
        assert_eq!(table.out_csr.group_count(), 1);
        assert_eq!(table.in_csr.group_count(), 1);
        assert!(table.get_edge(4, 0, 0, 200).is_none());
        assert_eq!(table.out_edges(4, 200).len(), 0);
    }

    #[test]
    fn test_remap_preserves_tombstoned_entries() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(3, 4, 1, &[], 100).unwrap();
        // Edge 0->1 is deleted at ts=150; must survive remapping.
        table.delete_edge(0, 1, 0, 150).unwrap();

        // Vertex 2 removed; live {0, 1, 3, 4} → dense {0, 1, 2, 3}.
        let mapping = mapping_from_removals(&[0, 1, 3, 4]);

        table
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();

        // Live at ts before deletion: still visible via remapped rows.
        let before = table.out_edges(0, 149);
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].dst_vid.as_int64(), Some(1));
        // Live at ts after deletion: hidden, but entry preserved.
        assert_eq!(table.out_edges(0, 200).len(), 0);
        // Second edge: src 3 -> 2, dst 4 -> 3 (dense).
        assert!(table.get_edge(2, 3, 1, 200).is_some());
        // Highest edge-bearing row is 2; a single group remains.
        assert_eq!(table.out_csr.group_count(), 1);
    }

    #[test]
    fn test_remap_dst_only_affects_neighbors_out() {
        let mut table = make_table();
        table.insert_edge(1, 4, 0, &[], 100).unwrap();

        // Only dst label compacted: rows stay, out neighbors change.
        let mapping = HashMap::from([(4u32, 0u32)]);

        table.remap_vertex_ids(None, Some(&mapping)).unwrap();

        assert!(table.get_edge(1, 0, 0, 200).is_some());
        assert!(table.get_edge(1, 4, 0, 200).is_none());
        assert_eq!(table.out_csr.group_count(), 1);
    }

    #[test]
    fn test_remap_src_only_affects_rows() {
        let mut table = make_table();
        table.insert_edge(4, 1, 0, &[], 100).unwrap();

        // Only src label compacted: rows change, out neighbors stay.
        let mapping = HashMap::from([(4u32, 0u32)]);

        table.remap_vertex_ids(Some(&mapping), None).unwrap();

        assert!(table.get_edge(0, 1, 0, 200).is_some());
        assert!(table.get_edge(4, 1, 0, 200).is_none());
        assert_eq!(table.out_csr.group_count(), 1);
        assert_eq!(table.in_csr.group_count(), 1);
    }

    #[test]
    fn test_remap_empty_mapping_is_noop() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let groups_before = table.out_csr.group_count();
        table.remap_vertex_ids(Some(&HashMap::new()), None).unwrap();
        assert_eq!(table.out_csr.group_count(), groups_before);
        assert!(table.get_edge(0, 1, 0, 200).is_some());
    }

    #[test]
    fn test_remap_moves_edges_across_groups() {
        let mut table = make_table();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        assert_eq!(table.out_csr.group_count(), 2);

        // Densify 5000 -> 1 and 6000 -> 2: the edge moves into group 0.
        let src_mapping = HashMap::from([(5000u32, 1u32)]);
        let dst_mapping = HashMap::from([(6000u32, 2u32)]);
        table
            .remap_vertex_ids(Some(&src_mapping), Some(&dst_mapping))
            .unwrap();

        assert!(table.get_edge(1, 2, 0, 200).is_some());
        assert!(table.get_edge(5000, 6000, 0, 200).is_none());
        assert_eq!(table.out_csr.group_count(), 1);
    }

    #[test]
    fn test_remap_flush_load_roundtrip() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(5000, 6000, 0, &[], 100).unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");

        let src_mapping = HashMap::from([(5000u32, 1u32)]);
        let dst_mapping = HashMap::from([(6000u32, 2u32)]);
        table
            .remap_vertex_ids(Some(&src_mapping), Some(&dst_mapping))
            .unwrap();
        assert!(!table.out_csr.dirty_group_ids().is_empty());
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("post-remap flush should succeed");

        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.get_edge(0, 1, 0, 200).is_some());
        assert!(loaded.get_edge(1, 2, 0, 200).is_some());
        assert!(loaded.get_edge(5000, 6000, 0, 200).is_none());
        assert_eq!(loaded.edge_count(), 2);
    }
}
