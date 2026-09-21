use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::super::{EdgePosition, MutableCsrTrait};
use super::{BundledCsr, BundledOverflowValues, INVALID_EDGE_ID};

impl BundledCsr {
    /// Grow the primary value columns to cover the topology primary block.
    pub(crate) fn sync_primary_len(&mut self) {
        let want = self.topology.endpoints.len();
        if self.primary_values.len() < want {
            self.primary_values.resize(want, 0);
        }
        if self.primary_valid.len() < want {
            self.primary_valid.resize(want, false);
        }
    }

    /// Grow the overflow value row to cover the topology overflow row.
    pub(crate) fn sync_overflow_row(&mut self, vid: u32) {
        let topo_lens: Vec<usize> = self
            .topology
            .overflow_chunks
            .get(vid)
            .map(|chunks| chunks.iter().map(|c| c.len()).collect())
            .unwrap_or_default();
        if topo_lens.is_empty() {
            if self.overflow_values.get(vid).is_some() {
                self.overflow_values.take(vid);
            }
            return;
        }
        self.overflow_values
            .ensure_capacity(self.topology.vertex_capacity());
        let slot = self.overflow_values.slot_mut(vid);
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
        let chunks = slot.as_mut().expect("overflow value row just created");
        while chunks.len() < topo_lens.len() {
            let want = topo_lens[chunks.len()];
            let mut fresh = BundledOverflowValues::with_capacity(want.max(1));
            fresh.values.resize(want, 0);
            fresh.resize_valid(want, false);
            chunks.push(fresh);
        }
        for (chunk, want) in chunks.iter_mut().zip(topo_lens.iter()) {
            if chunk.len() < *want {
                chunk.values.resize(*want, 0);
                chunk.resize_valid(*want, false);
            }
        }
    }

    pub(crate) fn primary_index(&self, src_vid: u32, slot: u32) -> Option<usize> {
        let src_idx = src_vid as usize;
        if src_idx >= self.topology.vertex_capacity() {
            return None;
        }
        if slot as usize >= self.topology.rows.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.topology.rows.adj_offsets[src_idx] as usize + slot as usize;
        if idx >= self.topology.edge_ids.len() || idx >= self.primary_values.len() {
            return None;
        }
        Some(idx)
    }

    /// Overwrite the value of one physical slot holding a live edge.
    pub fn set_value_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        value: Option<u64>,
    ) -> bool {
        match position {
            EdgePosition::Primary { slot } => {
                let Some(idx) = self.primary_index(src_vid, slot) else {
                    return false;
                };
                if self.topology.edge_ids[idx] == INVALID_EDGE_ID.0 {
                    return false;
                }
                let (raw, valid) = match value {
                    Some(v) => (v, true),
                    None => (0, false),
                };
                self.primary_values[idx] = raw;
                self.primary_valid.set(idx, valid);
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let topo_live = self
                    .topology
                    .overflow_chunks
                    .get(src_vid)
                    .and_then(|chunks| chunks.get(chunk as usize))
                    .and_then(|c| c.edge_ids.get(slot as usize))
                    .is_some_and(|eid| *eid != INVALID_EDGE_ID.0);
                if !topo_live {
                    return false;
                }
                let Some(chunks) = self.overflow_values.get_mut(src_vid) else {
                    return false;
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return false;
                };
                if slot as usize >= c.len() {
                    return false;
                }
                let (raw, valid) = match value {
                    Some(v) => (v, true),
                    None => (0, false),
                };
                c.values[slot as usize] = raw;
                c.set_valid(slot as usize, valid);
                true
            }
        }
    }

    /// Insert one edge carrying its inline value (`None` stores NULL).
    pub fn insert_edge_with_value(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> StorageResult<()> {
        let position = self
            .topology
            .insert_edge_returning_position(src_vid, dst, edge_id)?;
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        let ok = self.set_value_at_position(src_vid, position, value);
        if !ok {
            return Err(StorageError::data_corruption(format!(
                "bundled value slot missing after insert of edge {:?}",
                edge_id
            )));
        }
        Ok(())
    }

    /// Overwrite the value of one edge by id within its source row.
    pub fn set_value_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> bool {
        let Some((position, _)) = self.topology.locate_edge(src_vid, edge_id) else {
            return false;
        };
        self.set_value_at_position(src_vid, position, value)
    }

    /// Overwrite the value of the live edge for one endpoint.
    pub fn set_value_by_endpoint(
        &mut self,
        src_vid: u32,
        endpoint: u32,
        value: Option<u64>,
    ) -> bool {
        let mut target = None;
        self.topology
            .visit_physical_with_position(src_vid, |position, nbr| {
                if nbr.endpoint == endpoint && nbr.edge_id != INVALID_EDGE_ID {
                    target = Some(position);
                    false
                } else {
                    true
                }
            });
        match target {
            Some(position) => self.set_value_at_position(src_vid, position, value),
            None => false,
        }
    }

    /// Revert a deletion, restoring the caller's value alongside the topology.
    pub fn revert_delete_at_position_with_value(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> bool {
        if !self
            .topology
            .revert_delete_at_position(src_vid, position, expected, ts)
        {
            return false;
        }
        self.sync_primary_len();
        self.sync_overflow_row(src_vid);
        self.set_value_at_position(src_vid, position, value)
    }

    /// Clear the validity bit of one physical slot, keeping the stale word.
    pub(crate) fn clear_value_at_position(&mut self, src_vid: u32, position: EdgePosition) {
        match position {
            EdgePosition::Primary { slot } => {
                if let Some(idx) = self.primary_index(src_vid, slot) {
                    if idx < self.primary_valid.len() {
                        self.primary_valid.set(idx, false);
                    }
                }
            }
            EdgePosition::Overflow { chunk, slot } => {
                if let Some(chunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(c) = chunks.get_mut(chunk as usize) {
                        if (slot as usize) < c.len() {
                            c.set_valid(slot as usize, false);
                        }
                    }
                }
            }
        }
    }

    /// Erase one just-inserted edge for insert rollback, shifting the value column with the topology.
    pub(crate) fn rollback_insert_shifted(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        if edge_id == INVALID_EDGE_ID {
            return false;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.topology.vertex_capacity() {
            return false;
        }
        let found = {
            let (start, end) = self.topology.primary_window(src_idx);
            self.topology.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some(start + i)
                    } else {
                        None
                    }
                })
        };
        if let Some(idx) = found {
            let degree = self.topology.rows.degrees[src_idx] as usize;
            let base = self.topology.rows.adj_offsets[src_idx] as usize;
            self.topology
                .endpoints
                .copy_within(idx + 1..base + degree, idx);
            self.topology
                .edge_ids
                .copy_within(idx + 1..base + degree, idx);
            self.sync_primary_len();
            self.primary_values.copy_within(idx + 1..base + degree, idx);
            for i in idx..base + degree - 1 {
                let next = self.primary_valid.get(i + 1).map(|b| *b).unwrap_or(false);
                self.primary_valid.set(i, next);
            }
            self.topology.rows.degrees[src_idx] -= 1;
            self.topology.sub_capacity(1);
            self.topology.edge_count -= 1;
            self.topology.rebuild_live_set_for_vertex(src_vid);
            return true;
        }
        let located = self.topology.scan_overflow_for_edge_id(src_vid, edge_id);
        if let Some((chunk_idx, edge_idx)) = located {
            if let Some(chunks) = self.topology.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                let emptied = chunks[chunk_idx].is_empty();
                let mut freed = None;
                if emptied {
                    let removed = chunks.remove(chunk_idx);
                    freed = Some((removed.capacity(), chunks.is_empty()));
                }
                if let Some(vchunks) = self.overflow_values.get_mut(src_vid) {
                    if let Some(vc) = vchunks.get_mut(chunk_idx) {
                        if edge_idx < vc.len() {
                            vc.remove(edge_idx);
                        }
                    }
                    if emptied && chunk_idx < vchunks.len() {
                        vchunks.remove(chunk_idx);
                    }
                    if vchunks.is_empty() {
                        self.overflow_values.take(src_vid);
                    }
                }
                self.topology.edge_count -= 1;
                if let Some((freed_cap, all_gone)) = freed {
                    self.topology.sub_capacity(freed_cap);
                    if all_gone {
                        self.topology.overflow_chunks.remove(src_vid);
                    }
                }
                self.topology.rebuild_live_set_for_vertex(src_vid);
                return true;
            }
        }
        false
    }
}
