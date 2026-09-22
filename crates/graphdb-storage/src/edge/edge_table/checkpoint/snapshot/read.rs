use super::MappedFrozen;
use crate::edge::csr_shared::decode_endpoint_pair;
use crate::edge::{ColdStamps, EdgePosition, HotNbr, Nbr, INVALID_EDGE_ID};
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

impl MappedFrozen {
    /// Row length of one vertex. Out-of-range rows report zero.
    pub fn row_degree(&self, src_vid: u32) -> usize {
        let idx = src_vid as usize;
        if idx >= self.rows {
            0
        } else {
            self.degree_at(idx) as usize
        }
    }

    /// Mapped rows inherit the frozen sort order.
    pub fn is_row_sorted(&self, _src_vid: u32) -> bool {
        true
    }

    /// Threshold range inside one mapped row, mirroring the heap bisection.
    fn threshold_range(
        &self,
        start: usize,
        end: usize,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
    ) -> (usize, usize) {
        let key_at = |idx: usize| (self.endpoint_at(idx), self.rank_at(idx));
        let lo = match lower {
            Some((lo_ep, lo_rank)) => {
                let mut lo = start;
                let mut hi = end;
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if key_at(mid) < (lo_ep, lo_rank) {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                lo
            }
            None => start,
        };
        let hi = match upper {
            Some((hi_ep, hi_rank)) => {
                let mut lo = lo;
                let mut hi = end;
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    if key_at(mid) <= (hi_ep, hi_rank) {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                lo
            }
            None => end,
        };
        (lo, hi)
    }

    /// Visit live entries whose `(endpoint, rank)` key falls in the inclusive
    /// `[lower, upper]` range. The mapped row is sorted, so the window is
    /// bisected before the liveness filter.
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
        if start == end {
            return;
        }
        let (lo, hi) = self.threshold_range(start, end, lower, upper);
        for idx in lo..hi {
            let hot = HotNbr {
                endpoint: self.endpoint_at(idx),
                rank: self.rank_at(idx),
                edge_id: self.edge_id_at(idx),
            };
            let cold = ColdStamps {
                delete_ts: self.delete_at(idx),
            };
            if cold.is_live() && hot.edge_id != INVALID_EDGE_ID && !f(Nbr::from_parts(hot, cold)) {
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

    /// Timestamp-filtered read of one row.
    ///
    /// Test and offline use; production traversals use `iter_edges_of` or
    /// `fill_physical_into` instead of this allocating accessor.
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_visible_into(src_vid, ts, &mut out);
        out
    }

    /// Fill a caller buffer with the timestamp-visible entries of one row.
    ///
    /// Same filtered content as `edges_of` without the per-row allocation.
    /// Columns are sliced once per row and decoded in one pass.
    pub fn fill_visible_into(&self, src_vid: u32, ts: Timestamp, out: &mut Vec<Nbr>) {
        out.clear();
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        out.reserve(end - start);
        out.extend(self.row_slots(start, end).filter(|nbr| nbr.is_alive_at(ts)));
    }

    /// First timestamp-visible entry matching an endpoint key: key-range
    /// bisection plus in-range timestamp filter, same rule as the heap form.
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        let (lo, hi) = self.key_range(start, end, decoded_endpoint, decoded_rank);
        (lo..hi).find_map(|idx| {
            let nbr = self.slot_at(idx)?;
            nbr.is_alive_at(ts).then_some(nbr)
        })
    }

    /// First live entry matching an endpoint key without consulting snapshots.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        let (lo, hi) = self.key_range(start, end, decoded_endpoint, decoded_rank);
        (lo..hi).find_map(|idx| {
            let hot = self.hot_at(idx)?;
            let cold = self.cold_at(idx)?;
            if hot.edge_id != INVALID_EDGE_ID && cold.is_live() {
                Some(Nbr::from_parts(hot, cold))
            } else {
                None
            }
        })
    }

    /// Every physically stored entry of one row without timestamp filtering.
    ///
    /// Test and offline use; production scans use `fill_physical_into` or
    /// the visitor paths instead of this allocating accessor.
    pub fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src_vid, &mut out);
        out
    }

    /// Fill a caller buffer with every physically stored entry of one row.
    ///
    /// Columns are sliced once per row and decoded in one pass instead of
    /// one bounds-checked scalar read per slot.
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        out.clear();
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        out.reserve(end - start);
        out.extend(self.row_slots(start, end));
    }

    /// Visit every physically stored entry of one row without allocating.
    ///
    /// Decodes from one slice per column instead of one bounds-checked scalar
    /// read per slot.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        for nbr in self.row_slots(start, end) {
            if !f(nbr) {
                return;
            }
        }
    }

    /// Visit every physically stored hot half of one row without allocating
    /// and without touching the stamp columns.
    ///
    /// Decodes from one slice per topology column instead of one scalar read
    /// per slot.
    pub fn visit_hot<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        for hot in self.row_hot_slots(start, end) {
            if !f(hot) {
                return;
            }
        }
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
        (start..end).any(|idx| self.edge_id_at(idx) == edge_id)
    }

    /// Read the inline value of one edge by id within its source row.
    ///
    /// Mapped half of the frozen paired traversal: resolve the edge from
    /// the topology walk first, then read its value here. Sidecars without
    /// a value column still resolve the edge but report a NULL slot.
    pub fn bundled_value_by_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        let (start, end) = self.row_window(src_vid)?;
        for idx in start..end {
            if self.edge_id_at(idx) == edge_id {
                return Some((self.value_at(idx), self.valid_at(idx)));
            }
        }
        None
    }

    /// Read the inline value of the first physical entry for one endpoint.
    ///
    /// First-match walk mirroring the heap accessor: tombstones are physical
    /// entries and may match.
    pub fn bundled_value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        let (start, end) = self.row_window(src_vid)?;
        for idx in start..end {
            if self.endpoint_at(idx) == endpoint && self.edge_id_at(idx) != INVALID_EDGE_ID {
                return Some((self.value_at(idx), self.valid_at(idx)));
            }
        }
        None
    }

    /// Visit every physically stored entry of one row with its inline value
    /// (`None` for NULL slots).
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        for idx in start..end {
            let Some(nbr) = self.slot_at(idx) else {
                continue;
            };
            let raw = self.value_at(idx);
            let value = self.valid_at(idx).then_some(raw);
            if !f(nbr, value) {
                return;
            }
        }
    }

    /// Whether any slot holds a valid inline value.
    pub fn any_valid_values(&self) -> bool {
        let bytes = self.column_bytes(self.columns.validity);
        bytes.iter().any(|b| *b != 0)
    }

    /// Locate the first entry with `edge_id`, returning its packed slot.
    pub fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let (start, end) = self.row_window(src_vid)?;
        for (slot, idx) in (start..end).enumerate() {
            if self.edge_id_at(idx) == edge_id {
                return Some((
                    EdgePosition::Primary { slot: slot as u32 },
                    self.slot_at(idx)?,
                ));
            }
        }
        None
    }

    /// Physical entry census of one row: `(live, dead, capacity)`.
    pub fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        let Some((start, end)) = self.row_window(vid) else {
            return (0, 0, 0);
        };
        let mut live = 0usize;
        let mut dead = 0usize;
        for idx in start..end {
            let cold = ColdStamps {
                delete_ts: self.delete_at(idx),
            };
            if cold.is_live() {
                live += 1;
            } else {
                dead += 1;
            }
        }
        (live, dead, end - start)
    }

    /// Mapped groups are never reclaimed; maintenance skips them like frozen.
    pub fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }
}
