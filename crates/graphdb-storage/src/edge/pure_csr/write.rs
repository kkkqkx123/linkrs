use graphdb_core::types::{EdgeId, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::super::EdgePosition;
use super::PureTopologyCsr;

impl PureTopologyCsr {
    pub(crate) fn scan_overflow_for_edge_id(
        &self,
        src_vid: u32,
        edge_id: EdgeId,
    ) -> Option<(usize, usize)> {
        let chunks = self.overflow_chunks.get(src_vid)?;
        for (chunk_idx, chunk) in chunks.iter().enumerate() {
            for (edge_idx, &eid) in chunk.edge_ids.iter().enumerate() {
                if EdgeId(eid) == edge_id {
                    return Some((chunk_idx, edge_idx));
                }
            }
        }
        None
    }

    /// Insert one edge, reporting the physical slot it landed in.
    ///
    /// Shared by the trait entry below and by the bundled form, so both
    /// shapes run identical dedup and spill decisions with no duplicated
    /// row-management logic.
    pub(crate) fn insert_edge_returning_position(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
    ) -> StorageResult<EdgePosition> {
        let (decoded_endpoint, decoded_rank) = dst.decode_edge_endpoint();
        let endpoint = decoded_endpoint.as_int64().unwrap_or(0) as u32;

        if decoded_rank != 0 {
            return Err(StorageError::conflict(format!(
                "[PureTopologyCsr] rank must be 0, got {}; {}",
                decoded_rank,
                crate::edge::BUNDLED_RANK_REQUIRES_COLUMNAR_MSG
            )));
        }

        let src_idx = src_vid as usize;

        if src_idx >= self.vertex_capacity() {
            self.ensure_vertex_capacity(src_idx + 1);
        }

        if self.rows.primary_capacities[src_idx] == 0 {
            self.allocate_primary_block(src_idx);
        }

        let live = if let Some(set) = self.live_sets.get(src_vid) {
            if set.contains(&endpoint) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            set.len()
        } else {
            let (present, live) = self.row_live_scan(src_vid, endpoint);
            if present {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {:?}",
                    src_vid, dst
                )));
            }
            live
        };

        let degree = self.rows.degrees[src_idx] as usize;
        if degree < self.rows.primary_capacities[src_idx] as usize {
            let base = self.rows.adj_offsets[src_idx] as usize;
            self.endpoints[base + degree] = endpoint;
            self.edge_ids[base + degree] = edge_id.0;
            self.rows.degrees[src_idx] += 1;
            self.mark_primary_unsorted(src_idx);
            let position = EdgePosition::Primary {
                slot: degree as u32,
            };
            self.track_live_insert(src_vid, endpoint, position);
            self.edge_count += 1;
            return Ok(position);
        }

        let effective_chunk_edges = if live > 0 {
            (live * 2).max(self.overflow_chunk_edges)
        } else {
            self.overflow_chunk_edges
        };
        let (chunk_count, added) =
            self.overflow_chunks
                .push_to_row(src_vid, (endpoint, edge_id), effective_chunk_edges);
        if let Some(new_cap) = added {
            self.add_capacity(new_cap);
        }
        let position = self
            .overflow_chunks
            .get(src_vid)
            .map(|chunks| {
                let chunk = chunks.len().saturating_sub(1);
                let slot = chunks
                    .last()
                    .map(|tail| tail.len().saturating_sub(1))
                    .unwrap_or(0);
                EdgePosition::Overflow {
                    chunk: chunk as u32,
                    slot: slot as u32,
                }
            })
            .unwrap_or(EdgePosition::Overflow {
                chunk: chunk_count.saturating_sub(1) as u32,
                slot: 0,
            });
        self.track_live_insert(src_vid, endpoint, position);
        self.edge_count += 1;
        Ok(position)
    }
}
