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
//!
//! Observability: row and neighbor mapping misses are counted and debug
//! logged. Unmapped endpoints keep their original value. Misses never decide
//! correctness; they only expose caller mapping construction defects.
//!
//! Edge-id counter discipline: `next_edge_id` only guarantees monotonicity
//! without collision, never crash-to-crash stability. Empty-table reload falls
//! back to max id plus one; tests must assert monotonicity, never exact values.
//!
//! Complexity: linear in physical rows including tombstones. Per-group drain
//! keeps peak memory bounded to one group; large-table compaction belongs in a
//! maintenance window. Property index rebuild reuses the streaming build.

use super::core::EdgeStore;
use crate::edge::csr_trait::MutableCsrTrait;
use crate::edge::{CsrShardSet, EdgeStrategy, Nbr};
use graphdb_core::types::{Timestamp, VertexId};
use graphdb_core::StorageResult;
use std::collections::HashMap;

/// Remap miss counters for one rebuild call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemapStats {
    pub row_misses: usize,
    pub neighbor_misses: usize,
    pub entries: usize,
}

fn remapped_row_counted(id: u32, mapping: Option<&HashMap<u32, u32>>, misses: &mut usize) -> u32 {
    match mapping {
        Some(m) => match m.get(&id).copied() {
            Some(new_id) => new_id,
            None => {
                *misses += 1;
                id
            }
        },
        None => id,
    }
}

fn remap_endpoint_key_counted(
    key: VertexId,
    mapping: Option<&HashMap<u32, u32>>,
    misses: &mut usize,
) -> VertexId {
    let (endpoint, rank) = EdgeStore::decode_edge_endpoint(key);
    match endpoint.as_int64() {
        Some(id) if id >= 0 => match mapping {
            Some(m) => match m.get(&(id as u32)).copied() {
                Some(new_id) => EdgeStore::edge_endpoint_key(new_id, rank),
                None => {
                    *misses += 1;
                    key
                }
            },
            None => key,
        },
        Some(_) | None => key,
    }
}

/// Rebuild one direction into a fresh shard set with translated rows and
/// neighbors. Tombstoned entries are re-marked so snapshot visibility is
/// preserved. Trailing empty groups are dropped; non-empty strategies keep
/// at least one group. Miss counters observe unmapped rows and neighbors.
fn remap_direction(
    old: &CsrShardSet,
    row_mapping: Option<&HashMap<u32, u32>>,
    neighbor_mapping: Option<&HashMap<u32, u32>>,
    strategy: EdgeStrategy,
    group_bits: u32,
    overflow_chunk_edges: usize,
    stats: &mut RemapStats,
) -> StorageResult<CsrShardSet> {
    let mut rebuilt = CsrShardSet::new(strategy, group_bits, overflow_chunk_edges)?;
    if strategy == EdgeStrategy::None {
        return Ok(rebuilt);
    }
    // Drain per group so a single group never has to hold the whole table;
    // routing decides the target group of each translated entry, which may
    // differ from its source group after densification. Sparse holes are
    // never visited.
    for gid in old.existing_group_ids() {
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
            let mut row_miss = 0usize;
            let mut nbr_miss = 0usize;
            let new_src = remapped_row_counted(src, row_mapping, &mut row_miss);
            let new_neighbor =
                remap_endpoint_key_counted(nbr.to_vertex_id(), neighbor_mapping, &mut nbr_miss);
            stats.row_misses += row_miss;
            stats.neighbor_misses += nbr_miss;
            stats.entries += 1;
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
                rebuilt.mark_reclaim_hint_for(new_src);
            }
        }
    }
    // Rebuilt rows must reach the next checkpoint: mark every surviving
    // group dirty so incremental flush cannot skip them. Reclaim hints are
    // preserved: tombstoned entries survived the rebuild and the next
    // reclaim pass must inspect their groups once. The rebuild-time append
    // logs are dropped because the delete dirt above forces a base merge
    // that carries the same states.
    rebuilt.mark_all_dirty();
    rebuilt.clear_all_append_logs();
    rebuilt.truncate_trailing_empty_groups();
    Ok(rebuilt)
}

/// Offline reshard statistics for one width change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReshardStats {
    pub old_bits: u32,
    pub new_bits: u32,
    pub out_groups: usize,
    pub in_groups: usize,
    pub edges: u64,
}

impl EdgeStore {
    /// Offline group-width change: the only adjustment outlet for the locked
    /// address width. Reads the old width group by group and writes the new
    /// width, then switches the manifest on the next checkpoint. There is no
    /// online width-change branch: callers must hold exclusive access and
    /// checkpoint after a successful reshard. Vertex ids are unchanged, so
    /// the property index needs no rebuild. Widths outside `1..=20` are
    /// rejected; the current width is a no-op success.
    pub fn reshard(&mut self, new_group_bits: u32) -> StorageResult<ReshardStats> {
        crate::edge::node_group::validate_group_bits(new_group_bits)?;
        let old_bits = self.config.node_group_bits;
        if new_group_bits == old_bits {
            return Ok(ReshardStats {
                old_bits,
                new_bits: new_group_bits,
                out_groups: self.out_csr.group_count(),
                in_groups: self.in_csr.group_count(),
                edges: self.edge_count(),
            });
        }
        let mut stats = RemapStats::default();
        self.out_csr = remap_direction(
            &self.out_csr,
            None,
            None,
            self.schema.oe_strategy,
            new_group_bits,
            self.config.overflow_chunk_edges,
            &mut stats,
        )?;
        self.in_csr = remap_direction(
            &self.in_csr,
            None,
            None,
            self.schema.ie_strategy,
            new_group_bits,
            self.config.overflow_chunk_edges,
            &mut stats,
        )?;
        self.config.node_group_bits = new_group_bits;
        self.rebuild_owner_map();
        log::debug!(
            "EdgeTable[{}] resharded width {} -> {}; out_groups={}, in_groups={}, edges={}",
            self.label,
            old_bits,
            new_group_bits,
            self.out_csr.group_count(),
            self.in_csr.group_count(),
            self.edge_count(),
        );
        Ok(ReshardStats {
            old_bits,
            new_bits: new_group_bits,
            out_groups: self.out_csr.group_count(),
            in_groups: self.in_csr.group_count(),
            edges: self.edge_count(),
        })
    }

    /// Propagate vertex compaction old-to-new internal ID mappings into this
    /// edge table.
    ///
    /// The table references two vertex label ID spaces:
    /// - `src_mapping` applies to out shard rows and in shard neighbor keys
    /// - `dst_mapping` applies to in shard rows and out shard neighbor keys
    ///
    /// Both directions are rebuilt group by group with trailing empty groups
    /// dropped. The property index encodes (src, dst) internal IDs in its keys
    /// and is rebuilt from the remapped data when enabled. Unmapped endpoints
    /// keep their original value; misses are debug logged only.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) -> StorageResult<()> {
        self.remap_vertex_ids_with_stats(src_mapping, dst_mapping)
            .map(|_| ())
    }

    /// Same as `remap_vertex_ids` but returns miss counters for tests.
    pub fn remap_vertex_ids_with_stats(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) -> StorageResult<RemapStats> {
        if src_mapping.is_none() && dst_mapping.is_none() {
            return Ok(RemapStats::default());
        }
        let src_empty = src_mapping.is_none_or(|m| m.is_empty());
        let dst_empty = dst_mapping.is_none_or(|m| m.is_empty());
        if src_empty && dst_empty {
            return Ok(RemapStats::default());
        }

        let mut stats = RemapStats::default();
        self.out_csr = remap_direction(
            &self.out_csr,
            src_mapping,
            dst_mapping,
            self.schema.oe_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
            &mut stats,
        )?;
        self.in_csr = remap_direction(
            &self.in_csr,
            dst_mapping,
            src_mapping,
            self.schema.ie_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
            &mut stats,
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
        self.rebuild_owner_map();

        log::debug!(
            "EdgeTable[{}] remapped vertex IDs (src_mapping={}, dst_mapping={}); out_groups={}, in_groups={}, row_misses={}, neighbor_misses={}, entries={}",
            self.label,
            src_mapping.map(|m| m.len()).unwrap_or(0),
            dst_mapping.map(|m| m.len()).unwrap_or(0),
            self.out_csr.group_count(),
            self.in_csr.group_count(),
            stats.row_misses,
            stats.neighbor_misses,
            stats.entries,
        );

        Ok(stats)
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

    fn make_single_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "single".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Single,
            ie_strategy: EdgeStrategy::Single,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
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

    #[test]
    fn test_remap_compaction_reports_zero_misses() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(1, 0, 0, &[], 100).unwrap();
        let mapping = HashMap::from([(0u32, 0u32), (1u32, 1u32)]);
        let stats = table
            .remap_vertex_ids_with_stats(Some(&mapping), Some(&mapping))
            .unwrap();
        assert_eq!(stats.row_misses, 0);
        assert_eq!(stats.neighbor_misses, 0);
        assert!(table.get_edge(0, 1, 0, 200).is_some());
        assert!(table.get_edge(1, 0, 0, 200).is_some());
        assert_eq!(table.live_authority_orphans(), 0);
    }

    #[test]
    fn test_remap_partial_mapping_reports_expected_misses() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let mapping = HashMap::from([(99u32, 100u32)]);
        let stats = table
            .remap_vertex_ids_with_stats(Some(&mapping), Some(&mapping))
            .unwrap();
        assert!(stats.row_misses > 0);
        assert!(stats.neighbor_misses > 0);
        assert!(table.get_edge(0, 1, 0, 200).is_some());
    }

    #[test]
    fn test_single_strategy_remap_into_fresh_shards() {
        let mut table = make_single_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let mapping = HashMap::from([(0u32, 0u32), (1u32, 1u32)]);
        let stats = table
            .remap_vertex_ids_with_stats(Some(&mapping), Some(&mapping))
            .unwrap();
        assert_eq!(stats.row_misses, 0);
        assert_eq!(stats.neighbor_misses, 0);
        assert!(table.get_edge(0, 1, 0, 200).is_some());
        assert_eq!(table.live_authority_orphans(), 0);
    }

    #[test]
    fn test_edge_id_counter_monotonic_across_reload() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(0, 2, 0, &[], 110).unwrap();
        table.delete_edge(0, 2, 0, 150).unwrap();
        let before = table.next_edge_id;
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.next_edge_id >= before);
        let mut out_seen = std::collections::HashSet::new();
        for (_, nbr) in loaded.out_csr.iter_all() {
            assert!(out_seen.insert(nbr.edge_id));
        }
        let mut in_seen = std::collections::HashSet::new();
        for (_, nbr) in loaded.in_csr.iter_all() {
            assert!(in_seen.insert(nbr.edge_id));
        }
        assert_eq!(out_seen, in_seen);
    }
}
