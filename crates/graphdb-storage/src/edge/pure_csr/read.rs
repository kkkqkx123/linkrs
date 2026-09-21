use graphdb_core::types::EdgeId;

use super::super::{EdgePosition, HotNbr, Nbr};
use super::iter::{PureAllIter, PureRowIter};
use super::{PureTopologyCsr, INVALID_EDGE_ID};

impl PureTopologyCsr {
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for i in start..end {
            let endpoint = self.endpoints[i];
            let edge_id = EdgeId(self.edge_ids[i]);
            if edge_id == INVALID_EDGE_ID {
                continue;
            }
            if !f(self.make_nbr(endpoint, edge_id)) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let endpoint = chunk.endpoints[i];
                    let edge_id = EdgeId(chunk.edge_ids[i]);
                    if edge_id == INVALID_EDGE_ID {
                        continue;
                    }
                    if !f(self.make_nbr(endpoint, edge_id)) {
                        return;
                    }
                }
            }
        }
    }

    pub fn visit_physical_with_position<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(EdgePosition, Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for (i, idx) in (start..end).enumerate() {
            let endpoint = self.endpoints[idx];
            let edge_id = EdgeId(self.edge_ids[idx]);
            if !f(
                EdgePosition::Primary { slot: i as u32 },
                self.make_nbr(endpoint, edge_id),
            ) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for slot_idx in 0..chunk.len() {
                    let endpoint = chunk.endpoints[slot_idx];
                    let edge_id = EdgeId(chunk.edge_ids[slot_idx]);
                    if !f(
                        EdgePosition::Overflow {
                            chunk: chunk_idx as u32,
                            slot: slot_idx as u32,
                        },
                        self.make_nbr(endpoint, edge_id),
                    ) {
                        return;
                    }
                }
            }
        }
    }

    pub fn visit_hot<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let (start, end) = self.primary_window(src_idx);
        for idx in start..end {
            if self.edge_ids[idx] == INVALID_EDGE_ID.0 {
                continue;
            }
            let h = HotNbr {
                endpoint: self.endpoints[idx],
                rank: 0,
                edge_id: EdgeId(self.edge_ids[idx]),
            };
            if !f(h) {
                return;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    if chunk.edge_ids[i] == INVALID_EDGE_ID.0 {
                        continue;
                    }
                    let h = HotNbr {
                        endpoint: chunk.endpoints[i],
                        rank: 0,
                        edge_id: EdgeId(chunk.edge_ids[i]),
                    };
                    if !f(h) {
                        return;
                    }
                }
            }
        }
    }

    /// Borrowed row walk over live entries without allocating.
    ///
    /// Skips sentinel holes inline, so callers iterate one row with no
    /// intermediate vector regardless of primary versus overflow layout.
    pub fn iter_row(&self, src_vid: u32) -> PureRowIter<'_> {
        let src_idx = src_vid as usize;
        let (start, end) = if src_idx < self.vertex_capacity() {
            self.primary_window(src_idx)
        } else {
            (0, 0)
        };
        let (primary_endpoints, primary_ids) =
            if start <= end && end <= self.endpoints.len() && end <= self.edge_ids.len() {
                (&self.endpoints[start..end], &self.edge_ids[start..end])
            } else {
                (&[][..], &[][..])
            };
        PureRowIter {
            csr: self,
            primary_endpoints,
            primary_ids,
            primary_idx: 0,
            overflow: self.overflow_chunks.get(src_vid),
            chunk_idx: 0,
            slot_idx: 0,
        }
    }

    /// Whether the primary window of one row arrives in key order.
    ///
    /// On-demand probe for planning and tests: the hot threshold path
    /// consults the cached per-row flag instead. After a maintenance sort
    /// live entries lead sorted with sentinel holes sunk to the back, so
    /// the window bisects even while overflow stays unsorted.
    pub fn is_primary_sorted(&self, src_vid: u32) -> bool {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return true;
        }
        let (start, end) = self.primary_window(src_idx);
        if end.saturating_sub(start) <= 1 {
            return true;
        }
        for i in start..end.saturating_sub(1) {
            if (self.endpoints[i], self.edge_ids[i]) > (self.endpoints[i + 1], self.edge_ids[i + 1])
            {
                return false;
            }
        }
        true
    }

    /// Whether the live endpoints of one row arrive in ascending order.
    ///
    /// Pure rows are insertion-ordered, so this usually reports false on
    /// multi-edge rows. Frozen packing sorts rows, after which the same
    /// check would report true. Query planning consults this before
    /// choosing a bisection over a linear walk.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        let mut last: Option<(u32, u64)> = None;
        for nbr in self.iter_row(src_vid) {
            let key = (nbr.endpoint, nbr.edge_id.0);
            if let Some(prev) = last {
                if key < prev {
                    return false;
                }
            }
            last = Some(key);
        }
        true
    }

    /// Visit live entries whose endpoint falls in the inclusive
    /// `[lower, upper]` range (`None` means unbounded).
    ///
    /// Sorted primary prefixes bisect to the endpoint window, then filter
    /// holes inline, and the overflow suffix always scans linearly. The
    /// prefix probe is the cached per-row order flag, so hybrid rows still
    /// bisect without paying a full sortedness walk on every query.
    pub fn visit_threshold<F>(&self, src_vid: u32, lower: Option<u32>, upper: Option<u32>, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let src_idx = src_vid as usize;
        if src_idx >= self.vertex_capacity() {
            return;
        }
        let in_range = |endpoint: u32| -> bool {
            if let Some(lo) = lower {
                if endpoint < lo {
                    return false;
                }
            }
            if let Some(hi) = upper {
                if endpoint > hi {
                    return false;
                }
            }
            true
        };
        let (start, end) = self.primary_window(src_idx);
        let endpoints = &self.endpoints[start..end];
        let edge_ids = &self.edge_ids[start..end];
        if self.primary_sorted_flag(src_idx) && endpoints.len() > 1 {
            let lo = match lower {
                Some(lo) => endpoints.partition_point(|e| *e < lo),
                None => 0,
            };
            let hi = match upper {
                Some(hi) => endpoints.partition_point(|e| *e <= hi),
                None => endpoints.len(),
            };
            for (endpoint, eid) in endpoints[lo..hi].iter().zip(&edge_ids[lo..hi]) {
                if *eid != INVALID_EDGE_ID.0 && !f(self.make_nbr(*endpoint, EdgeId(*eid))) {
                    return;
                }
            }
        } else {
            for (endpoint, eid) in endpoints.iter().zip(edge_ids.iter()) {
                if *eid != INVALID_EDGE_ID.0
                    && in_range(*endpoint)
                    && !f(self.make_nbr(*endpoint, EdgeId(*eid)))
                {
                    return;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(src_vid) {
            for chunk in chunks {
                for i in 0..chunk.len() {
                    let eid = chunk.edge_ids[i];
                    if eid != INVALID_EDGE_ID.0
                        && in_range(chunk.endpoints[i])
                        && !f(self.make_nbr(chunk.endpoints[i], EdgeId(eid)))
                    {
                        return;
                    }
                }
            }
        }
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }

    /// Borrowed walk over every live entry of the table without allocating.
    pub fn iter_all(&self) -> PureAllIter<'_> {
        let cap = self.vertex_capacity() as u32;
        let row = self.iter_row(0);
        PureAllIter {
            csr: self,
            cap,
            vid: 0,
            row,
        }
    }

    pub(crate) fn row_live_scan(&self, vid: u32, endpoint: u32) -> (bool, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (false, 0);
        }
        let mut present = false;
        let mut live = 0usize;
        let (start, end) = self.primary_window(idx);
        for i in start..end {
            if self.edge_ids[i] != INVALID_EDGE_ID.0 {
                live += 1;
                if self.endpoints[i] == endpoint {
                    present = true;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for chunk in chunks {
                for (i, &eid) in chunk.edge_ids.iter().enumerate() {
                    if eid != INVALID_EDGE_ID.0 {
                        live += 1;
                        if chunk.endpoints[i] == endpoint {
                            present = true;
                        }
                    }
                }
            }
        }
        (present, live)
    }
}
