use super::super::csr_shared::is_reclaimable_cold;
use super::super::{ColdStamps, EdgeId, HotNbr, Nbr, Timestamp};
use super::overflow::{OverflowChunk, OverflowStorage};
use super::row::PACKED_CSR_DENSITY;
use super::MutableCsr;

impl MutableCsr {
    /// Compact with per-edge removal reporting (`on_edge_removed` receives
    /// each dropped edge id and delete timestamp).
    ///
    /// Two read-only passes over the old layout, one allocation for the new
    /// primary list: the first pass counts kept entries per row and sizes
    /// capacities, the second copies kept entries straight into place.
    /// No intermediate staging copy of every kept edge is built, so the
    /// transient peak is one new list instead of a staging copy plus a new
    /// list. Overflow chains are eliminated: every kept entry lands in its
    /// row's primary block.
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        // Without an active snapshot cutoff no deletion may be dropped.
        let removals_enabled = cutoff < Timestamp::MAX;
        let rows = self.vertex_capacity();

        // Pass 1 (read-only): kept count and target capacity per row.
        let mut new_degrees = Vec::with_capacity(rows);
        let mut new_capacities = Vec::with_capacity(rows);
        let mut removed_count = 0usize;
        for vid in 0..rows {
            let (hot, cold) = self.primary_pair(vid);
            let mut kept = 0usize;
            for (h, c) in hot.iter().zip(cold.iter()) {
                if removals_enabled && is_reclaimable_cold(c, cutoff) {
                    on_edge_removed(h.edge_id, c.delete_ts);
                    removed_count += 1;
                } else {
                    kept += 1;
                }
            }
            if let Some(chunks) = self.overflow_chunks.get(vid as u32) {
                for chunk in chunks {
                    for (hot, cold) in chunk.hot_slice().iter().zip(chunk.cold_slice()) {
                        if removals_enabled && is_reclaimable_cold(cold, cutoff) {
                            on_edge_removed(hot.edge_id, cold.delete_ts);
                            removed_count += 1;
                        } else {
                            kept += 1;
                        }
                    }
                }
            }
            new_degrees.push(kept as u32);
            new_capacities.push(Self::compact_row_capacity(kept, reserve_ratio) as u32);
        }

        // Pass 2: copy kept entries straight into the single new lists.
        let new_total_edge_capacity: usize = new_capacities.iter().map(|&c| c as usize).sum();
        let mut new_hot_list = Vec::with_capacity(new_total_edge_capacity);
        let mut new_cold_list = Vec::with_capacity(new_total_edge_capacity);
        let mut final_offsets = Vec::with_capacity(rows);
        for vid in 0..rows {
            final_offsets.push(new_hot_list.len() as u32);
            let (hot, cold) = self.primary_pair(vid);
            for (h, c) in hot.iter().zip(cold.iter()) {
                if removals_enabled && is_reclaimable_cold(c, cutoff) {
                    continue;
                }
                new_hot_list.push(*h);
                new_cold_list.push(*c);
            }
            if let Some(chunks) = self.overflow_chunks.get(vid as u32) {
                for chunk in chunks {
                    for (hot, cold) in chunk.hot_slice().iter().zip(chunk.cold_slice()) {
                        if removals_enabled && is_reclaimable_cold(cold, cutoff) {
                            continue;
                        }
                        new_hot_list.push(*hot);
                        new_cold_list.push(*cold);
                    }
                }
            }
            // Fill remaining capacity with the never-alive gap sentinel so
            // an overrun scan reports absence instead of a ghost live edge.
            let cap = new_capacities[vid] as usize;
            let deg = new_degrees[vid] as usize;
            debug_assert_eq!(new_hot_list.len() - final_offsets[vid] as usize, deg);
            let remaining = cap - deg;
            if remaining > 0 {
                new_hot_list.resize(new_hot_list.len() + remaining, HotNbr::dead_gap());
                new_cold_list.resize(new_cold_list.len() + remaining, ColdStamps::dead_gap());
            }
        }

        self.hot_list = new_hot_list;
        self.cold_list = new_cold_list;
        self.rows.adj_offsets = final_offsets;
        self.rows.degrees = new_degrees;
        self.rows.primary_capacities = new_capacities;
        self.total_edge_capacity = new_total_edge_capacity;

        self.overflow_chunks = OverflowStorage::new();
        self.live_sets.clear();
        self.rebuild_live_sets();
        self.reset_reuse_hints();
        self.reset_primary_sorted();

        removed_count
    }

    /// Target primary capacity for a compacted row holding `kept` entries.
    ///
    /// Shared sizing rule for full rebuilds: guard against
    /// reserve_ratio >= 1.0 (division by zero would yield infinity,
    /// saturating the cast to u32::MAX and exploding the rebuilt CSR
    /// allocation) by treating it as "no reserve". At the packed density
    /// target rows size through the shared helper so rebuild gaps match
    /// steady-state write gaps.
    pub(crate) fn compact_row_capacity(kept: usize, reserve_ratio: f32) -> usize {
        if kept == 0 {
            return 0;
        }
        if reserve_ratio < 1.0 {
            let density = 1.0 - reserve_ratio;
            if (density - PACKED_CSR_DENSITY).abs() < 1e-6 {
                return Self::sized_row_capacity(kept);
            }
            return ((kept as f32 / density).ceil() as usize).max(1);
        }
        kept.max(1)
    }

    /// Count entries of one vertex reclaimable at `cutoff`.
    ///
    /// Only tombstones eligible under the shared collection predicate
    /// count; tombstones pinned by older snapshots are left alone.
    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let mut count = 0;
        let (_, cold) = self.primary_pair(idx);
        for c in cold.iter() {
            if is_reclaimable_cold(c, cutoff) {
                count += 1;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for cold in chunk.cold_slice() {
                    if is_reclaimable_cold(cold, cutoff) {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    /// Whether one vertex holds anything reclaimable at `cutoff`.
    pub fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        self.reclaimable_count(vid, cutoff) > 0
    }

    /// Single-walk reclaim probe of one vertex: `(dead, reclaimable)`.
    ///
    /// Fuses the census and eligibility walks so maintenance passes pay one
    /// row scan instead of two. `dead` counts every tombstoned entry
    /// regardless of eligibility; `reclaimable` counts only entries the
    /// current cutoff already covers.
    pub fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (0, 0);
        }
        let eligible = cutoff != Timestamp::MAX;
        let mut dead = 0usize;
        let mut reclaimable = 0usize;
        let (_, cold) = self.primary_pair(idx);
        for c in cold.iter() {
            if !c.is_live() {
                dead += 1;
                if eligible && is_reclaimable_cold(c, cutoff) {
                    reclaimable += 1;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for cold in chunk.cold_slice() {
                    if !cold.is_live() {
                        dead += 1;
                        if eligible && is_reclaimable_cold(cold, cutoff) {
                            reclaimable += 1;
                        }
                    }
                }
            }
        }
        (dead, reclaimable)
    }

    /// Physical entry census of one vertex: `(live, dead, capacity)`.
    ///
    /// `live` counts entries with no deletion stamp, `dead` counts
    /// tombstoned entries regardless of eligibility, and `capacity` is the
    /// reserved row capacity plus allocated overflow chunks.
    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (0, 0, 0);
        }
        let mut live = 0usize;
        let mut dead = 0usize;
        let (_, cold) = self.primary_pair(idx);
        for c in cold.iter() {
            if c.is_live() {
                live += 1;
            } else {
                dead += 1;
            }
        }
        let mut capacity = self.rows.primary_capacities[idx] as usize;
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            capacity += chunks.iter().map(|chunk| chunk.capacity()).sum::<usize>();
            for chunk in chunks {
                for cold in chunk.cold_slice() {
                    if cold.is_live() {
                        live += 1;
                    } else {
                        dead += 1;
                    }
                }
            }
        }
        (live, dead, capacity)
    }

    /// Reclaim one vertex in place, leaving every other row untouched.
    ///
    /// Eligible tombstones are dropped and reported through
    /// `on_edge_removed`; live entries and pinned tombstones are tightened
    /// to the front of their current slots. Row offsets and capacities of
    /// other vertices never move, so the work stays proportional to the
    /// degree of `vid` instead of the size of the table.
    pub fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        // Probe before allocating: rows without reclaimable tombstones skip
        // the overflow clone, the kept buffer and the live-set rebuild, so
        // maintenance sweeps over clean rows stay allocation-free.
        if self.vertex_reclaim_probe(vid, cutoff).1 == 0 {
            return 0;
        }
        let mut removed = 0usize;

        let degree = self.rows.degrees[idx] as usize;
        let offset = self.rows.adj_offsets[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let drop = self
                .cold_at(offset + i)
                .is_some_and(|cold| is_reclaimable_cold(&cold, cutoff));
            if drop {
                let hot = self.hot_list[offset + i];
                let cold = self.cold_list[offset + i];
                on_edge_removed(hot.edge_id, cold.delete_ts);
                removed += 1;
            } else {
                if keep != i {
                    let kept_slot = self.slot_at(offset + i).unwrap();
                    self.set_slot(offset + keep, kept_slot);
                }
                keep += 1;
            }
        }
        self.rows.degrees[idx] = keep as u32;
        self.invalidate_reuse_hint(idx);

        if self.overflow_chunks.get(vid).is_some() {
            // Take ownership of the chunk list instead of cloning it: the
            // entry is reinserted below, so the filter runs on owned chunks
            // with no duplicate allocation.
            let chunks = self.overflow_chunks.remove(vid).unwrap_or_default();
            let freed: usize = chunks.iter().map(|chunk| chunk.capacity()).sum();
            self.sub_capacity(freed);
            let mut kept: Vec<Nbr> = Vec::new();
            for chunk in &chunks {
                for i in 0..chunk.len() {
                    let nbr = chunk.slot_at(i).unwrap();
                    if is_reclaimable_cold(&nbr.cold(), cutoff) {
                        on_edge_removed(nbr.edge_id, nbr.delete_ts);
                        removed += 1;
                    } else {
                        kept.push(nbr);
                    }
                }
            }
            if kept.is_empty() {
                self.rebuild_live_set_for_vertex(vid);
            } else {
                // Same single-block consolidation as the repack path.
                let single = OverflowChunk::consolidated(&kept);
                let added: usize = single.capacity();
                self.add_capacity(added);
                self.overflow_chunks.insert(vid, vec![single]);
                self.rebuild_live_set_for_vertex(vid);
            }
        } else if removed > 0 {
            self.rebuild_live_set_for_vertex(vid);
        }

        removed
    }
}
