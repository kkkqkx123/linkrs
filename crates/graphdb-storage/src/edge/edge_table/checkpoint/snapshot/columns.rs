use super::format::{read_u32_le_at, read_u64_le_at, ColumnRange};
use super::MappedFrozen;
use crate::edge::{ColdStamps, HotNbr, Nbr};
use graphdb_core::types::{EdgeId, Timestamp};

impl MappedFrozen {
    #[inline]
    pub(crate) fn degree_at(&self, row: usize) -> u32 {
        // Validated at open: degree column covers every row.
        read_u32_le_at(&self.map, self.columns.degrees.start + row * 4)
            .expect("snapshot degree column validated at open")
    }

    /// Raw little-endian bytes of one snapshot column.
    ///
    /// Column ranges are validated at open, so the slice always covers the
    /// column. Row scans take these slices once per row and decode the row
    /// window with chunk iteration instead of paying one bounds-checked
    /// scalar read per slot.
    #[inline]
    pub(crate) fn column_bytes(&self, range: ColumnRange) -> &[u8] {
        self.map
            .get(range.start..range.end())
            .expect("snapshot column range validated at open")
    }

    /// Raw bytes of one entry column restricted to a validated row window.
    #[inline]
    pub(crate) fn row_column_bytes(
        &self,
        range: ColumnRange,
        width: usize,
        start: usize,
        end: usize,
    ) -> &[u8] {
        self.column_bytes(range)
            .get(start * width..end * width)
            .expect("snapshot row window validated against column ranges")
    }

    /// Decode the hot halves of a validated row window as one chunked pass.
    ///
    /// Every column slice holds exactly `end - start` entries by the width
    /// contract of [`Self::row_column_bytes`], so the zips cannot truncate.
    pub(crate) fn row_hot_slots(
        &self,
        start: usize,
        end: usize,
    ) -> impl Iterator<Item = HotNbr> + '_ {
        let endpoints = self
            .row_column_bytes(self.columns.endpoints, 4, start, end)
            .as_chunks::<4>()
            .0;
        let ranks = self
            .row_column_bytes(self.columns.ranks, 8, start, end)
            .as_chunks::<8>()
            .0;
        let edge_ids = self
            .row_column_bytes(self.columns.edge_ids, 8, start, end)
            .as_chunks::<8>()
            .0;
        endpoints
            .iter()
            .zip(ranks)
            .zip(edge_ids)
            .map(|((e, r), id)| HotNbr {
                endpoint: u32::from_le_bytes(*e),
                rank: i64::from_le_bytes(*r),
                edge_id: EdgeId(u64::from_le_bytes(*id)),
            })
    }

    /// Decode every slot of a validated row window as one chunked pass.
    pub(crate) fn row_slots(&self, start: usize, end: usize) -> impl Iterator<Item = Nbr> + '_ {
        let deletes = self
            .row_column_bytes(self.columns.deletes, 8, start, end)
            .as_chunks::<8>()
            .0;
        self.row_hot_slots(start, end)
            .zip(deletes)
            .map(|(hot, d)| Nbr {
                endpoint: hot.endpoint,
                rank: hot.rank,
                edge_id: hot.edge_id,
                delete_ts: u64::from_le_bytes(*d),
            })
    }

    /// Fill a caller buffer with every hot half of one row.
    ///
    /// Hot-only counterpart of `fill_physical_into`: topology columns are
    /// sliced once per row and decoded in tight chunk loops, so the stamp
    /// columns stay out of cache on this walk.
    pub fn fill_hot_into(&self, src_vid: u32, out: &mut Vec<HotNbr>) {
        out.clear();
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        out.reserve(end - start);
        out.extend(self.row_hot_slots(start, end));
    }

    /// Fill a caller buffer with every cold half of one row.
    ///
    /// Stamp-only counterpart of `fill_physical_into` for maintenance walks
    /// that never need topology.
    pub fn fill_cold_into(&self, src_vid: u32, out: &mut Vec<ColdStamps>) {
        out.clear();
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        if start == end {
            return;
        }
        let deletes = self.row_column_bytes(self.columns.deletes, 8, start, end);
        out.reserve(end - start);
        for delete in deletes.as_chunks::<8>().0 {
            out.push(ColdStamps {
                delete_ts: u64::from_le_bytes(*delete),
            });
        }
    }

    #[inline]
    pub(crate) fn endpoint_at(&self, idx: usize) -> u32 {
        read_u32_le_at(&self.map, self.columns.endpoints.start + idx * 4)
            .expect("snapshot endpoint column validated at open")
    }

    #[inline]
    pub(crate) fn rank_at(&self, idx: usize) -> i64 {
        read_u64_le_at(&self.map, self.columns.ranks.start + idx * 8)
            .expect("snapshot rank column validated at open") as i64
    }

    #[inline]
    pub(crate) fn edge_id_at(&self, idx: usize) -> EdgeId {
        EdgeId(
            read_u64_le_at(&self.map, self.columns.edge_ids.start + idx * 8)
                .expect("snapshot edge-id column validated at open"),
        )
    }

    #[inline]
    pub(crate) fn delete_at(&self, idx: usize) -> Timestamp {
        read_u64_le_at(&self.map, self.columns.deletes.start + idx * 8)
            .expect("snapshot delete column validated at open")
    }

    /// Whether this snapshot carries a bundled inline value column.
    ///
    /// Only sidecars written from valued frozen groups do; the value
    /// readers below return inert defaults otherwise.
    pub fn has_valued_entries(&self) -> bool {
        self.columns.values.len > 0
    }

    /// Raw inline value word at a packed index.
    ///
    /// Zero for sidecars without a value column; callers pair it with
    /// `valid_at` before interpreting it.
    #[inline]
    pub(crate) fn value_at(&self, idx: usize) -> u64 {
        if self.columns.values.len == 0 {
            return 0;
        }
        read_u64_le_at(&self.map, self.columns.values.start + idx * 8)
            .expect("snapshot value column validated at open")
    }

    /// Validity bit at a packed index.
    ///
    /// False for sidecars without a value column.
    #[inline]
    pub(crate) fn valid_at(&self, idx: usize) -> bool {
        if self.columns.validity.len == 0 {
            return false;
        }
        let at = self.columns.validity.start + idx / 8;
        let byte = self
            .map
            .get(at..at + 1)
            .expect("snapshot validity column validated at open")[0];
        byte & (1 << (idx % 8)) != 0
    }

    /// Hot half at a packed index, decoded on demand from the mapping.
    #[inline]
    pub fn hot_at(&self, idx: usize) -> Option<HotNbr> {
        if idx >= self.entries {
            return None;
        }
        Some(HotNbr {
            endpoint: self.endpoint_at(idx),
            rank: self.rank_at(idx),
            edge_id: self.edge_id_at(idx),
        })
    }

    /// Cold half at a packed index, decoded on demand from the mapping.
    #[inline]
    pub fn cold_at(&self, idx: usize) -> Option<ColdStamps> {
        if idx >= self.entries {
            return None;
        }
        Some(ColdStamps {
            delete_ts: self.delete_at(idx),
        })
    }

    /// Assembled slot copy at a packed index.
    #[inline]
    pub fn slot_at(&self, idx: usize) -> Option<Nbr> {
        Some(Nbr::from_parts(self.hot_at(idx)?, self.cold_at(idx)?))
    }

    pub(crate) fn row_window(&self, src_vid: u32) -> Option<(usize, usize)> {
        let idx = src_vid as usize;
        if idx >= self.rows {
            return None;
        }
        let start = self.offsets[idx] as usize;
        let degree = self.degree_at(idx) as usize;
        if start.saturating_add(degree) > self.entries {
            return None;
        }
        Some((start, start + degree))
    }

    /// `(endpoint, rank)` key range inside one mapped row, mirroring the
    /// frozen bisection over heap slices.
    pub(crate) fn key_range(
        &self,
        start: usize,
        end: usize,
        endpoint: u32,
        rank: i64,
    ) -> (usize, usize) {
        let key_at = |idx: usize| (self.endpoint_at(idx), self.rank_at(idx));
        let mut lo = start;
        let mut hi = end;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if key_at(mid) < (endpoint, rank) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let lower = lo;
        hi = end;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if key_at(mid) <= (endpoint, rank) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lower, lo)
    }
}
