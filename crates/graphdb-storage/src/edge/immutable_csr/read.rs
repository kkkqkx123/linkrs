use super::super::csr_shared::{decode_endpoint_pair, is_reclaimable_cold};
use super::super::{EdgeId, EdgePosition, HotNbr, Nbr, Timestamp, VertexId, INVALID_EDGE_ID};
use super::pack::frozen_key_range;
use super::ImmutableCsr;

impl ImmutableCsr {
    pub(crate) fn row_window(&self, src_vid: u32) -> Option<(usize, usize)> {
        let idx = src_vid as usize;
        if idx >= self.degrees.len() {
            return None;
        }
        let start = self.offsets[idx] as usize;
        let degree = self.degrees[idx] as usize;
        if start.saturating_add(degree) > self.hot_entries.len() {
            return None;
        }
        Some((start, start + degree))
    }

    /// Assembled slot copy at a packed index.
    #[inline]
    pub(crate) fn slot_at(&self, idx: usize) -> Option<Nbr> {
        Some(Nbr::from_parts(
            *self.hot_entries.get(idx)?,
            *self.cold_entries.get(idx)?,
        ))
    }

    /// Row length of one vertex. Out-of-range rows report zero.
    pub fn row_degree(&self, src_vid: u32) -> usize {
        let idx = src_vid as usize;
        if idx >= self.degrees.len() {
            0
        } else {
            self.degrees[idx] as usize
        }
    }

    /// Timestamp-filtered read of one row.
    ///
    /// Test and offline use; production traversals use the row iterator or
    /// caller-buffer fill paths instead of this allocating accessor.
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let Some((start, end)) = self.row_window(src_vid) else {
            return Vec::new();
        };
        self.hot_entries[start..end]
            .iter()
            .zip(&self.cold_entries[start..end])
            .filter_map(|(hot, cold)| {
                let nbr = Nbr::from_parts(*hot, *cold);
                nbr.is_alive_at(ts).then_some(nbr)
            })
            .collect()
    }

    /// First timestamp-visible entry matching an endpoint key.
    ///
    /// Bisects the sorted row to the `(endpoint, rank)` key range, then
    /// filters inside the range by timestamp. The row sort makes edge-id
    /// order the total version order within each key.
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        let hot = &self.hot_entries[start..end];
        let cold = &self.cold_entries[start..end];
        let (lo, hi) = frozen_key_range(hot, decoded_endpoint, decoded_rank);
        hot[lo..hi]
            .iter()
            .zip(&cold[lo..hi])
            .find_map(|(hot, cold)| {
                let nbr = Nbr::from_parts(*hot, *cold);
                nbr.is_alive_at(ts).then_some(nbr)
            })
    }

    /// First live entry matching an endpoint key without consulting snapshots.
    ///
    /// Same key-range bisection as [`Self::get_edge`]; liveness is the raw
    /// open-deletion-stamp check instead of a timestamp filter.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        let hot = &self.hot_entries[start..end];
        let cold = &self.cold_entries[start..end];
        let (lo, hi) = frozen_key_range(hot, decoded_endpoint, decoded_rank);
        hot[lo..hi]
            .iter()
            .zip(&cold[lo..hi])
            .find_map(|(hot, cold)| {
                if hot.edge_id != INVALID_EDGE_ID && cold.is_live() {
                    Some(Nbr::from_parts(*hot, *cold))
                } else {
                    None
                }
            })
    }

    /// Every physically stored entry of one row without timestamp filtering.
    ///
    /// Test and offline use; production scans use `fill_physical_into` or
    /// the row iterator instead of this allocating accessor.
    pub fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src_vid, &mut out);
        out
    }

    /// Fill a caller buffer with every physically stored entry of one row.
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        out.reserve(end - start);
        for (hot, cold) in self.hot_entries[start..end]
            .iter()
            .zip(&self.cold_entries[start..end])
        {
            out.push(Nbr::from_parts(*hot, *cold));
        }
    }

    /// Visit every physically stored entry of one row without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        for (hot, cold) in self.hot_entries[start..end]
            .iter()
            .zip(&self.cold_entries[start..end])
        {
            if !f(Nbr::from_parts(*hot, *cold)) {
                return;
            }
        }
    }

    /// Visit every physically stored hot half of one row without allocating
    /// and without touching the stamp lines.
    ///
    /// Hot-only counterpart of [`Self::visit_physical`] for traversals that
    /// resolve visibility through the version authority by `edge_id`.
    pub fn visit_hot<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        for hot in &self.hot_entries[start..end] {
            if !f(*hot) {
                return;
            }
        }
    }

    /// Packed rows are always sorted by `(endpoint, rank, edge_id)`.
    pub fn is_row_sorted(&self, _src_vid: u32) -> bool {
        true
    }

    /// Visit live entries whose `(endpoint, rank)` key falls in the inclusive
    /// `[lower, upper]` range. The packed row is sorted, so the window is
    /// bisected and only the window is scanned for liveness.
    pub fn visit_threshold<F>(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        mut f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        let hot = &self.hot_entries[start..end];
        let cold = &self.cold_entries[start..end];
        let lo = match lower {
            Some((lo_ep, lo_rank)) => {
                hot.partition_point(|h| (h.endpoint, h.rank) < (lo_ep, lo_rank))
            }
            None => 0,
        };
        let hi = match upper {
            Some((hi_ep, hi_rank)) => {
                hot.partition_point(|h| (h.endpoint, h.rank) <= (hi_ep, hi_rank))
            }
            None => hot.len(),
        };
        for (h, c) in hot[lo..hi].iter().zip(&cold[lo..hi]) {
            if c.is_live() && !f(Nbr::from_parts(*h, *c)) {
                return;
            }
        }
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }

    /// Read-only view of one packed slot without mutating state.
    pub fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        if offset < 0 {
            return None;
        }
        let (start, end) = self.row_window(src_vid)?;
        if start.saturating_add(offset as usize) >= end {
            return None;
        }
        self.slot_at(start + offset as usize)
    }

    /// Whether one row holds any physically stored entry.
    pub fn has_physical_entries(&self, vid: u32) -> bool {
        self.row_degree(vid) > 0
    }

    /// Whether one row holds `edge_id`.
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((start, end)) = self.row_window(src_vid) else {
            return false;
        };
        self.hot_entries[start..end]
            .iter()
            .any(|hot| hot.edge_id == edge_id)
    }

    /// Locate the first entry with `edge_id`, returning its packed slot.
    pub fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let (start, end) = self.row_window(src_vid)?;
        for (slot, (hot, cold)) in self.hot_entries[start..end]
            .iter()
            .zip(&self.cold_entries[start..end])
            .enumerate()
        {
            if hot.edge_id == edge_id {
                return Some((
                    EdgePosition::Primary { slot: slot as u32 },
                    Nbr::from_parts(*hot, *cold),
                ));
            }
        }
        None
    }

    /// Physical entry census of one row: `(live, dead, capacity)`.
    ///
    /// Same live/dead predicates as the mutable census; capacity equals the
    /// row length because frozen rows carry no reserved gaps.
    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let Some((start, end)) = self.row_window(vid) else {
            return (0, 0, 0);
        };
        let mut live = 0usize;
        let mut dead = 0usize;
        for cold in &self.cold_entries[start..end] {
            if cold.is_live() {
                live += 1;
            } else {
                dead += 1;
            }
        }
        (live, dead, end - start)
    }

    /// Count entries of one row reclaimable at `cutoff`.
    pub fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        if cutoff == Timestamp::MAX {
            return 0;
        }
        let Some((start, end)) = self.row_window(vid) else {
            return 0;
        };
        self.cold_entries[start..end]
            .iter()
            .filter(|cold| is_reclaimable_cold(cold, cutoff))
            .count()
    }
}
