use super::super::csr_shared::is_reclaimable_slot;
use super::super::{EdgeId, Nbr, Timestamp};
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
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut live = 0usize;
        for i in 0..degree {
            if self
                .nbr_list
                .get(offset + i)
                .is_some_and(|nbr| nbr.delete_ts == Timestamp::MAX)
            {
                live += 1;
            }
        }
        (self.primary_capacities[idx] as usize).saturating_sub(live)
    }

    /// Live primary entries per unit of reserved primary capacity.
    pub fn row_density(&self, vid: u32) -> f32 {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 1.0;
        }
        let cap = self.primary_capacities[idx] as usize;
        if cap == 0 {
            return 1.0;
        }
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut live = 0usize;
        for i in 0..degree {
            if self
                .nbr_list
                .get(offset + i)
                .is_some_and(|nbr| nbr.delete_ts == Timestamp::MAX)
            {
                live += 1;
            }
        }
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
        if idx >= self.vertex_capacity() || self.primary_capacities[idx] == 0 {
            return false;
        }
        if self.overflow_chunks.get(&vid).is_none_or(Vec::is_empty) {
            return false;
        }
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let cap = self.primary_capacities[idx] as usize;
        let mut live: Vec<Nbr> = Vec::new();
        let mut pinned: Vec<Nbr> = Vec::new();
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    live.push(*nbr);
                } else {
                    pinned.push(*nbr);
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        live.push(*nbr);
                    } else {
                        pinned.push(*nbr);
                    }
                }
            }
        }
        let slots = cap.min(live.len() + pinned.len());
        let mut placed = 0usize;
        for nbr in live.iter().chain(pinned.iter()).take(slots) {
            if offset + placed < self.nbr_list.len() {
                self.nbr_list[offset + placed] = *nbr;
            }
            placed += 1;
        }
        self.degrees[idx] = placed as u32;
        let placed_live = live.len().min(slots);
        let overflow_live: Vec<Nbr> = live.into_iter().skip(placed_live).collect();
        let placed_pinned = pinned.len().min(slots.saturating_sub(placed_live));
        let overflow_pinned: Vec<Nbr> = pinned.into_iter().skip(placed_pinned).collect();
        let old_overflow_cap: usize = self
            .overflow_chunks
            .get(&vid)
            .map_or(0, |chunks| chunks.iter().map(Vec::capacity).sum());
        if overflow_live.is_empty() && overflow_pinned.is_empty() {
            self.overflow_chunks.remove(&vid);
            self.sub_capacity(old_overflow_cap);
        } else {
            let mut rest: Vec<Nbr> =
                Vec::with_capacity(overflow_live.len() + overflow_pinned.len());
            rest.extend_from_slice(&overflow_live);
            rest.extend_from_slice(&overflow_pinned);
            let chunk_edges = self.effective_chunk_edges(placed_live);
            let mut repacked: Vec<Vec<Nbr>> = Vec::new();
            for piece in rest.chunks(chunk_edges.max(1)) {
                let mut v = Vec::with_capacity(chunk_edges.max(1));
                v.extend_from_slice(piece);
                repacked.push(v);
            }
            let new_overflow_cap: usize = repacked.iter().map(Vec::capacity).sum();
            self.sub_capacity(old_overflow_cap);
            self.add_capacity(new_overflow_cap);
            if let Some(slot) = self.overflow_chunks.get_mut(&vid) {
                *slot = repacked;
            }
        }
        self.rebuild_live_set_for_vertex(vid);
        self.overflow_chunks.get(&vid).is_none_or(Vec::is_empty)
    }

    pub(crate) fn compact_overflow_for_vertex(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) {
        let Some(chunks) = self.overflow_chunks.get(&vid).cloned() else {
            return;
        };
        let old_cap: usize = chunks.iter().map(|c| c.capacity()).sum();
        let removals_enabled = cutoff != Timestamp::MAX;
        let mut kept: Vec<Nbr> = Vec::new();
        for chunk in &chunks {
            for nbr in chunk {
                if removals_enabled && is_reclaimable_slot(nbr, cutoff) {
                    on_edge_removed(nbr.edge_id, nbr.delete_ts);
                } else {
                    kept.push(*nbr);
                }
            }
        }
        if kept.is_empty() {
            // Remove empty overflow entry entirely to reclaim metadata.
            self.overflow_chunks.remove(&vid);
            self.sub_capacity(old_cap);
            self.rebuild_live_set_for_vertex(vid);
            return;
        }
        // Repack kept entries (live plus pinned tombstones) into fresh
        // graded chunks. Grading stays by live width so tiers match the
        // steady-state layout.
        let live_kept = kept
            .iter()
            .filter(|nbr| nbr.delete_ts == Timestamp::MAX)
            .count();
        let chunk_edges = self.effective_chunk_edges(live_kept.max(1));
        let mut new_chunks: Vec<Vec<Nbr>> = Vec::new();
        for chunk in kept.chunks(chunk_edges) {
            let mut v = Vec::with_capacity(chunk_edges);
            v.extend_from_slice(chunk);
            new_chunks.push(v);
        }
        // Update capacity accounting: old capacity vs new.
        let new_cap: usize = new_chunks.iter().map(|c| c.capacity()).sum();
        self.sub_capacity(old_cap);
        self.add_capacity(new_cap);
        if let Some(slot) = self.overflow_chunks.get_mut(&vid) {
            *slot = new_chunks;
        }
        self.rebuild_live_set_for_vertex(vid);
    }
}
