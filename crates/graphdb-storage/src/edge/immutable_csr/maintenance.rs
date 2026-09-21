use super::super::csr_shared::is_reclaimable_cold;
use super::super::{EdgeId, Timestamp};
use super::ImmutableCsr;

impl ImmutableCsr {
    /// Reclaim one row in place, leaving every other row untouched.
    ///
    /// Eligible tombstones of `vid` are compacted out of the row window with
    /// a write-pointer pass, then the vacated tail range is drained once so
    /// later rows slide forward; their offsets shift by the removed count.
    /// No entry outside the row window is copied and no second table-sized
    /// buffer is allocated, so the work stays proportional to the row degree
    /// plus the trailing offset fixup. Removed entries are reported in row
    /// order; live entries keep their sorted order.
    pub fn compact_row(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let idx = vid as usize;
        if idx >= self.degrees.len() {
            return 0;
        }
        if self.reclaimable_count(vid, cutoff) == 0 {
            return 0;
        }
        let Some((start, end)) = self.row_window(vid) else {
            return 0;
        };
        let mut write = start;
        for read in start..end {
            let hot = self.hot_entries[read];
            let cold = self.cold_entries[read];
            if is_reclaimable_cold(&cold, cutoff) {
                on_edge_removed(hot.edge_id, cold.delete_ts);
            } else {
                if write != read {
                    self.hot_entries[write] = hot;
                    self.cold_entries[write] = cold;
                }
                write += 1;
            }
        }
        let removed = end - write;
        self.hot_entries.drain(write..end);
        self.cold_entries.drain(write..end);
        self.degrees[idx] = (write - start) as u32;
        let shift = removed as u32;
        for off in self.offsets.iter_mut().skip(idx + 1) {
            *off -= shift;
        }
        removed
    }

    /// Drop GC-eligible tombstones in place without unfreezing.
    ///
    /// Rows stay sorted because filtering preserves order; degrees, offsets
    /// and the packed halves are rebuilt tight with no reserved gaps. Live
    /// entries are untouched, so the live edge count does not change. Each
    /// dropped entry is reported through `on_edge_removed` for tombstone
    /// accounting above this layer.
    pub fn compact_with_cutoff(
        &mut self,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let rows = self.degrees.len();
        let mut removed = 0usize;
        let mut kept_hot = Vec::with_capacity(self.hot_entries.len());
        let mut kept_cold = Vec::with_capacity(self.cold_entries.len());
        let mut new_degrees = Vec::with_capacity(rows);
        for vid in 0..rows {
            let start = self.offsets[vid] as usize;
            let degree = self.degrees[vid] as usize;
            let end = start.saturating_add(degree).min(self.hot_entries.len());
            let mut kept = 0usize;
            for idx in start..end {
                let cold = self.cold_entries[idx];
                if is_reclaimable_cold(&cold, cutoff) {
                    on_edge_removed(self.hot_entries[idx].edge_id, cold.delete_ts);
                    removed += 1;
                } else {
                    kept_hot.push(self.hot_entries[idx]);
                    kept_cold.push(cold);
                    kept += 1;
                }
            }
            new_degrees.push(kept as u32);
        }
        self.hot_entries = kept_hot;
        self.cold_entries = kept_cold;
        self.degrees = new_degrees;
        self.rebuild_offsets();
        removed
    }
}
