use super::MutableCsr;
use super::overflow::OverflowStorage;
use super::row::PACKED_CSR_DENSITY;
use super::super::csr_shared::is_reclaimable_slot;
use super::super::{EdgeId, Nbr, Timestamp};

impl MutableCsr {
    /// Compact with per-edge removal reporting (`on_edge_removed` receives
    /// each dropped edge id and delete timestamp).
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        // Without an active snapshot cutoff no deletion may be dropped.
        let removals_enabled = cutoff < Timestamp::MAX;

        // Compact individual vertex data (primary + overflow)
        // and compute new layout.
        let mut new_offsets = Vec::with_capacity(self.vertex_capacity());
        let mut new_degrees = Vec::with_capacity(self.vertex_capacity());
        let mut new_capacities = Vec::with_capacity(self.vertex_capacity());
        let mut new_edges = Vec::<Nbr>::new();
        let mut removed_count = 0usize;

        for vid in 0..self.vertex_capacity() {
            let start = self.adj_offsets[vid] as usize;
            let degree = self.degrees[vid] as usize;

            new_offsets.push(new_edges.len());

            // Collect active edges from primary (not deleted)
            for i in 0..degree {
                let nbr = &self.nbr_list[start + i];
                if removals_enabled && is_reclaimable_slot(nbr, cutoff) {
                    on_edge_removed(nbr.edge_id, nbr.delete_ts);
                    removed_count += 1;
                } else {
                    new_edges.push(*nbr);
                }
            }

            // Collect active edges from overflow
            if let Some(chunks) = self.overflow_chunks.get(&(vid as u32)) {
                for chunk in chunks {
                    for nbr in chunk {
                        if removals_enabled && is_reclaimable_slot(nbr, cutoff) {
                            on_edge_removed(nbr.edge_id, nbr.delete_ts);
                            removed_count += 1;
                        } else {
                            new_edges.push(*nbr);
                        }
                    }
                }
            }

            let valid = new_edges.len() - new_offsets[vid];
            new_degrees.push(valid as u32);
            // Guard against reserve_ratio >= 1.0 (division by zero would yield
            // infinity, saturating the cast to u32::MAX and exploding the
            // rebuilt CSR allocation). Treat it as "no reserve". At the
            // packed density target rows size through the shared helper so
            // rebuild gaps match steady-state write gaps.
            let new_cap = if valid > 0 {
                if reserve_ratio < 1.0 {
                    let density = 1.0 - reserve_ratio;
                    if (density - PACKED_CSR_DENSITY).abs() < 1e-6 {
                        Self::sized_row_capacity(valid) as u32
                    } else {
                        ((valid as f32 / density).ceil() as u32).max(1)
                    }
                } else {
                    (valid as u32).max(1)
                }
            } else {
                0
            };
            new_capacities.push(new_cap);
        }

        // Rebuild nbr_list as flat CSR (no overflow)
        let new_total_edge_capacity: usize = new_capacities.iter().map(|&c| c as usize).sum();
        let mut new_nbr_list = Vec::with_capacity(new_total_edge_capacity);
        let mut final_offsets = Vec::with_capacity(self.vertex_capacity());

        for vid in 0..self.vertex_capacity() {
            final_offsets.push(new_nbr_list.len() as u32);
            let off = new_offsets[vid];
            let deg = new_degrees[vid] as usize;
            let cap = new_capacities[vid] as usize;

            new_nbr_list.extend_from_slice(&new_edges[off..off + deg]);
            // Fill remaining capacity with empty Nbr
            let remaining = cap - deg;
            if remaining > 0 {
                new_nbr_list.resize(new_nbr_list.len() + remaining, Nbr::new(0, 0, EdgeId(0)));
            }
        }

        self.nbr_list = new_nbr_list;
        self.adj_offsets = final_offsets;
        self.degrees = new_degrees;
        self.primary_capacities = new_capacities;
        self.total_edge_capacity = new_total_edge_capacity;

        self.overflow_chunks = OverflowStorage::new();
        self.live_sets.clear();
        self.rebuild_live_sets();

        removed_count
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
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if is_reclaimable_slot(nbr, cutoff) {
                    count += 1;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if is_reclaimable_slot(nbr, cutoff) {
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
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                } else {
                    dead += 1;
                }
            }
        }
        let mut capacity = self.primary_capacities[idx] as usize;
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            capacity += chunks.iter().map(Vec::capacity).sum::<usize>();
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
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
        let mut removed = 0usize;

        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let drop = self
                .nbr_list
                .get(offset + i)
                .is_some_and(|nbr| is_reclaimable_slot(nbr, cutoff));
            if drop {
                let nbr = self.nbr_list[offset + i];
                on_edge_removed(nbr.edge_id, nbr.delete_ts);
                removed += 1;
            } else {
                if keep != i {
                    self.nbr_list[offset + keep] = self.nbr_list[offset + i];
                }
                keep += 1;
            }
        }
        self.degrees[idx] = keep as u32;

        if self.overflow_chunks.get(&vid).is_some() {
            let chunks = self.overflow_chunks.get(&vid).cloned().unwrap_or_default();
            let mut kept: Vec<Nbr> = Vec::new();
            for chunk in &chunks {
                for nbr in chunk {
                    if is_reclaimable_slot(nbr, cutoff) {
                        on_edge_removed(nbr.edge_id, nbr.delete_ts);
                        removed += 1;
                    } else {
                        kept.push(*nbr);
                    }
                }
            }
            if kept.is_empty() {
                let freed: usize = chunks.iter().map(|c| c.capacity()).sum();
                self.overflow_chunks.remove(&vid);
                self.total_edge_capacity = self.total_edge_capacity.saturating_sub(freed);
            } else {
                let chunk_edges = self.effective_chunk_edges(keep);
                let mut repacked: Vec<Vec<Nbr>> = Vec::new();
                for piece in kept.chunks(chunk_edges.max(1)) {
                    let mut v = Vec::with_capacity(chunk_edges.max(1));
                    v.extend_from_slice(piece);
                    repacked.push(v);
                }
                let freed: usize = chunks.iter().map(|c| c.capacity()).sum();
                let added: usize = repacked.iter().map(|c| c.capacity()).sum();
                self.total_edge_capacity = self
                    .total_edge_capacity
                    .saturating_sub(freed)
                    .saturating_add(added);
                if let Some(slot) = self.overflow_chunks.get_mut(&vid) {
                    *slot = repacked;
                }
            }
            self.rebuild_live_set_for_vertex(vid);
        } else if removed > 0 {
            self.rebuild_live_set_for_vertex(vid);
        }

        removed
    }
}
