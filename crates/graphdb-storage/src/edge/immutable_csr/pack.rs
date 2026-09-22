use graphdb_core::types::Timestamp;
use graphdb_core::StorageError;

use super::super::{
    BundledCsr, ColdStamps, HotNbr, MutableCsr, Nbr, PureTopologyCsr, SingleMutableCsr,
    INVALID_EDGE_ID,
};
use super::ImmutableCsr;
use bitvec::vec::BitVec;

pub(crate) fn frozen_error() -> StorageError {
    StorageError::invalid_operation(
        "frozen CSR group rejects writes: unfreeze the group before writing".to_string(),
    )
}

/// Frozen row order: point-query key first, then version order, then a
/// stable total-order tiebreak. Total, so packing sorts deterministically.
pub(crate) fn frozen_row_key(nbr: &Nbr) -> (u32, i64, u64) {
    (nbr.endpoint, nbr.rank, nbr.edge_id.0)
}

/// Sort one packed row into frozen order and drop reserved-slot gap
/// sentinels, which carry no edge. Tombstones sort by the same key and stay:
/// queries filter them by timestamp inside the key range.
fn sort_packed_row(row: &mut Vec<Nbr>) {
    row.retain(|nbr| nbr.edge_id != INVALID_EDGE_ID);
    row.sort_by_key(frozen_row_key);
}

/// `(endpoint, rank)` key range inside one sorted frozen row, as
/// `(start, end)` offsets relative to the row slice. Empty when the row
/// holds no entry with the key.
pub(crate) fn frozen_key_range(hot: &[HotNbr], endpoint: u32, rank: i64) -> (usize, usize) {
    let lo = hot.partition_point(|h| (h.endpoint, h.rank) < (endpoint, rank));
    let hi = hot.partition_point(|h| (h.endpoint, h.rank) <= (endpoint, rank));
    (lo, hi)
}

impl ImmutableCsr {
    /// Empty table with no rows.
    pub fn new() -> Self {
        Self {
            hot_entries: Vec::new(),
            cold_entries: Vec::new(),
            degrees: Vec::new(),
            offsets: Vec::new(),
            edge_count: 0,
            values: Vec::new(),
            valid: BitVec::new(),
        }
    }

    /// Drop every entry, keeping no rows.
    pub fn clear(&mut self) {
        self.hot_entries.clear();
        self.cold_entries.clear();
        self.degrees.clear();
        self.offsets.clear();
        self.edge_count = 0;
        self.values.clear();
        self.valid.clear();
    }

    /// Row count of the packed table.
    pub fn vertex_capacity(&self) -> usize {
        self.degrees.len()
    }

    /// Packed hot halves for snapshot-file writers.
    pub(crate) fn packed_hot(&self) -> &[HotNbr] {
        &self.hot_entries
    }

    /// Packed cold halves for snapshot-file writers.
    pub(crate) fn packed_cold(&self) -> &[ColdStamps] {
        &self.cold_entries
    }

    /// Packed row degrees for snapshot-file writers.
    pub(crate) fn packed_degrees(&self) -> &[u32] {
        &self.degrees
    }

    /// Live edge count of the packed table.
    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    /// Pack every physical entry of a mutable CSR, primary rows and overflow
    /// chains merged per row, then sorted into frozen row order.
    ///
    /// Content-preserving up to logical equivalence: tombstones travel
    /// verbatim, reserved-slot gap sentinels are dropped, and rows are sorted
    /// by `(endpoint, rank, edge_id)`. Timestamp-filtered reads
    /// observe the same logical entries as the source table.
    pub fn pack_from_mutable(csr: &MutableCsr) -> Self {
        Self::pack_from_rows(
            csr.vertex_capacity(),
            csr.edge_count() as usize,
            |local, row| {
                csr.fill_physical_into(local, row);
            },
        )
    }

    /// Pack every physical entry of a single-edge CSR, sorted the same way.
    ///
    /// Rows hold at most one entry; the packed form is uniform with the
    /// multi-edge pack so one frozen type serves both strategies.
    pub fn pack_single_from(csr: &SingleMutableCsr) -> Self {
        Self::pack_from_rows(
            csr.vertex_capacity(),
            csr.edge_count() as usize,
            |local, row| {
                csr.fill_physical_into(local, row);
            },
        )
    }

    /// Pack a pure-topology group directly into the frozen layout.
    ///
    /// Same sorted content as routing through a temporary mutable table,
    /// without the full copy and without fabricating timestamp replicas.
    /// Pure rows carry no timestamps and no tombstones: every physically
    /// stored entry is live, so the pack walks the topology only.
    pub fn pack_from_pure(csr: &PureTopologyCsr) -> Self {
        Self::pack_from_rows(
            csr.vertex_capacity(),
            csr.edge_count() as usize,
            |local, row| {
                row.clear();
                csr.visit_physical(local, |nbr| {
                    row.push(nbr);
                    true
                });
            },
        )
    }

    /// Pack a bundled group together with its inline values.
    ///
    /// Same sorted content as the topology-only pack, with the value column
    /// carried slot-parallel in row order: a NULL slot stores a zero word
    /// with a cleared validity bit, mirroring the bundled layout. Freezing a
    /// valued bundled group therefore preserves its properties; unfreezing
    /// replays them back through the regular valued insert entry.
    pub fn pack_from_bundled(csr: &BundledCsr) -> Self {
        Self::pack_valued_rows(
            csr.vertex_capacity(),
            csr.edge_count() as usize,
            |local, row, vals| {
                row.clear();
                vals.clear();
                csr.visit_physical_with_values(local, |nbr, value| {
                    row.push(nbr);
                    vals.push(value);
                    true
                });
            },
        )
    }

    /// Shared pack over a row-fill closure: extract, sort, append, count.
    ///
    /// Single packing entry so the multi-edge and single-edge packs cannot
    /// drift apart; only the row source differs.
    fn pack_from_rows(
        rows: usize,
        edge_hint: usize,
        mut fill_row: impl FnMut(u32, &mut Vec<Nbr>),
    ) -> Self {
        let mut hot_entries = Vec::with_capacity(edge_hint);
        let mut cold_entries = Vec::with_capacity(edge_hint);
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            fill_row(local as u32, &mut row_buf);
            sort_packed_row(&mut row_buf);
            degrees.push(row_buf.len() as u32);
            for nbr in &row_buf {
                if nbr.edge_id != INVALID_EDGE_ID && nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                }
                hot_entries.push(nbr.hot());
                cold_entries.push(nbr.cold());
            }
        }
        let mut packed = Self {
            hot_entries,
            cold_entries,
            degrees,
            offsets: Vec::with_capacity(rows),
            edge_count: live,
            values: Vec::new(),
            valid: BitVec::new(),
        };
        packed.rebuild_offsets();
        packed
    }

    /// Shared pack over a row-fill closure carrying inline values.
    ///
    /// Valued counterpart of [`Self::pack_from_rows`]: each row's entries
    /// sort together with their values by the same frozen key, so the value
    /// column stays slot-parallel to the packed halves after the reorder.
    /// Gap sentinels are dropped with their slots; tombstones travel with
    /// their retained words like the bundled layout.
    fn pack_valued_rows(
        rows: usize,
        edge_hint: usize,
        mut fill_row: impl FnMut(u32, &mut Vec<Nbr>, &mut Vec<Option<u64>>),
    ) -> Self {
        let mut hot_entries = Vec::with_capacity(edge_hint);
        let mut cold_entries = Vec::with_capacity(edge_hint);
        let mut values = Vec::with_capacity(edge_hint);
        let mut valid = BitVec::new();
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut val_buf = Vec::new();
        let mut pairs = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            fill_row(local as u32, &mut row_buf, &mut val_buf);
            pairs.clear();
            pairs.extend(row_buf.drain(..).zip(val_buf.drain(..)));
            pairs.retain(|(nbr, _)| nbr.edge_id != INVALID_EDGE_ID);
            pairs.sort_by_key(|(nbr, _)| frozen_row_key(nbr));
            degrees.push(pairs.len() as u32);
            for (nbr, value) in &pairs {
                if nbr.edge_id != INVALID_EDGE_ID && nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                }
                hot_entries.push(nbr.hot());
                cold_entries.push(nbr.cold());
                values.push(value.unwrap_or(0));
                valid.push(value.is_some());
            }
        }
        let mut packed = Self {
            hot_entries,
            cold_entries,
            degrees,
            offsets: Vec::with_capacity(rows),
            edge_count: live,
            values,
            valid,
        };
        packed.rebuild_offsets();
        packed
    }

    pub(crate) fn rebuild_offsets(&mut self) {
        self.offsets.clear();
        self.offsets.reserve(self.degrees.len());
        let mut base = 0u32;
        for degree in &self.degrees {
            self.offsets.push(base);
            base = base.saturating_add(*degree);
        }
    }
}
