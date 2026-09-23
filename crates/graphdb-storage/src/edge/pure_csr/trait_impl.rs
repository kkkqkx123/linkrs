use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::StorageResult;

use super::super::csr_trait::MutableCsrTrait;
use super::super::{EdgePosition, Nbr};
use super::overflow::PureOverflowChunk;
use super::{PureTopologyCsr, INVALID_EDGE_ID};

impl MutableCsrTrait for PureTopologyCsr {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        self.insert_edge_returning_position(src_vid, dst, edge_id)
            .map(|_| ())
    }

    fn delete_edge(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        if edge_id == INVALID_EDGE_ID {
            return Ok(false);
        }

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }

        let found = {
            let (start, end) = self.primary_window(src_idx);
            self.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some((start + i, self.endpoints[start + i]))
                    } else {
                        None
                    }
                })
        };
        if let Some((idx, endpoint)) = found {
            self.edge_ids[idx] = INVALID_EDGE_ID.0;
            self.edge_count -= 1;
            self.track_live_remove(src_vid, endpoint);
            return Ok(true);
        }

        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let endpoint =
                self.overflow_chunks.get(src_vid).unwrap()[chunk_idx].endpoints[edge_idx];
            if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].edge_ids[edge_idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        let mut noop = |_: EdgeId| {};
        self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut noop)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        _ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        let Some((decoded_endpoint, _decoded_rank)) = dst.try_decode_edge_endpoint() else {
            return 0;
        };
        let Some(target_endpoint) = decoded_endpoint.as_internal_u32() else {
            return 0;
        };
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            if self.endpoints[i] == target_endpoint && self.edge_ids[i] != INVALID_EDGE_ID.0 {
                let eid = EdgeId(self.edge_ids[i]);
                self.edge_ids[i] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                on_deleted(eid);
                deleted += 1;
            }
        }

        if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
            for chunk in chunks.iter_mut() {
                let to_delete: Vec<(usize, u64)> = chunk
                    .edge_ids
                    .iter()
                    .enumerate()
                    .filter(|(i, &eid)| {
                        chunk.endpoints[*i] == target_endpoint && EdgeId(eid) != INVALID_EDGE_ID
                    })
                    .map(|(i, &eid)| (i, eid))
                    .collect();
                for (i, eid) in to_delete {
                    chunk.edge_ids[i] = INVALID_EDGE_ID.0;
                    self.edge_count -= 1;
                    on_deleted(EdgeId(eid));
                    deleted += 1;
                }
            }
        }

        if deleted > 0 {
            self.track_live_remove(src_vid, target_endpoint);
        }

        deleted
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        _ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        let Some((decoded_endpoint, _decoded_rank)) = dst.try_decode_edge_endpoint() else {
            return 0;
        };
        let Some(target_endpoint) = decoded_endpoint.as_internal_u32() else {
            return 0;
        };
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return 0;
        }

        let mut deleted = 0usize;

        let (start, end) = self.primary_window(src_idx);
        for (i, idx) in (start..end).enumerate() {
            if self.endpoints[idx] == target_endpoint && self.edge_ids[idx] != INVALID_EDGE_ID.0 {
                let eid = EdgeId(self.edge_ids[idx]);
                self.edge_ids[idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                on_deleted(eid, Some(EdgePosition::Primary { slot: i as u32 }));
                deleted += 1;
            }
        }

        if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
            for (chunk_idx, chunk) in chunks.iter_mut().enumerate() {
                let to_delete: Vec<(usize, u64)> = chunk
                    .edge_ids
                    .iter()
                    .enumerate()
                    .filter(|(i, &eid)| {
                        chunk.endpoints[*i] == target_endpoint && EdgeId(eid) != INVALID_EDGE_ID
                    })
                    .map(|(i, &eid)| (i, eid))
                    .collect();
                for (slot_idx, eid) in to_delete {
                    chunk.edge_ids[slot_idx] = INVALID_EDGE_ID.0;
                    self.edge_count -= 1;
                    on_deleted(
                        EdgeId(eid),
                        Some(EdgePosition::Overflow {
                            chunk: chunk_idx as u32,
                            slot: slot_idx as u32,
                        }),
                    );
                    deleted += 1;
                }
            }
        }

        if deleted > 0 {
            self.track_live_remove(src_vid, target_endpoint);
        }

        deleted
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let mut found = None;
        self.visit_physical_with_position(src_vid, |position, nbr| {
            if nbr.edge_id == edge_id {
                found = Some((position, nbr));
                false
            } else {
                true
            }
        });
        found
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return Ok(false);
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return Ok(false);
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                if idx >= self.edge_ids.len() || EdgeId(self.edge_ids[idx]) != expected {
                    return Ok(false);
                }
                if self.edge_ids[idx] == INVALID_EDGE_ID.0 {
                    return Ok(false);
                }
                let endpoint = self.endpoints[idx];
                self.edge_ids[idx] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                Ok(true)
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return Ok(false);
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return Ok(false);
                };
                if slot as usize >= c.len() {
                    return Ok(false);
                }
                if c.edge_id_at(slot as usize) != Some(expected) {
                    return Ok(false);
                }
                if c.edge_ids[slot as usize] == INVALID_EDGE_ID.0 {
                    return Ok(false);
                }
                let endpoint = c.endpoints[slot as usize];
                c.edge_ids[slot as usize] = INVALID_EDGE_ID.0;
                self.edge_count -= 1;
                self.track_live_remove(src_vid, endpoint);
                Ok(true)
            }
        }
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        match position {
            EdgePosition::Primary { slot } => {
                let src_idx = src_vid as usize;
                if src_idx >= self.vertex_capacity() {
                    return false;
                }
                if slot as usize >= self.rows.degrees[src_idx] as usize {
                    return false;
                }
                let idx = self.rows.adj_offsets[src_idx] as usize + slot as usize;
                if idx >= self.edge_ids.len() {
                    return false;
                }
                if self.edge_ids[idx] != INVALID_EDGE_ID.0 {
                    return false;
                }
                self.edge_ids[idx] = expected.0;
                self.edge_count += 1;
                let endpoint = self.endpoints[idx];
                self.track_live_insert(src_vid, endpoint, EdgePosition::Primary { slot });
                true
            }
            EdgePosition::Overflow { chunk, slot } => {
                let Some(chunks) = self.overflow_chunks.get_mut(src_vid) else {
                    return false;
                };
                let Some(c) = chunks.get_mut(chunk as usize) else {
                    return false;
                };
                if slot as usize >= c.len() {
                    return false;
                }
                if c.edge_ids[slot as usize] != INVALID_EDGE_ID.0 {
                    return false;
                }
                let endpoint = c.endpoints[slot as usize];
                c.edge_ids[slot as usize] = expected.0;
                self.edge_count += 1;
                self.track_live_insert(src_vid, endpoint, EdgePosition::Overflow { chunk, slot });
                true
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        if offset < 0 {
            return Ok(false);
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Ok(false);
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return Ok(false);
        }
        let idx = self.rows.adj_offsets[src_idx] as usize + offset as usize;
        if idx >= self.edge_ids.len() {
            return Ok(false);
        }
        if self.edge_ids[idx] == INVALID_EDGE_ID.0 {
            return Ok(false);
        }
        let endpoint = self.endpoints[idx];
        self.edge_ids[idx] = INVALID_EDGE_ID.0;
        self.edge_count -= 1;
        self.track_live_remove(src_vid, endpoint);
        Ok(true)
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        // Deletion overwrites the edge id with the unassignable sentinel, so
        // an offset-only revert cannot recover the erased identity. Callers
        // needing restore must keep the edge id and use the positioned path.
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        if offset < 0 {
            return None;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.rows.primary_capacities[src_idx] == 0 {
            return None;
        }
        if offset as usize >= self.rows.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.rows.adj_offsets[src_idx] as usize + offset as usize;
        let endpoint = *self.endpoints.get(idx)?;
        let edge_id = EdgeId(*self.edge_ids.get(idx)?);
        Some(self.make_nbr(endpoint, edge_id))
    }

    /// Locate the first live edge by endpoint without consulting snapshots.
    ///
    /// Wide rows answer from the endpoint location index: a present key
    /// addresses its slot directly, an absent key returns without scanning.
    /// Narrow rows without an index fall through to the linear walk.
    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, _decoded_rank) = dst.try_decode_edge_endpoint()?;
        let target_endpoint = decoded_endpoint.as_internal_u32()?;

        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        if let Some(set) = self.live_sets.get(src_vid) {
            let position = set.position(&target_endpoint)?;
            let nbr = match position {
                EdgePosition::Primary { slot } => {
                    let base = self.rows.adj_offsets[src_idx] as usize;
                    let idx = base + slot as usize;
                    let endpoint = *self.endpoints.get(idx)?;
                    let edge_id = EdgeId(*self.edge_ids.get(idx)?);
                    self.make_nbr(endpoint, edge_id)
                }
                EdgePosition::Overflow { chunk, slot } => {
                    let chunks = self.overflow_chunks.get(src_vid)?;
                    let c = chunks.get(chunk as usize)?;
                    let endpoint = c.endpoint_at(slot as usize)?;
                    let edge_id = c.edge_id_at(slot as usize)?;
                    self.make_nbr(endpoint, edge_id)
                }
            };
            // Position mismatch means the row moved without a rebuild, which
            // must not happen; fall back to the scan instead of answering
            // from the wrong slot.
            if nbr.endpoint == target_endpoint
                && nbr.edge_id != INVALID_EDGE_ID
                && nbr.delete_ts == Timestamp::MAX
            {
                return Some(nbr);
            }
        }

        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            if self.endpoints[i] == target_endpoint && self.edge_ids[i] != INVALID_EDGE_ID.0 {
                return Some(self.make_nbr(self.endpoints[i], EdgeId(self.edge_ids[i])));
            }
        }

        if let Some(single) = self.overflow_chunks.single_chunk(src_vid) {
            for i in 0..single.len() {
                if single.endpoints[i] == target_endpoint && single.edge_ids[i] != INVALID_EDGE_ID.0
                {
                    return Some(self.make_nbr(single.endpoints[i], EdgeId(single.edge_ids[i])));
                }
            }
            return None;
        }

        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if chunk.endpoints[i] == target_endpoint
                        && chunk.edge_ids[i] != INVALID_EDGE_ID.0
                    {
                        return Some(self.make_nbr(chunk.endpoints[i], EdgeId(chunk.edge_ids[i])));
                    }
                }
            }
        }

        None
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src_vid, &mut out);
        out
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        out.reserve(end - start);
        for i in start..end {
            let edge_id = EdgeId(self.edge_ids[i]);
            if edge_id == INVALID_EDGE_ID {
                continue;
            }
            out.push(self.make_nbr(self.endpoints[i], edge_id));
        }
        if let Some(single) = self.overflow_chunks.single_chunk(src_vid) {
            out.reserve(single.len());
            for i in 0..single.len() {
                let edge_id = EdgeId(single.edge_ids[i]);
                if edge_id == INVALID_EDGE_ID {
                    continue;
                }
                out.push(self.make_nbr(single.endpoints[i], edge_id));
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                out.reserve(chunk.len());
                for i in 0..chunk.len() {
                    let edge_id = EdgeId(chunk.edge_ids[i]);
                    if edge_id == INVALID_EDGE_ID {
                        continue;
                    }
                    out.push(self.make_nbr(chunk.endpoints[i], edge_id));
                }
            }
        }
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        if self.rows.degrees[idx] > 0 {
            return true;
        }
        self.overflow_chunks
            .get(vid)
            .is_some_and(|chunks| chunks.iter().any(|c| !c.is_empty()))
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let (start, end) = self.primary_window(src_idx);
        self.edge_ids[start..end]
            .iter()
            .any(|&eid| EdgeId(eid) == edge_id)
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }

        let found = {
            let (start, end) = self.primary_window(src_idx);
            self.edge_ids[start..end]
                .iter()
                .enumerate()
                .find_map(|(i, &eid)| {
                    if EdgeId(eid) == edge_id {
                        Some((start + i, self.endpoints[start + i]))
                    } else {
                        None
                    }
                })
        };
        if let Some((idx, _endpoint)) = found {
            let degree = self.rows.degrees[src_idx] as usize;
            let base = self.rows.adj_offsets[src_idx] as usize;
            self.endpoints
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.edge_ids
                .copy_within(base + idx - base + 1..base + degree, base + idx - base);
            self.rows.degrees[src_idx] -= 1;
            self.sub_capacity(1);
            self.edge_count -= 1;
            self.rebuild_live_set_for_vertex(src_vid);
            return true;
        }

        if let Some((chunk_idx, edge_idx)) = self.scan_overflow_for_edge_id(src_vid, edge_id) {
            let detached = if let Some(chunks) = self.overflow_chunks.get_mut(src_vid) {
                chunks[chunk_idx].remove(edge_idx);
                if chunks[chunk_idx].is_empty() {
                    let removed = chunks.remove(chunk_idx);
                    Some((removed.capacity(), chunks.is_empty()))
                } else {
                    None
                }
            } else {
                return false;
            };
            self.edge_count -= 1;
            if let Some((freed, emptied)) = detached {
                self.sub_capacity(freed);
                if emptied {
                    self.overflow_chunks.remove(src_vid);
                }
            }
            self.rebuild_live_set_for_vertex(src_vid);
            return true;
        }

        false
    }

    fn revert_delete_by_edge_id(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        // Deleted slots hold the sentinel instead of the edge id, so an
        // id-keyed scan cannot locate them. Restoration uses the positioned
        // path with the expected id supplied by the caller.
        false
    }

    /// Timestamp-filtered point lookup.
    ///
    /// Pure rows store no timestamps: [`Self::make_nbr`] stamps every live
    /// entry identically, so the timestamp carries no information here and
    /// this entry shares the physical indexed path exactly instead of
    /// duplicating its scan. Wide rows therefore get the same absent-key
    /// short-circuit as [`Self::get_edge_physical`].
    fn get_edge(&self, src_vid: u32, dst: VertexId, _ts: Timestamp) -> Option<Nbr> {
        self.get_edge_physical(src_vid, dst)
    }

    fn edges_of(&self, src_vid: u32, _ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let (start, end) = self.primary_window(src_idx);
        let overflow_len = self
            .overflow_chunks
            .get(src_vid)
            .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum::<usize>())
            .unwrap_or(0);
        let mut result = Vec::with_capacity((end - start) + overflow_len);

        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                result.push(self.make_nbr(self.endpoints[i], EdgeId(self.edge_ids[i])));
            }
        }

        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if chunk.edge_ids[i] != INVALID_EDGE_ID.0 {
                        result.push(self.make_nbr(chunk.endpoints[i], EdgeId(chunk.edge_ids[i])));
                    }
                }
            }
        }

        result
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }

        let mut removed = 0usize;

        let base = self.rows.adj_offsets[idx] as usize;
        let degree = self.rows.degrees[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let idx_i = base + i;
            if self.edge_ids[idx_i] == INVALID_EDGE_ID.0 {
                // Holes carry no edge identity: drop them silently instead
                // of reporting the sentinel to the authority above.
                removed += 1;
            } else {
                if keep != i {
                    self.endpoints[base + keep] = self.endpoints[idx_i];
                    self.edge_ids[base + keep] = self.edge_ids[idx_i];
                }
                keep += 1;
            }
        }
        self.rows.degrees[idx] = keep as u32;
        self.rows.primary_capacities[idx] = keep as u32;

        if self.overflow_chunks.get(vid).is_some() {
            let chunks = self.overflow_chunks.remove(vid).unwrap_or_default();
            let freed: usize = chunks.iter().map(|chunk| chunk.capacity()).sum();
            self.sub_capacity(freed);
            let mut kept_ep: Vec<u32> = Vec::new();
            let mut kept_eid: Vec<u64> = Vec::new();
            for chunk in &chunks {
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid == INVALID_EDGE_ID.0 {
                        removed += 1;
                    } else {
                        kept_ep.push(chunk.endpoints[i]);
                        kept_eid.push(eid);
                    }
                }
            }
            if !kept_ep.is_empty() {
                let single = PureOverflowChunk::consolidated(&kept_ep, &kept_eid);
                let added = single.capacity();
                self.add_capacity(added);
                self.overflow_chunks.insert(vid, vec![single]);
            }
            self.rebuild_live_set_for_vertex(vid);
        } else if removed > 0 {
            self.rebuild_live_set_for_vertex(vid);
        }

        removed
    }

    fn reclaimable_count(&self, vid: u32, _cutoff: Timestamp) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        self.vertex_census(vid).1
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (0, 0, 0);
        }
        let mut alive = 0usize;
        let mut dead = 0usize;
        let (start, end) = self.primary_window(idx);
        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                alive += 1;
            } else {
                dead += 1;
            }
        }
        let mut capacity = self.rows.primary_capacities[idx] as usize;
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            capacity += chunks.iter().map(|chunk| chunk.capacity()).sum::<usize>();
            for chunk in chunks {
                for &eid in &chunk.edge_ids {
                    if eid != INVALID_EDGE_ID.0 {
                        alive += 1;
                    } else {
                        dead += 1;
                    }
                }
            }
        }
        (alive, dead, capacity)
    }

    fn row_gap(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        self.rows.primary_capacities[idx].saturating_sub(self.rows.degrees[idx]) as usize
    }

    fn row_density(&self, vid: u32) -> f32 {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 1.0;
        }
        let cap = self.rows.primary_capacities[idx] as f32;
        if cap == 0.0 {
            return 1.0;
        }
        self.rows.degrees[idx] as f32 / cap
    }

    fn used_memory_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.endpoints.len() * 4
            + self.edge_ids.len() * 8
            + self.rows.adj_offsets.len() * 4 * 3
            + self.primary_sorted.capacity() * std::mem::size_of::<bool>()
            + self.overflow_chunks.index_bytes()
            + self.overflow_chunks.total_entry_count() * (4 + 8)
            + self.live_sets.index_bytes()
            + self.live_sets.heap_bytes_total()
    }
}
