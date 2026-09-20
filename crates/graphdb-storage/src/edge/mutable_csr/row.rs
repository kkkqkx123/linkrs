use super::super::csr_shared::is_reclaimable_cold;
use super::super::{ColdStamps, EdgeId, HotNbr, Nbr, Timestamp};
use super::overflow::OverflowChunk;
use super::MutableCsr;

/// Target density for packed rows: live entries per unit of reserved row
/// capacity. Rebuilds size rows to `ceil(live / PACKED_CSR_DENSITY)` so
/// everyday writes land in row gaps before spilling to overflow.
pub(crate) const PACKED_CSR_DENSITY: f32 = 0.8;
/// Minimum primary slots kept for a live row after a row rebalance, so tiny
/// rows still hold a small write gap without another allocation.
pub(crate) const MIN_ROW_CAPACITY: usize = 4;

/// Proportional overflow sizing, single benchmarked scheme. Chunk sizes
/// grow geometrically with live row width from a small floor, so a 5-edge
/// row reserves 8 slots instead of 256. The effective size is
/// `min(configured, graded(live))` so explicit test configurations keep
/// their exact size.
pub(crate) const OVERFLOW_CHUNK_MIN: usize = 8;
pub(crate) const OVERFLOW_CHUNK_MAX: usize = 4096;

/// Overflow chunk size proportional to live row width: the next power of two
/// above `live` with a small floor and a hard cap. Monotonic in `live`, so
/// rows never shrink their chunk size as they grow.
pub(crate) fn graded_overflow_chunk_edges(live: usize) -> usize {
    live.next_power_of_two()
        .max(OVERFLOW_CHUNK_MIN)
        .min(OVERFLOW_CHUNK_MAX)
}

impl MutableCsr {
    /// Row capacity holding `live` entries at the packed density target.
    pub(crate) fn sized_row_capacity(live: usize) -> usize {
        if live == 0 {
            return 0;
        }
        ((live as f32 / PACKED_CSR_DENSITY).ceil() as usize).max(MIN_ROW_CAPACITY.min(live.max(1)))
    }

    /// Effective overflow chunk size for a row with `live` live entries.
    pub(crate) fn effective_chunk_edges(&self, live: usize) -> usize {
        graded_overflow_chunk_edges(live)
            .min(self.overflow_chunk_edges)
            .max(1)
    }

    /// Reserved primary slots minus live primary entries of one row.
    pub fn row_gap(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let (_, cold) = self.primary_pair(idx);
        let live = cold.iter().filter(|c| c.is_live()).count();
        (self.rows.primary_capacities[idx] as usize).saturating_sub(live)
    }

    /// Live primary entries per unit of reserved primary capacity.
    pub fn row_density(&self, vid: u32) -> f32 {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 1.0;
        }
        let cap = self.rows.primary_capacities[idx] as usize;
        if cap == 0 {
            return 1.0;
        }
        let (_, cold) = self.primary_pair(idx);
        let live = cold.iter().filter(|c| c.is_live()).count();
        live as f32 / cap as f32
    }

    /// Rebalance one row in place: tighten live primary entries to the
    /// front, pull overflow live entries into primary gaps, and repack any
    /// leftover overflow into graded chunks. Primary capacity never grows
    /// here, so other rows never move; capacity growth stays with full and
    /// region compactions that size rows at the density target. Pinned
    /// tombstones stay in place. Returns true when overflow shrank.
    pub fn rebalance_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() || self.rows.primary_capacities[idx] == 0 {
            return false;
        }
        if self.overflow_chunks.get(vid).is_none_or(Vec::is_empty) {
            return false;
        }
        let degree = self.rows.degrees[idx] as usize;
        let offset = self.rows.adj_offsets[idx] as usize;
        let cap = self.rows.primary_capacities[idx] as usize;
        let mut live: Vec<Nbr> = Vec::new();
        let mut pinned: Vec<Nbr> = Vec::new();
        for i in 0..degree {
            if let Some(nbr) = self.slot_at(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    live.push(nbr);
                } else {
                    pinned.push(nbr);
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if let Some(nbr) = chunk.slot_at(i) {
                        if nbr.delete_ts == Timestamp::MAX {
                            live.push(nbr);
                        } else {
                            pinned.push(nbr);
                        }
                    }
                }
            }
        }
        let slots = cap.min(live.len() + pinned.len());
        let mut placed = 0usize;
        for nbr in live.iter().chain(pinned.iter()).take(slots) {
            if offset + placed < self.hot_list.len() {
                self.set_slot(offset + placed, *nbr);
            }
            placed += 1;
        }
        self.rows.degrees[idx] = placed as u32;
        self.invalidate_reuse_hint(idx);
        let placed_live = live.len().min(slots);
        let overflow_live: Vec<Nbr> = live.into_iter().skip(placed_live).collect();
        let placed_pinned = pinned.len().min(slots.saturating_sub(placed_live));
        let overflow_pinned: Vec<Nbr> = pinned.into_iter().skip(placed_pinned).collect();
        let old_overflow_cap: usize = self.overflow_chunks.get(vid).map_or(0, |chunks| {
            chunks.iter().map(|chunk| chunk.capacity()).sum()
        });
        if overflow_live.is_empty() && overflow_pinned.is_empty() {
            self.overflow_chunks.remove(vid);
            self.sub_capacity(old_overflow_cap);
        } else {
            let mut rest: Vec<Nbr> =
                Vec::with_capacity(overflow_live.len() + overflow_pinned.len());
            rest.extend_from_slice(&overflow_live);
            rest.extend_from_slice(&overflow_pinned);
            // Same single-block consolidation as the repack path: leftover
            // overflow reads as one block after the primary row.
            let single = OverflowChunk::consolidated(&rest);
            let new_overflow_cap = single.capacity();
            self.sub_capacity(old_overflow_cap);
            self.add_capacity(new_overflow_cap);
            if let Some(slot) = self.overflow_chunks.get_mut(vid) {
                *slot = vec![single];
            }
        }
        self.rebuild_live_set_for_vertex(vid);
        self.overflow_chunks.get(vid).is_none_or(Vec::is_empty)
    }

    /// Expand a single vertex's primary block in place.
    ///
    /// The "middle gear" between gap-fill and overflow: when a narrow row's
    /// primary block is full and no tombstone is reusable, this copies the
    /// row to a fresh tail block with doubled capacity instead of spilling
    /// to overflow.  The old primary block becomes dead gaps reclaimable by
    /// the next compaction.
    ///
    /// Returns `true` when expansion succeeded and the caller may retry
    /// the gap-fill write; `false` when the row is too wide or the table
    /// is too large for a single-vertex expansion (fall through to
    /// overflow).
    #[allow(dead_code)]
    pub(crate) fn expand_vertex_primary(&mut self, src_idx: usize) -> bool {
        let current_cap = self.rows.primary_capacities[src_idx] as usize;
        if current_cap == 0 {
            return false;
        }
        let degree = self.rows.degrees[src_idx] as usize;
        let new_cap = Self::sized_row_capacity(degree + 1);
        if new_cap <= current_cap {
            return false;
        }
        // Cap single-vertex expansion: beyond this threshold overflow
        // chunks are more space-efficient.  The cap also prevents
        // unbounded tail growth when the row keeps growing after
        // expansion.
        if new_cap > self.overflow_chunk_edges {
            return false;
        }

        let old_offset = self.rows.adj_offsets[src_idx] as usize;
        let new_offset = self.hot_list.len();

        // Copy existing entries to temporary buffers to avoid borrow
        // conflicts, then append to the tail of the primary lists.
        let hot_src = self.hot_list[old_offset..old_offset + degree].to_vec();
        let cold_src = self.cold_list[old_offset..old_offset + degree].to_vec();
        self.hot_list.extend_from_slice(&hot_src);
        self.cold_list.extend_from_slice(&cold_src);

        // Fill remaining capacity with gap sentinels.
        let extra = new_cap - degree;
        self.hot_list
            .resize(new_offset + new_cap, HotNbr::dead_gap());
        self.cold_list
            .resize(new_offset + new_cap, ColdStamps::dead_gap());

        // Redirect the vertex to the new block.
        self.rows.adj_offsets[src_idx] = new_offset as u32;
        self.rows.primary_capacities[src_idx] = new_cap as u32;
        // degrees stays the same (we copied `degree` entries).

        // Capacity accounting: old block becomes dead gaps (still counted
        // in total_edge_capacity via the delta), new slots are the extra.
        self.add_capacity(extra);

        // Invalidate stale hints and rebuild the live set for the new
        // positions.
        self.invalidate_reuse_hint(src_idx);
        self.rebuild_live_set_for_vertex(src_idx as u32);

        true
    }

    pub(crate) fn compact_overflow_for_vertex(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) {
        let Some(chunks) = self.overflow_chunks.remove(vid) else {
            return;
        };
        let old_cap: usize = chunks.iter().map(|c| c.capacity()).sum();
        self.sub_capacity(old_cap);
        let removals_enabled = cutoff != Timestamp::MAX;
        let mut kept: Vec<Nbr> = Vec::new();
        for chunk in &chunks {
            for i in 0..chunk.len() {
                let nbr = chunk.slot_at(i).unwrap();
                if removals_enabled && is_reclaimable_cold(&nbr.cold(), cutoff) {
                    on_edge_removed(nbr.edge_id, nbr.delete_ts);
                } else {
                    kept.push(nbr);
                }
            }
        }
        if kept.is_empty() {
            self.rebuild_live_set_for_vertex(vid);
            return;
        }
        // Vertex-level expansion: absorb overflow entries into the primary
        // block so the row reads entirely from primary without overflow
        // indirection.  The primary block is expanded at the tail of the
        // hot/cold lists; the old block becomes dead gaps reclaimable by
        // the next full compaction.
        let idx = vid as usize;
        if idx < self.vertex_capacity() && self.rows.primary_capacities[idx] > 0 {
            let degree = self.rows.degrees[idx] as usize;
            let needed = degree + kept.len();
            let target_cap = Self::sized_row_capacity(needed);
            // Only expand when the target fits within the chunk-edge
            // budget; wider rows stay with a single overflow chunk.
            if target_cap <= self.overflow_chunk_edges {
                let current_cap = self.rows.primary_capacities[idx] as usize;
                let old_offset = self.rows.adj_offsets[idx] as usize;
                let new_offset = self.hot_list.len();
                // Copy primary entries to the new tail block.
                let hot_src = self.hot_list[old_offset..old_offset + degree].to_vec();
                let cold_src = self.cold_list[old_offset..old_offset + degree].to_vec();
                self.hot_list.extend_from_slice(&hot_src);
                self.cold_list.extend_from_slice(&cold_src);
                // Append overflow entries right after primary.
                for nbr in &kept {
                    self.hot_list.push(nbr.hot());
                    self.cold_list.push(nbr.cold());
                }
                // Fill remaining capacity with gap sentinels.
                self.hot_list
                    .resize(new_offset + target_cap, HotNbr::dead_gap());
                self.cold_list
                    .resize(new_offset + target_cap, ColdStamps::dead_gap());
                self.rows.adj_offsets[idx] = new_offset as u32;
                self.rows.degrees[idx] = needed as u32;
                self.rows.primary_capacities[idx] = target_cap as u32;
                if target_cap > current_cap {
                    self.add_capacity(target_cap - current_cap);
                }
                self.invalidate_reuse_hint(idx);
                self.rebuild_live_set_for_vertex(vid);
                self.vertex_expansion_count += 1;
                return;
            }
        }
        // Fallback: consolidate into one overflow chunk.
        let single = OverflowChunk::consolidated(&kept);
        let new_cap = single.capacity();
        self.add_capacity(new_cap);
        self.overflow_chunks.insert(vid, vec![single]);
        self.rebuild_live_set_for_vertex(vid);
    }
}
