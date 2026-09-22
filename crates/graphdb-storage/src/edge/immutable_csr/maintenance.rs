use super::super::csr_shared::is_reclaimable_cold;
use super::super::{EdgeId, Timestamp};
use super::ImmutableCsr;

impl ImmutableCsr {
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

    /// Reclaim a set of rows in one linear pass without unfreezing.
    ///
    /// Production replacement for repeated `compact_row` calls: one filter
    /// over the packed halves plus one offset rebuild reclaims any number
    /// of rows for a single table-proportional cost instead of one trailing
    /// memmove per row. Rows outside `vids` are carried verbatim; listed
    /// rows drop only cutoff-eligible tombstones and keep sorted order.
    /// An empty or maximum-cutoff request compacts nothing. Reported
    /// removals arrive in row order.
    pub fn compact_rows_batched(
        &mut self,
        vids: &[u32],
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        if cutoff == Timestamp::MAX || vids.is_empty() {
            return 0;
        }
        let rows = self.degrees.len();
        let mut wanted = vec![false; rows];
        let mut any = false;
        for vid in vids {
            let idx = *vid as usize;
            if idx < rows && !wanted[idx] {
                wanted[idx] = true;
                any = true;
            }
        }
        if !any {
            return 0;
        }
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
                if wanted[vid] && is_reclaimable_cold(&cold, cutoff) {
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
        if removed == 0 {
            return 0;
        }
        self.hot_entries = kept_hot;
        self.cold_entries = kept_cold;
        self.degrees = new_degrees;
        self.rebuild_offsets();
        removed
    }
}
