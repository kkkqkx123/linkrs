use super::super::csr_shared::decode_endpoint_pair;
use super::super::{EdgeId, Nbr, Timestamp, VertexId};
use super::MutableCsr;

impl MutableCsr {
    /// Read-only view of one primary slot without mutating state.
    ///
    /// Used to verify that a caller-supplied offset still addresses the
    /// expected edge before a destructive offset write runs.
    pub fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        if offset < 0 {
            return None;
        }
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() || self.primary_capacities[src_idx] == 0 {
            return None;
        }
        if offset as usize >= self.degrees[src_idx] as usize {
            return None;
        }
        let idx = self.adj_offsets[src_idx] as usize + offset as usize;
        self.nbr_list.get(idx).copied()
    }

    /// Locate the first live edge by endpoint without consulting snapshots.
    ///
    /// Physical addressing only; tombstoned and gap slots are skipped.
    /// Snapshot visibility is decided by the version authority above this
    /// layer, while tombstone access uses the full physical views.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        use super::super::INVALID_EDGE_ID;
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.endpoint == decoded_endpoint
                    && nbr.rank == decoded_rank
                    && nbr.edge_id != INVALID_EDGE_ID
                    && nbr.delete_ts == Timestamp::MAX
                {
                    return Some(*nbr);
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.edge_id != INVALID_EDGE_ID
                        && nbr.delete_ts == Timestamp::MAX
                    {
                        return Some(*nbr);
                    }
                }
            }
        }
        None
    }

    /// Every physically stored entry of one vertex without timestamp filtering.
    ///
    /// Visibility is decided by the version authority above this layer.
    pub fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src_vid, &mut out);
        out
    }

    /// Fill a caller buffer with every physically stored entry of one vertex.
    ///
    /// Same content as the allocating accessor above, without the per-vertex
    /// allocation. Batch scans reuse one buffer across vertices and slice it
    /// per vertex instead of collecting one vector per vertex.
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        out.reserve(degree);
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                out.push(*nbr);
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                out.extend_from_slice(chunk);
            }
        }
    }

    /// Visit every physically stored entry of one vertex without allocating.
    ///
    /// The visitor returns false to stop early. Visibility is decided by the
    /// version authority above this layer.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if !f(*nbr) {
                    return;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if !f(*nbr) {
                        return;
                    }
                }
            }
        }
    }

    /// Whether the primary row of one vertex holds `edge_id`.
    ///
    /// Used to distinguish a stale offset (edge lives in primary at a
    /// different offset, must fail) from an overflow row (no offset can
    /// address it, may fall back to the edge-id path).
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return false;
        }
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            if self
                .nbr_list
                .get(offset + i)
                .is_some_and(|nbr| nbr.edge_id == edge_id)
            {
                return true;
            }
        }
        false
    }

    /// Whether one vertex holds any physically stored entry.
    pub fn has_physical_entries(&self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return false;
        }
        if self.degrees[idx] > 0 {
            return true;
        }
        self.overflow_chunks
            .get(&vid)
            .is_some_and(|chunks| chunks.iter().any(|c| !c.is_empty()))
    }

    /// Get edges of a vertex at a given timestamp
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        let overflow_len = self
            .overflow_chunks
            .get(&src_vid)
            .map(|chunks| chunks.iter().map(Vec::len).sum::<usize>())
            .unwrap_or(0);
        let mut result = Vec::with_capacity(degree + overflow_len);

        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.is_alive_at(ts) {
                result.push(*nbr);
            }
        }

        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.is_alive_at(ts) {
                        result.push(*nbr);
                    }
                }
            }
        }

        result
    }

    /// Get a specific edge
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        // Scan primary
        let degree = self.degrees[src_idx] as usize;
        let offset = self.adj_offsets[src_idx] as usize;
        for i in 0..degree {
            let nbr = &self.nbr_list[offset + i];
            if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank && nbr.is_alive_at(ts) {
                return Some(*nbr);
            }
        }

        // Scan overflow
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.is_alive_at(ts)
                    {
                        return Some(*nbr);
                    }
                }
            }
        }

        None
    }
}
