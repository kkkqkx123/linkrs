//! Vertex ID remapping.
//!
//! When a vertex table is compacted, surviving vertices receive new internal
//! IDs (densified). Edge CSRs index rows by internal IDs and store neighbors
//! as encoded `(internal_id, rank)` keys, so the old-to-new mapping must be
//! propagated here or every edge reference to compacted vertices breaks.
//!
//! Rows and neighbors are rebuilt by reconstructing the CSR from all
//! physically present entries (including tombstoned ones, preserving
//! time-travel visibility). Rebuilding also truncates the row space to the
//! highest edge-bearing row plus one, reclaiming space left behind by deleted
//! vertices (Ladybug `getMaxOffsetWithRels() + 1` semantics).
//!
//! Vertex internal ID spaces are per-label, so an edge table must be given
//! two separate mappings: one for its `src_label` space and one for its
//! `dst_label` space. Out rows and in neighbors live in the src space; in
//! rows and out neighbors live in the dst space.

use super::core::TimeTravelEdgeStore;
use super::segment::CsrSegment;
use crate::edge::csr_trait::MutableCsrTrait;
use crate::edge::{Csr, CsrBase, CsrVariant, EdgeStrategy, Nbr};
use graphdb_core::types::{Timestamp, VertexId};
use graphdb_core::StorageResult;
use std::collections::HashMap;

/// Translate an encoded `(endpoint_internal_id, rank)` neighbor key using the
/// old-to-new internal ID mapping. Unmapped endpoints are returned unchanged.
pub(crate) fn remap_endpoint_key(key: VertexId, mapping: Option<&HashMap<u32, u32>>) -> VertexId {
    let (endpoint, rank) = TimeTravelEdgeStore::decode_edge_endpoint(key);
    match endpoint.as_int64() {
        Some(id) if id >= 0 => match mapping.and_then(|m| m.get(&(id as u32))).copied() {
            Some(new_id) => TimeTravelEdgeStore::edge_endpoint_key(new_id, rank),
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

/// Rebuild a mutable CSR with translated rows/neighbors and a truncated row
/// space (max translated row + 1). Tombstoned entries are re-marked so
/// historical visibility is preserved. An empty CSR is rebuilt with a single
/// row: after a compaction remap the pre-compaction capacity must not linger.
fn remap_variant(
    old: CsrVariant,
    row_mapping: Option<&HashMap<u32, u32>>,
    neighbor_mapping: Option<&HashMap<u32, u32>>,
    strategy: EdgeStrategy,
    overflow_chunk_edges: usize,
) -> StorageResult<CsrVariant> {
    let entries: Vec<(u32, Nbr)> = old
        .iter_all()
        .map(|(src, nbr)| {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            (src_u32, nbr)
        })
        .collect();

    let mut max_row = 0u32;
    let mut new_entries = Vec::with_capacity(entries.len());
    for (src, nbr) in entries {
        let new_src = remapped_row(src, row_mapping);
        let new_neighbor = remap_endpoint_key(nbr.neighbor, neighbor_mapping);
        max_row = max_row.max(new_src);
        new_entries.push((
            new_src,
            Nbr {
                neighbor: new_neighbor,
                ..nbr
            },
        ));
    }

    let capacity = (max_row as usize).saturating_add(1);
    let mut csr = CsrVariant::from_strategy_with_overflow(
        strategy,
        capacity,
        new_entries.len(),
        overflow_chunk_edges,
    )?;
    for (src, nbr) in &new_entries {
        csr.insert_edge(
            *src,
            nbr.neighbor,
            nbr.edge_id,
            nbr.create_ts,
        )?;
        if nbr.delete_ts != Timestamp::MAX {
            let _ = csr.delete_edge(*src, nbr.edge_id, nbr.delete_ts);
        }
    }
    Ok(csr)
}

/// Rebuild a frozen segment's immutable CSR with translated rows/neighbors and
/// a truncated row space. Entry order is preserved so position-based EdgeId
/// recovery (`edge_ids`) stays valid.
fn remap_segment_csr(
    segment: &mut CsrSegment,
    row_mapping: Option<&HashMap<u32, u32>>,
    neighbor_mapping: Option<&HashMap<u32, u32>>,
) -> StorageResult<()> {
    let entries: Vec<(u32, Nbr)> = segment
        .csr
        .read()
        .iter()
        .map(|(src, nbr)| {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            (
                src_u32,
                Nbr::new(nbr.neighbor, nbr.edge_id, nbr.timestamp),
            )
        })
        .collect();

    if entries.is_empty() {
        return Ok(());
    }

    let mut max_row = 0u32;
    let mut new_entries = Vec::with_capacity(entries.len());
    for (src, nbr) in entries {
        let new_src = remapped_row(src, row_mapping);
        let new_neighbor = remap_endpoint_key(nbr.neighbor, neighbor_mapping);
        max_row = max_row.max(new_src);
        new_entries.push((
            new_src,
            Nbr {
                neighbor: new_neighbor,
                ..nbr
            },
        ));
    }

    let capacity = (max_row as usize).saturating_add(1);
    let mut csr = segment.csr.write();
    *csr = Csr::from_nbr_entries(&new_entries, capacity);
    Ok(())
}

impl TimeTravelEdgeStore {
    /// Propagate vertex compaction old-to-new internal ID mappings into this
    /// edge table.
    ///
    /// The table references two vertex label ID spaces:
    /// - `src_mapping` applies to out CSR rows and in CSR neighbor keys
    /// - `dst_mapping` applies to in CSR rows and out CSR neighbor keys
    ///
    /// Frozen segments are remapped in place; derived structures (sparse
    /// vertex index, merged current snapshot, property index) are rebuilt or
    /// invalidated. Row spaces of both mutable CSRs and segments are truncated
    /// to the highest edge-bearing row plus one.
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

        self.out_csr = remap_variant(
            std::mem::replace(&mut self.out_csr, CsrVariant::None { vertex_capacity: 0 }),
            src_mapping,
            dst_mapping,
            self.schema.oe_strategy,
            self.config.overflow_chunk_edges,
        )?;
        self.in_csr = remap_variant(
            std::mem::replace(&mut self.in_csr, CsrVariant::None { vertex_capacity: 0 }),
            dst_mapping,
            src_mapping,
            self.schema.ie_strategy,
            self.config.overflow_chunk_edges,
        )?;

        for segment in &mut self.out_segments {
            remap_segment_csr(segment, src_mapping, dst_mapping)?;
        }
        for segment in &mut self.in_segments {
            remap_segment_csr(segment, dst_mapping, src_mapping)?;
        }

        // Derived structures: sparse index keys and the merged snapshot cache
        // are row-indexed and must follow the remap.
        self.rebuild_sparse_vertex_indices();
        self.current_snapshot_out = None;
        self.current_snapshot_in = None;
        self.snapshot_dirty = true;
        self.update_segment_checksums();

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
            "EdgeTable[{}] remapped vertex IDs (src_mapping={}, dst_mapping={}); out_csr capacity={}, in_csr capacity={}",
            self.label,
            src_mapping.map(|m| m.len()).unwrap_or(0),
            dst_mapping.map(|m| m.len()).unwrap_or(0),
            self.out_csr.vertex_capacity(),
            self.in_csr.vertex_capacity(),
        );

        Ok(())
    }
}

/// Rebuild an immutable CSR with translated rows/neighbors and a truncated
/// row space (max edge-bearing row + 1), mirroring Ladybug's
/// `getMaxOffsetWithRels()+1` semantics.
pub(crate) fn remap_immutable_csr(
    csr: &Csr,
    row_mapping: Option<&HashMap<u32, u32>>,
    neighbor_mapping: Option<&HashMap<u32, u32>>,
) -> StorageResult<Csr> {
    if row_mapping.is_none() && neighbor_mapping.is_none() {
        return Ok(csr.clone());
    }
    if row_mapping.is_some_and(|m| m.is_empty()) && neighbor_mapping.is_some_and(|m| m.is_empty()) {
        return Ok(csr.clone());
    }

    let entries: Vec<_> = csr
        .iter()
        .map(|(src, nbr)| {
            let src_u32 = src.as_int64().unwrap_or(0) as u32;
            let new_src = remapped_row(src_u32, row_mapping);
            let new_neighbor = remap_endpoint_key(nbr.neighbor, neighbor_mapping);
            (
                new_src,
                Nbr::new(new_neighbor, nbr.edge_id, nbr.timestamp),
            )
        })
        .collect();

    if entries.is_empty() {
        return Ok(Csr::new());
    }

    let max_row = entries.iter().map(|(src, _)| *src).max().unwrap_or(0);
    let capacity = (max_row as usize).saturating_add(1);
    Ok(Csr::from_nbr_entries(&entries, capacity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;

    fn make_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
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

        // Rows beyond the highest edge-bearing row are gone.
        assert!(table.get_edge(4, 0, 0, 200).is_none());
        assert_eq!(table.out_edges(4, 200).len(), 0);
        assert_eq!(table.out_csr.vertex_capacity(), 4);
        assert_eq!(table.in_csr.vertex_capacity(), 4);
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
        // Max edge-bearing row is 2 (src 3 -> 2); row space truncated to 3.
        assert_eq!(table.out_csr.vertex_capacity(), 3);
    }

    #[test]
    fn test_remap_segments() {
        let mut table = make_table();
        for (i, (src, dst)) in [(0u32, 1u32), (1, 3), (3, 0)].into_iter().enumerate() {
            table.insert_edge(src, dst, i as i64, &[], 100).unwrap();
        }
        table.freeze_csr_only(150);

        assert_eq!(table.out_segments.len(), 1);
        let cap_before = table.out_segments[0].csr.read().vertex_capacity();
        assert!(
            cap_before >= 4,
            "segment covers row 3: capacity {cap_before}"
        );

        // Vertex 2 removed; live {0, 1, 3} → dense {0, 1, 2}.
        let mapping = mapping_from_removals(&[0, 1, 3]);

        table
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();

        // Segment rows truncated to max remapped row + 1 = 3.
        let cap_after = table.out_segments[0].csr.read().vertex_capacity();
        assert_eq!(cap_after, 3);

        assert!(table.get_edge(0, 1, 0, 200).is_some());
        assert!(table.get_edge(1, 2, 1, 200).is_some());
        assert!(table.get_edge(2, 0, 2, 200).is_some());
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
        assert_eq!(table.out_csr.vertex_capacity(), 2);
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
        assert_eq!(table.out_csr.vertex_capacity(), 1);
        assert_eq!(table.in_csr.vertex_capacity(), 2);
    }

    #[test]
    fn test_remap_empty_mapping_is_noop() {
        let mut table = make_table();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let cap_before = table.out_csr.vertex_capacity();
        table.remap_vertex_ids(Some(&HashMap::new()), None).unwrap();
        assert_eq!(table.out_csr.vertex_capacity(), cap_before);
        assert!(table.get_edge(0, 1, 0, 200).is_some());
    }
}
