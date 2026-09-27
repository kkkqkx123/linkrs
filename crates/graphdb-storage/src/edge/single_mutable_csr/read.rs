//! Read path for `SingleMutableCsr`: physical reads, threshold scans and
//! per-vertex accessors.

use super::super::csr_shared::decode_endpoint_pair;
use super::super::{EdgeId, EdgePosition, Nbr, Timestamp, VertexId, INVALID_EDGE_ID};
use super::SingleMutableCsr;

impl SingleMutableCsr {
    pub fn get_edge(&self, src: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let src_idx = src as usize;

        if src_idx >= self.vertex_capacity() {
            return None;
        }

        let (dst_ep, dst_rank) = decode_endpoint_pair(dst)?;
        let probe = self.slot_at(src_idx)?;

        if !probe.is_alive_at(ts) {
            return None;
        }

        if probe.endpoint == dst_ep && probe.rank == dst_rank {
            Some(probe)
        } else {
            None
        }
    }

    /// Locate the single slot holding `edge_id`, if any.
    ///
    /// Returns the row-local primary position alongside a copy, including
    /// tombstoned slots so delete and revert callers can act on them.
    pub fn locate_edge(&self, src: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let probe = self.slot_at(src as usize)?;
        if probe.edge_id == INVALID_EDGE_ID || probe.edge_id != edge_id {
            return None;
        }
        Some((EdgePosition::Primary { slot: 0 }, probe))
    }

    pub fn nbr_at_offset(&self, src: u32, offset: i32) -> Option<Nbr> {
        if offset != 0 {
            return None;
        }
        self.slot_at(src as usize)
    }

    pub fn get_edge_physical(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;
        if probe.edge_id == INVALID_EDGE_ID || probe.delete_ts != Timestamp::MAX {
            return None;
        }
        let (dst_ep, dst_rank) = decode_endpoint_pair(dst)?;
        if probe.endpoint == dst_ep && probe.rank == dst_rank {
            Some(probe)
        } else {
            None
        }
    }

    /// Test and offline use; production scans use `fill_physical_into` or
    /// the visitor paths instead of this allocating accessor.
    pub fn physical_edges_of(&self, src: u32) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_physical_into(src, &mut out);
        out
    }

    /// Fill a caller buffer with the physically stored entry of one slot.
    ///
    /// Same content as the allocating accessor above, without the per-vertex
    /// allocation. Lets batch scans share one buffer across vertices.
    pub fn fill_physical_into(&self, src: u32, out: &mut Vec<Nbr>) {
        out.clear();
        if let Some(probe) = self.slot_at(src as usize) {
            if probe.edge_id != INVALID_EDGE_ID {
                out.push(probe);
            }
        }
    }

    pub fn visit_physical<F>(&self, src: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        if let Some(probe) = self.slot_at(src as usize) {
            if probe.edge_id != INVALID_EDGE_ID {
                let _ = f(probe);
            }
        }
    }

    /// Visit the single hot half of one vertex without allocating.
    pub fn visit_hot<F>(&self, src: u32, mut f: F)
    where
        F: FnMut(super::super::HotNbr) -> bool,
    {
        let idx = src as usize;
        if idx >= self.vertex_capacity || !self.has_present(idx) {
            return;
        }
        let (seg, off) = Self::locate(idx);
        if let Some(segment) = self.segments.get(seg).and_then(|s| s.as_ref()) {
            let hot = segment.hot[off];
            if hot.edge_id != INVALID_EDGE_ID {
                let _ = f(hot);
            }
        }
    }

    /// Single-slot rows hold at most one entry and are trivially ordered.
    pub fn is_row_sorted(&self, _src: u32) -> bool {
        true
    }

    /// Threshold visit over at most one live entry.
    pub fn visit_threshold<F>(
        &self,
        src: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        mut f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        let Some(probe) = self.slot_at(src as usize) else {
            return;
        };
        if probe.edge_id == INVALID_EDGE_ID || probe.delete_ts != Timestamp::MAX {
            return;
        }
        if let Some((lo_ep, lo_rank)) = lower {
            if (probe.endpoint, probe.rank) < (lo_ep, lo_rank) {
                return;
            }
        }
        if let Some((hi_ep, hi_rank)) = upper {
            if (probe.endpoint, probe.rank) > (hi_ep, hi_rank) {
                return;
            }
        }
        let _ = f(probe);
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }

    pub fn has_physical_entries(&self, vid: u32) -> bool {
        self.hot_at(vid as usize)
            .is_some_and(|hot| hot.edge_id != INVALID_EDGE_ID)
    }

    pub fn primary_contains(&self, src: u32, edge_id: EdgeId) -> bool {
        self.hot_at(src as usize)
            .is_some_and(|hot| hot.edge_id == edge_id)
    }

    /// Test and offline use; production reads use `iter_edges_of` directly
    /// instead of collecting through this allocating accessor.
    pub fn edges_of(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        self.iter_edges_of(src, ts).into_iter().collect()
    }

    /// Read the single slot of one vertex when it is alive at `ts`.
    ///
    /// Allocation-free counterpart of `edges_of`: no vector is built for the
    /// one-entry row. Returns `None` for out-of-range rows and for slots
    /// that are empty or not alive at `ts`.
    pub fn iter_edges_of(&self, src: u32, ts: Timestamp) -> Option<Nbr> {
        let probe = self.slot_at(src as usize)?;
        probe.is_alive_at(ts).then_some(probe)
    }
}
