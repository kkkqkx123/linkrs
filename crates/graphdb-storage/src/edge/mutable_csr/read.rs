use super::super::csr_shared::decode_endpoint_pair;
use super::super::{ColdStamps, EdgeId, HotNbr, Nbr, Timestamp, VertexId};
use super::write::EdgePosition;
use super::MutableCsr;

impl MutableCsr {
    /// Clamped primary window of one row as `(start, end)` list indices.
    ///
    /// Single bounds check per row: every row scan below goes through this
    /// instead of probing each slot with `get`, so per-slot branches and
    /// `Option` unwraps disappear from the hot paths.
    pub(super) fn primary_window(&self, src_idx: usize) -> (usize, usize) {
        if src_idx >= self.vertex_capacity() {
            return (0, 0);
        }
        let start = self.adj_offsets[src_idx] as usize;
        let end = start
            .saturating_add(self.degrees[src_idx] as usize)
            .min(self.hot_list.len())
            .min(self.cold_list.len());
        (start.min(end), end)
    }

    /// Primary hot slice of one row; empty when the row is missing or empty.
    pub(super) fn primary_hot(&self, src_idx: usize) -> &[HotNbr] {
        let (start, end) = self.primary_window(src_idx);
        &self.hot_list[start..end]
    }

    /// Primary hot plus cold slices of one row, locked to the same window.
    pub(super) fn primary_pair(&self, src_idx: usize) -> (&[HotNbr], &[ColdStamps]) {
        let (start, end) = self.primary_window(src_idx);
        (&self.hot_list[start..end], &self.cold_list[start..end])
    }

    /// Assembled slot copy at a wide-row index position.
    ///
    /// Row-relative addressing shared by the endpoint location fast paths:
    /// primary positions index from the row start, overflow positions address
    /// a chunk and a slot inside it. Returns `None` for out-of-range
    /// positions so stale entries fall back to the row scan.
    fn slot_at_position(
        &self,
        src_vid: u32,
        src_idx: usize,
        position: EdgePosition,
    ) -> Option<Nbr> {
        match position {
            EdgePosition::Primary { slot } => {
                let base = *self.adj_offsets.get(src_idx)? as usize;
                self.slot_at(base + slot as usize)
            }
            EdgePosition::Overflow { chunk, slot } => self
                .overflow_chunks
                .get(&src_vid)?
                .get(chunk as usize)?
                .slot_at(slot as usize),
        }
    }

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
        self.slot_at(idx)
    }

    /// Locate the first live edge by endpoint without consulting snapshots.
    ///
    /// Physical addressing only; tombstoned and gap slots are skipped.
    /// Snapshot visibility is decided by the version authority above this
    /// layer, while tombstone access uses the full physical views.
    ///
    /// Wide rows answer from the endpoint location index: a present key
    /// addresses its slot directly, an absent key returns without scanning.
    /// Narrow rows without an index fall through to the linear walk.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        use super::super::INVALID_EDGE_ID;
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }
        if let Some(set) = self.live_sets.get(&src_vid) {
            let Some(position) = set.position(&(decoded_endpoint, decoded_rank)) else {
                return None;
            };
            // Position mismatch means the row moved without a rebuild, which
            // must not happen; fall back to the scan instead of answering
            // from the wrong slot.
            if let Some(nbr) = self.slot_at_position(src_vid, src_idx, position) {
                if nbr.endpoint == decoded_endpoint
                    && nbr.rank == decoded_rank
                    && nbr.edge_id != INVALID_EDGE_ID
                    && nbr.delete_ts == Timestamp::MAX
                {
                    return Some(nbr);
                }
            }
        }
        let (hot, cold) = self.primary_pair(src_idx);
        for (h, c) in hot.iter().zip(cold.iter()) {
            if h.endpoint == decoded_endpoint
                && h.rank == decoded_rank
                && h.edge_id != INVALID_EDGE_ID
                && c.is_live()
            {
                return Some(Nbr::from_parts(*h, *c));
            }
        }
        // Consolidated single-block rows skip the chain loop.
        if let Some(single) = self.overflow_chunks.single_chunk(&src_vid) {
            for (hot, cold) in single.hot_slice().iter().zip(single.cold_slice()) {
                if hot.endpoint == decoded_endpoint
                    && hot.rank == decoded_rank
                    && hot.edge_id != INVALID_EDGE_ID
                    && cold.is_live()
                {
                    return Some(Nbr::from_parts(*hot, *cold));
                }
            }
            return None;
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for (hot, cold) in chunk.hot_slice().iter().zip(chunk.cold_slice()) {
                    if hot.endpoint == decoded_endpoint
                        && hot.rank == decoded_rank
                        && hot.edge_id != INVALID_EDGE_ID
                        && cold.is_live()
                    {
                        return Some(Nbr::from_parts(*hot, *cold));
                    }
                }
            }
        }
        None
    }

    /// Every physically stored entry of one vertex without timestamp filtering.
    ///
    /// Visibility is decided by the version authority above this layer.
    /// Test and offline use; production scans use [`Self::fill_physical_into`]
    /// or the visitor paths instead of this allocating accessor.
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
        let (hot, cold) = self.primary_pair(src_idx);
        out.reserve(hot.len());
        out.extend(
            hot.iter()
                .zip(cold.iter())
                .map(|(h, c)| Nbr::from_parts(*h, *c)),
        );
        if let Some(single) = self.overflow_chunks.single_chunk(&src_vid) {
            out.reserve(single.len());
            for i in 0..single.len() {
                if let Some(nbr) = single.slot_at(i) {
                    out.push(nbr);
                }
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                out.reserve(chunk.len());
                for i in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(i) {
                        out.push(nbr);
                    }
                }
            }
        }
    }

    /// Visit every physically stored hot half of one vertex without
    /// allocating and without touching the stamp lines.
    ///
    /// Hot-only counterpart of [`Self::visit_physical`] for traversals that
    /// resolve visibility through the version authority by `edge_id`. Stamps
    /// stay out of cache on this walk. Consolidated rows read their single
    /// overflow block without the chain loop.
    pub fn visit_hot<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        for h in self.primary_hot(src_idx) {
            if !f(*h) {
                return;
            }
        }
        if let Some(single) = self.overflow_chunks.single_chunk(&src_vid) {
            for h in single.hot_slice() {
                if !f(*h) {
                    return;
                }
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for h in chunk.hot_slice() {
                    if !f(*h) {
                        return;
                    }
                }
            }
        }
    }

    /// Visit every physically stored entry of one vertex without allocating.
    ///
    /// The visitor returns false to stop early. Visibility is decided by the
    /// version authority above this layer. Consolidated rows read their
    /// single overflow block without the chain loop.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (hot, cold) = self.primary_pair(src_idx);
        for (h, c) in hot.iter().zip(cold.iter()) {
            if !f(Nbr::from_parts(*h, *c)) {
                return;
            }
        }
        if let Some(single) = self.overflow_chunks.single_chunk(&src_vid) {
            for i in 0..single.len() {
                if let Some(nbr) = single.slot_at(i) {
                    if !f(nbr) {
                        return;
                    }
                }
            }
            return;
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(i) {
                        if !f(nbr) {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Visit every physically stored entry with its row position.
    ///
    /// Same walk as [`Self::visit_physical`] but the visitor also receives
    /// the [`EdgePosition`] of each entry, so a later delete or revert can
    /// address the slot directly instead of rescanning the row. Positions
    /// stay valid only until the next compaction, rebalance, repack or
    /// removal of this row; positional writes revalidate the edge id.
    pub fn visit_physical_with_position<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(EdgePosition, Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (hot, cold) = self.primary_pair(src_idx);
        for (i, (h, c)) in hot.iter().zip(cold.iter()).enumerate() {
            if !f(
                EdgePosition::Primary { slot: i as u32 },
                Nbr::from_parts(*h, *c),
            ) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for slot_idx in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(slot_idx) {
                        if !f(
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                            nbr,
                        ) {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Locate the first entry with `edge_id`, returning its position and copy.
    ///
    /// Single scan shared by delete and revert callers that then act on the
    /// position directly.
    pub fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
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
        self.primary_hot(src_idx)
            .iter()
            .any(|hot| hot.edge_id == edge_id)
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

    /// Get edges of a vertex at a given timestamp.
    ///
    /// Test and offline use; production traversals use the visitor or
    /// caller-buffer fill paths instead of this allocating accessor.
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return Vec::new();
        }

        let (hot, cold) = self.primary_pair(src_idx);
        let overflow_len = self
            .overflow_chunks
            .get(&src_vid)
            .map(|chunks| chunks.iter().map(|chunk| chunk.len()).sum::<usize>())
            .unwrap_or(0);
        let mut result = Vec::with_capacity(hot.len() + overflow_len);

        result.extend(hot.iter().zip(cold.iter()).filter_map(|(h, c)| {
            let nbr = Nbr::from_parts(*h, *c);
            nbr.is_alive_at(ts).then_some(nbr)
        }));

        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(i) {
                        if nbr.is_alive_at(ts) {
                            result.push(nbr);
                        }
                    }
                }
            }
        }

        result
    }

    /// Get a specific edge
    ///
    /// Wide rows consult the endpoint location index first: a present key
    /// addresses its slot directly when the live entry covers `ts`. Absent
    /// keys at the maximum timestamp return without scanning; any other
    /// timestamp falls through to the row walk so historically visible
    /// tombstoned versions are still found.
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return None;
        }

        if let Some(set) = self.live_sets.get(&src_vid) {
            match set.position(&(decoded_endpoint, decoded_rank)) {
                Some(position) => {
                    if let Some(nbr) = self.slot_at_position(src_vid, src_idx, position) {
                        if nbr.endpoint == decoded_endpoint
                            && nbr.rank == decoded_rank
                            && nbr.delete_ts == Timestamp::MAX
                            && nbr.create_ts <= ts
                        {
                            return Some(nbr);
                        }
                    }
                }
                None if ts == Timestamp::MAX => return None,
                None => {}
            }
        }
        // Scan primary: compare the packed key halves directly and only
        // assemble the record on a key match.
        let (hot, cold) = self.primary_pair(src_idx);
        for (h, c) in hot.iter().zip(cold.iter()) {
            if h.endpoint == decoded_endpoint && h.rank == decoded_rank {
                let nbr = Nbr::from_parts(*h, *c);
                if nbr.is_alive_at(ts) {
                    return Some(nbr);
                }
            }
        }

        // Scan overflow: consolidated single-block rows skip the chain loop.
        if let Some(single) = self.overflow_chunks.single_chunk(&src_vid) {
            for i in 0..single.len() {
                if let Some(nbr) = single.slot_at(i) {
                    if nbr.endpoint == decoded_endpoint
                        && nbr.rank == decoded_rank
                        && nbr.is_alive_at(ts)
                    {
                        return Some(nbr);
                    }
                }
            }
            return None;
        }
        if let Some(chunks) = self.overflow_chunks.get(&src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(i) {
                        if nbr.endpoint == decoded_endpoint
                            && nbr.rank == decoded_rank
                            && nbr.is_alive_at(ts)
                        {
                            return Some(nbr);
                        }
                    }
                }
            }
        }

        None
    }
}
