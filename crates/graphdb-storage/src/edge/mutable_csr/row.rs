use super::super::csr_shared::is_reclaimable_cold;
use super::super::{EdgeId, Nbr, Timestamp};
use super::overflow::OverflowChunk;
use super::MutableCsr;

/// Target density for packed rows: live entries per unit of reserved row
/// capacity. Rebuilds size rows to `ceil(live / PACKED_CSR_DENSITY)` so
/// everyday writes land in row gaps before spilling to overflow.
///
/// Source: 0.8 reserves a 25% write gap per row, so steady-state inserts
/// fill gaps instead of allocating overflow chunks (covered by
/// `test_steady_state_gap_fill_before_overflow`), while keeping reserved
/// memory bounded. Recommended range 0.7..=0.9; lower wastes memory, higher
/// sheds the gap and pushes every insert into overflow. Retuning requires
/// rerunning the compaction and insert benchmarks first.
pub(crate) const PACKED_CSR_DENSITY: f32 = 0.8;
/// Minimum primary slots kept for a live row after a row rebalance, so tiny
/// rows still hold a small write gap without another allocation.
///
/// Source: 4 slots cover the common single-digit-degree vertex with room for
/// one more edge; smaller values reallocate on the very next insert, larger
/// values waste primary memory across millions of tiny rows. Fixed unless a
/// small-row allocation benchmark proves otherwise.
pub(crate) const MIN_ROW_CAPACITY: usize = 4;

/// Proportional overflow sizing, single benchmarked scheme. Chunk sizes
/// grow geometrically with live row width from a small floor, so a 5-edge
/// row reserves 8 slots instead of 256. The effective size is
/// `min(configured, graded(live))` so explicit test configurations keep
/// their exact size.
///
/// Source: the floor of 8 matches `MIN_ROW_CAPACITY` granularity and the
/// supernode bench in `benches/csr_perf_bench.rs` (graded tiers section);
/// the cap of 4096 matches the default chunk configuration so a single
/// chunk never exceeds one configured allocation unit. Geometric growth
/// keeps skewed-row appends amortized while small rows stay small.
/// Recommended floor range 4..=16, cap fixed at the configured chunk size.
/// Retuning requires rerunning the supernode append benchmark first.
pub(crate) const OVERFLOW_CHUNK_MIN: usize = 8;
pub(crate) const OVERFLOW_CHUNK_MAX: usize = 4096;

/// Overflow chunk size proportional to live row width: the next power of two
/// above `live` with a small floor and a hard cap. Monotonic in `live`, so
/// rows never shrink their chunk size as they grow.
pub(crate) fn graded_overflow_chunk_edges(live: usize) -> usize {
    live.next_power_of_two()
        .clamp(OVERFLOW_CHUNK_MIN, OVERFLOW_CHUNK_MAX)
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

    /// Sort one primary row into `(endpoint, rank, edge_id)` order.
    ///
    /// Maintenance-only entry: sorting moves slots, so every previously
    /// issued `EdgePosition` for this row becomes stale and the caller must
    /// relocate through the edge-id key first. The reuse hint is dropped and
    /// the live index is rebuilt into its sorted form. Overflow chunks stay
    /// in insertion order and remain the unsorted suffix. New writes keep
    /// the hot path (primary gap fill, then overflow tail) and therefore
    /// mark the row unsorted again as observed by `is_row_sorted`; the next
    /// maintenance pass re-sorts. No watermark or sorted flag is persisted:
    /// order is observed in memory and rebuilt on load.
    pub fn sort_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() || self.rows.primary_capacities[idx] == 0 {
            return false;
        }
        let degree = self.rows.degrees[idx] as usize;
        if degree <= 1 {
            return false;
        }
        let base = self.rows.adj_offsets[idx] as usize;
        if base.saturating_add(degree) > self.hot_list.len()
            || base.saturating_add(degree) > self.cold_list.len()
        {
            return false;
        }
        let mut order: Vec<usize> = (0..degree).collect();
        let hot = &self.hot_list[base..base + degree];
        let already = order.windows(2).all(|w| {
            let a = &hot[w[0]];
            let b = &hot[w[1]];
            (a.endpoint, a.rank, a.edge_id.0) <= (b.endpoint, b.rank, b.edge_id.0)
        });
        if already {
            return false;
        }
        order.sort_by(|&a, &b| {
            let ha = &hot[a];
            let hb = &hot[b];
            (ha.endpoint, ha.rank, ha.edge_id.0).cmp(&(hb.endpoint, hb.rank, hb.edge_id.0))
        });
        let hot_src = self.hot_list[base..base + degree].to_vec();
        let cold_src = self.cold_list[base..base + degree].to_vec();
        for (dst, src) in order.iter().enumerate() {
            self.hot_list[base + dst] = hot_src[*src];
            self.cold_list[base + dst] = cold_src[*src];
        }
        self.invalidate_reuse_hint(idx);
        self.rebuild_live_set_for_vertex(vid);
        true
    }

    /// Sort every primary row that is out of order. Returns reordered rows.
    ///
    /// Same maintenance-only invalidation as `sort_row`, applied row by row.
    pub fn sort_all_rows(&mut self) -> usize {
        let rows = self.vertex_capacity();
        let mut reordered = 0usize;
        for vid in 0..rows {
            if self.sort_row(vid as u32) {
                reordered += 1;
            }
        }
        reordered
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
        // Consolidate into one overflow chunk. Primary-tail expansion was
        // removed: merged rows stay on the single-chunk path so compaction
        // never copies primary blocks.
        let single = OverflowChunk::consolidated(&kept);
        let new_cap = single.capacity();
        self.add_capacity(new_cap);
        self.overflow_chunks.insert(vid, vec![single]);
        self.rebuild_live_set_for_vertex(vid);
    }
}
