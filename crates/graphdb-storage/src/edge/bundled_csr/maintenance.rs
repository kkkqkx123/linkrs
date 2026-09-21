use graphdb_core::types::{EdgeId, Timestamp};

use super::super::pure_csr::PureOverflowChunk;
use super::{BundledCsr, BundledOverflowValues, INVALID_EDGE_ID};

impl BundledCsr {
    /// Sort one primary row into `(endpoint, edge_id)` order with values.
    ///
    /// Same maintenance-only invalidation as the pure form; the value
    /// columns permute with the topology slots so no column drifts.
    /// Overflow chunks stay in insertion order as the unsorted suffix.
    /// The topology order flag follows the same placement as the pure
    /// sort, since this path permutes the primary window directly.
    pub fn sort_row(&mut self, vid: u32) -> bool {
        let idx = vid as usize;
        if idx >= self.topology.vertex_capacity() {
            self.topology.mark_primary_sorted(idx);
            return false;
        }
        self.sync_primary_len();
        let (start, end) = self.topology.primary_window(idx);
        if end.saturating_sub(start) <= 1 {
            self.topology.mark_primary_sorted(idx);
            return false;
        }
        let mut live: Vec<(u32, u64, u64, bool)> = Vec::new();
        for i in start..end {
            let eid = self.topology.edge_ids[i];
            if eid != INVALID_EDGE_ID.0 {
                live.push((
                    self.topology.endpoints[i],
                    eid,
                    self.primary_values[i],
                    self.primary_valid[i],
                ));
            }
        }
        if live.len() <= 1 {
            self.topology.mark_primary_sorted(idx);
            return false;
        }
        let mut sorted = live.clone();
        sorted.sort_by_key(|a| (a.0, a.1));
        if sorted
            .iter()
            .map(|(e, id, _, _)| (*e, *id))
            .eq(live.iter().map(|(e, id, _, _)| (*e, *id)))
        {
            self.topology.mark_primary_sorted(idx);
            return false;
        }
        for (offset, (endpoint, eid, value, valid)) in sorted.iter().enumerate() {
            self.topology.endpoints[start + offset] = *endpoint;
            self.topology.edge_ids[start + offset] = *eid;
            self.primary_values[start + offset] = *value;
            self.primary_valid[start + offset] = *valid;
        }
        for i in start + sorted.len()..end {
            self.topology.endpoints[i] = u32::MAX;
            self.topology.edge_ids[i] = INVALID_EDGE_ID.0;
            self.primary_values[i] = 0;
            self.primary_valid[i] = false;
        }
        self.topology.mark_primary_sorted(idx);
        self.topology.rebuild_live_set_for_vertex(vid);
        true
    }

    /// Sort every primary row that is out of order. Returns reordered rows.
    pub fn sort_all_rows(&mut self) -> usize {
        let rows = self.topology.vertex_capacity();
        let mut reordered = 0usize;
        for vid in 0..rows {
            if self.sort_row(vid as u32) {
                reordered += 1;
            }
        }
        reordered
    }

    /// Compact one vertex, moving the value column with the topology.
    pub(crate) fn compact_vertex_shifted(
        &mut self,
        vid: u32,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        let idx = vid as usize;
        if idx >= self.topology.vertex_capacity() {
            return 0;
        }
        let mut removed = 0usize;
        self.sync_primary_len();
        self.sync_overflow_row(vid);
        let base = self.topology.rows.adj_offsets[idx] as usize;
        let degree = self.topology.rows.degrees[idx] as usize;
        let mut keep = 0usize;
        for i in 0..degree {
            let slot = base + i;
            if self.topology.edge_ids[slot] == INVALID_EDGE_ID.0 {
                // Holes carry no edge identity: drop them (and their stale
                // words) silently instead of reporting the sentinel upward.
                removed += 1;
            } else {
                if keep != i {
                    self.topology.endpoints[base + keep] = self.topology.endpoints[slot];
                    self.topology.edge_ids[base + keep] = self.topology.edge_ids[slot];
                    self.primary_values[base + keep] = self.primary_values[slot];
                    self.primary_valid[base + keep] = self.primary_valid[slot];
                }
                keep += 1;
            }
        }
        self.topology.rows.degrees[idx] = keep as u32;
        self.topology.rows.primary_capacities[idx] = keep as u32;
        if self.topology.overflow_chunks.get(vid).is_some() {
            let chunks = self
                .topology
                .overflow_chunks
                .remove(vid)
                .unwrap_or_default();
            let value_chunks = self.overflow_values.take(vid).unwrap_or_default();
            let freed: usize = chunks.iter().map(|chunk| chunk.capacity()).sum();
            self.topology.sub_capacity(freed);
            let mut kept_ep: Vec<u32> = Vec::new();
            let mut kept_eid: Vec<u64> = Vec::new();
            let mut kept_val: Vec<u64> = Vec::new();
            let mut kept_ok: Vec<bool> = Vec::new();
            for (chunk_no, chunk) in chunks.iter().enumerate() {
                let values = value_chunks.get(chunk_no);
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid == INVALID_EDGE_ID.0 {
                        removed += 1;
                    } else {
                        kept_ep.push(chunk.endpoints[i]);
                        kept_eid.push(eid);
                        kept_val.push(values.and_then(|v| v.values.get(i)).copied().unwrap_or(0));
                        kept_ok.push(
                            values
                                .and_then(|v| v.valid.get(i))
                                .copied()
                                .unwrap_or(false),
                        );
                    }
                }
            }
            if !kept_ep.is_empty() {
                let single = PureOverflowChunk::consolidated(&kept_ep, &kept_eid);
                let added = single.capacity();
                self.topology.add_capacity(added);
                self.topology.overflow_chunks.insert(vid, vec![single]);
                self.overflow_values
                    .ensure_capacity(self.topology.vertex_capacity());
                let slot = self.overflow_values.slot_mut(vid);
                *slot = Some(vec![BundledOverflowValues::consolidated(
                    &kept_val, &kept_ok,
                )]);
            }
            self.topology.rebuild_live_set_for_vertex(vid);
        } else if removed > 0 {
            self.topology.rebuild_live_set_for_vertex(vid);
        }
        removed
    }
}
