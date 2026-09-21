use super::super::EdgePosition;
use super::live_set::PureLiveKeySet;
use super::{PureTopologyCsr, INVALID_EDGE_ID, LIVE_SET_WIDTH_BOUND};

impl PureTopologyCsr {
    /// Cached primary order of one row for threshold scans.
    ///
    /// True only when the primary window is known to arrive in key order;
    /// false always falls back to the linear scan, so a stale false costs
    /// speed but never correctness.
    pub(crate) fn primary_sorted_flag(&self, src_idx: usize) -> bool {
        self.primary_sorted[src_idx]
    }

    /// Clear the cached order after a primary write of a new key.
    ///
    /// Deletes, reverts and order-preserving moves must not call this:
    /// they keep the key order intact.
    pub(crate) fn mark_primary_unsorted(&mut self, src_idx: usize) {
        if let Some(flag) = self.primary_sorted.get_mut(src_idx) {
            *flag = false;
        }
    }

    /// Establish the cached order after the maintenance sort.
    pub(crate) fn mark_primary_sorted(&mut self, src_idx: usize) {
        if let Some(flag) = self.primary_sorted.get_mut(src_idx) {
            *flag = true;
        }
    }

    pub(crate) fn rebuild_live_sets(&mut self) {
        let capacity = self.vertex_capacity() as u32;
        self.live_sets.clear();
        self.live_sets.ensure_capacity(self.vertex_capacity());
        for vid in 0..capacity {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    /// Sort one primary row into `(endpoint, edge_id)` order.
    ///
    /// Maintenance-only entry: sorting moves slots, so every previously
    /// issued `EdgePosition` for this row becomes stale and the caller must
    /// relocate through the edge-id key first. Live entries sort to the
    /// front; sentinel holes sink to the back with a maximal endpoint so
    /// the whole window stays ordered. The cached primary order is
    /// established on every order-deciding return, and the live index is
    /// rebuilt. Overflow chunks stay in insertion order as the unsorted
    /// suffix. The flag is never persisted: load resets every row to
    /// unordered and the maintenance pass re-establishes order.
    pub fn sort_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.mark_primary_sorted(idx);
            return false;
        }
        let (start, end) = self.primary_window(idx);
        if end.saturating_sub(start) <= 1 {
            self.mark_primary_sorted(idx);
            return false;
        }
        let mut live: Vec<(u32, u64)> = Vec::new();
        for i in start..end {
            let eid = self.edge_ids[i];
            if eid != INVALID_EDGE_ID.0 {
                live.push((self.endpoints[i], eid));
            }
        }
        if live.len() <= 1 {
            self.mark_primary_sorted(idx);
            return false;
        }
        let mut sorted = live.clone();
        sorted.sort_unstable();
        if sorted == live {
            self.mark_primary_sorted(idx);
            return false;
        }
        for (offset, (endpoint, eid)) in sorted.iter().enumerate() {
            self.endpoints[start + offset] = *endpoint;
            self.edge_ids[start + offset] = *eid;
        }
        for i in start + sorted.len()..end {
            self.endpoints[i] = u32::MAX;
            self.edge_ids[i] = INVALID_EDGE_ID.0;
        }
        self.mark_primary_sorted(idx);
        self.rebuild_live_set_for_vertex(vid);
        true
    }

    /// Sort every primary row that is out of order. Returns reordered rows.
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

    pub(crate) fn rebuild_live_set_for_vertex(&mut self, vid: u32) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.live_sets.remove(vid);
            return;
        }
        let mut positioned = Vec::new();
        let (start, end) = self.primary_window(idx);
        for (slot, &eid) in self.edge_ids[start..end].iter().enumerate() {
            if eid != INVALID_EDGE_ID.0 {
                positioned.push((
                    self.endpoints[start + slot],
                    EdgePosition::Primary { slot: slot as u32 },
                ));
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for (slot_idx, &eid) in chunk.edge_ids.iter().enumerate() {
                    if eid != INVALID_EDGE_ID.0 {
                        positioned.push((
                            chunk.endpoints[slot_idx],
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        ));
                    }
                }
            }
        }
        if positioned.len() <= LIVE_SET_WIDTH_BOUND {
            self.live_sets.remove(vid);
        } else {
            self.live_sets
                .insert(vid, PureLiveKeySet::from_positions(positioned));
        }
    }

    pub(crate) fn track_live_insert(&mut self, vid: u32, endpoint: u32, position: EdgePosition) {
        if self.live_sets.get(vid).is_some() {
            self.live_sets.insert_key(vid, endpoint, position);
            return;
        }
        // Narrow rows stay set-free until the physical row width passes the
        // bound. The width check touches only lengths, not entries: counting
        // live entries instead would cost a full row walk per insert, so the
        // physical gate is kept deliberately. Rebuilds drop the set again
        // when the live width is still narrow, so tombstone-heavy rows may
        // rescan on later inserts until maintenance compacts them.
        let mut width = 0usize;
        if (vid as usize) < self.vertex_capacity() {
            width = self.rows.degrees[vid as usize] as usize;
            if let Some(chunks) = self.overflow_chunks.get(vid) {
                width += chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
            }
        }
        if width > LIVE_SET_WIDTH_BOUND {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    pub(crate) fn track_live_remove(&mut self, vid: u32, endpoint: u32) {
        if self.live_sets.get(vid).is_none() {
            return;
        }
        self.live_sets.remove_key(vid, &endpoint);
    }
}
