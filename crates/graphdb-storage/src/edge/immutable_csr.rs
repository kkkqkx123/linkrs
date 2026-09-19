//! Frozen immutable CSR topology.
//!
//! Packed read-only form of one group's adjacency: a single contiguous
//! neighbor segment plus a per-row degree table. Empty rows hold no slots.
//! There is no capacity array, no overflow chain, no live index and no lock:
//! a frozen group pays only its entries plus two small per-row arrays.
//!
//! A frozen group keeps every neighbor byte of the mutable group it was
//! packed from, including edge ids, both timestamps and tombstones, except
//! reserved-slot gap sentinels (`INVALID_EDGE_ID` fillers), which carry no
//! edge and are dropped at pack time. Freezing changes the physical layout
//! and the row order, so timestamp-filtered reads observe the same logical
//! content before and after, not the same byte order. Visibility authority
//! stays above this layer; row stamps are physical replicas as in the
//! mutable form.
//!
//! Every packed row is sorted by `(endpoint, rank, create_ts, edge_id)`.
//! Endpoint and rank keep one point-query key contiguous, create stamps fix
//! the version order of same-key entries, and edge ids make the order total.
//! Point queries bisect the key range and return the first timestamp-visible
//! version inside it, which is the earliest-created visible version
//! (edge-id order breaks create-stamp ties). Scans stay linear over the
//! sorted rows.
//!
//! Row offsets are rebuilt in memory on open and on load, never persisted.
//! Every mutating entry point rejects writes: a frozen group must be
//! explicitly unfrozen back into a mutable variant before it accepts writes
//! again. There is no implicit unfreeze on the write path.

use super::csr_shared::decode_endpoint_pair;
use super::mutable_csr::serialization::{
    decode_topology_i64_column, decode_topology_u32_column, decode_topology_u64_column,
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
    TopologyColumnEncoding,
};
use super::{
    ColdStamps, CsrBase, EdgeId, EdgePosition, HotNbr, MutableCsr, MutableCsrTrait, Nbr,
    SingleMutableCsr, Timestamp, VertexId, INVALID_EDGE_ID,
};
use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

/// Persisted format version of the frozen neighbor payload.
///
/// Version 1 only. Older or newer versions are rejected on load, never
/// converted.
pub(crate) const IMMUTABLE_CSR_FORMAT_VERSION: u32 = 1;

fn frozen_error() -> StorageError {
    StorageError::invalid_operation(
        "frozen CSR group rejects writes: unfreeze the group before writing".to_string(),
    )
}

/// Frozen row order: point-query key first, then version order, then a
/// stable total-order tiebreak. Total, so packing sorts deterministically.
fn frozen_row_key(nbr: &Nbr) -> (u32, i64, Timestamp, u64) {
    (nbr.endpoint, nbr.rank, nbr.create_ts, nbr.edge_id.0)
}

/// Sort one packed row into frozen order and drop reserved-slot gap
/// sentinels, which carry no edge. Tombstones sort by the same key and stay:
/// queries filter them by timestamp inside the key range.
fn sort_packed_row(row: &mut Vec<Nbr>) {
    row.retain(|nbr| nbr.edge_id != INVALID_EDGE_ID);
    row.sort_by(|a, b| frozen_row_key(a).cmp(&frozen_row_key(b)));
}

/// `(endpoint, rank)` key range inside one sorted frozen row, as
/// `(start, end)` offsets relative to the row slice. Empty when the row
/// holds no entry with the key.
fn frozen_key_range(hot: &[HotNbr], endpoint: u32, rank: i64) -> (usize, usize) {
    let lo = hot.partition_point(|h| (h.endpoint, h.rank) < (endpoint, rank));
    let hi = hot.partition_point(|h| (h.endpoint, h.rank) <= (endpoint, rank));
    (lo, hi)
}

/// Packed immutable adjacency of one group.
///
/// `hot_entries`/`cold_entries` hold every row back to back in row order,
/// each row sorted by `(endpoint, rank, create_ts, edge_id)`;
/// `degrees[row]` is the row length and `offsets[row]` its start inside the
/// halves. Empty rows contribute no slots. `offsets` is memory-only state
/// rebuilt by packing and by loading.
#[derive(Debug, Clone)]
pub struct ImmutableCsr {
    hot_entries: Vec<HotNbr>,
    cold_entries: Vec<ColdStamps>,
    degrees: Vec<u32>,
    offsets: Vec<u32>,
    edge_count: u64,
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
        }
    }

    /// Drop every entry, keeping no rows.
    pub fn clear(&mut self) {
        self.hot_entries.clear();
        self.cold_entries.clear();
        self.degrees.clear();
        self.offsets.clear();
        self.edge_count = 0;
    }

    /// Row count of the packed table.
    pub fn vertex_capacity(&self) -> usize {
        self.degrees.len()
    }

    /// Packed hot halves for serving-file writers.
    pub(crate) fn packed_hot(&self) -> &[HotNbr] {
        &self.hot_entries
    }

    /// Packed cold halves for serving-file writers.
    pub(crate) fn packed_cold(&self) -> &[ColdStamps] {
        &self.cold_entries
    }

    /// Packed row degrees for serving-file writers.
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
    /// by `(endpoint, rank, create_ts, edge_id)`. Timestamp-filtered reads
    /// observe the same logical entries as the source table.
    pub fn pack_from_mutable(csr: &MutableCsr) -> Self {
        let rows = csr.vertex_capacity();
        let mut hot_entries = Vec::with_capacity(csr.edge_count() as usize);
        let mut cold_entries = Vec::with_capacity(csr.edge_count() as usize);
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            csr.fill_physical_into(local as u32, &mut row_buf);
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
        };
        packed.rebuild_offsets();
        packed
    }

    /// Pack every physical entry of a single-edge CSR, sorted the same way.
    ///
    /// Rows hold at most one entry; the packed form is uniform with the
    /// multi-edge pack so one frozen type serves both strategies.
    pub fn pack_single_from(csr: &SingleMutableCsr) -> Self {
        let rows = csr.vertex_capacity();
        let mut hot_entries = Vec::new();
        let mut cold_entries = Vec::new();
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            csr.fill_physical_into(local as u32, &mut row_buf);
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
        };
        packed.rebuild_offsets();
        packed
    }

    fn rebuild_offsets(&mut self) {
        self.offsets.clear();
        self.offsets.reserve(self.degrees.len());
        let mut base = 0u32;
        for degree in &self.degrees {
            self.offsets.push(base);
            base = base.saturating_add(*degree);
        }
    }

    fn row_window(&self, src_vid: u32) -> Option<(usize, usize)> {
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
    fn slot_at(&self, idx: usize) -> Option<Nbr> {
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
    /// filters inside the range by timestamp. The row sort makes the answer
    /// the earliest-created visible version (edge-id order breaks ties).
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

    /// Frozen rows are never reclaimed; maintenance skips frozen groups.
    pub fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }

    /// Approximate memory usage in bytes.
    pub fn used_memory_size(&self) -> usize {
        self.hot_entries
            .len()
            .saturating_mul(std::mem::size_of::<HotNbr>())
            .saturating_add(
                self.cold_entries
                    .len()
                    .saturating_mul(std::mem::size_of::<ColdStamps>()),
            )
            .saturating_add(
                self.degrees
                    .len()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                self.offsets
                    .len()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(std::mem::size_of::<Self>())
    }

    /// Encoding report for the persisted neighbor columns.
    pub fn topology_encoding_report(&self) -> Vec<(String, TopologyColumnEncoding, usize, usize)> {
        let endpoints: Vec<u32> = self.hot_entries.iter().map(|hot| hot.endpoint).collect();
        let edge_ids: Vec<u64> = self.hot_entries.iter().map(|hot| hot.edge_id.0).collect();
        let (endpoint_choice, _) = encode_topology_u32_column(&endpoints);
        let (edge_id_choice, _) = encode_topology_u64_column(&edge_ids);
        let (degrees_choice, _) = encode_topology_u32_column(&self.degrees);
        vec![
            (
                "neighbor".to_string(),
                endpoint_choice.encoding,
                endpoint_choice.plain_bytes,
                endpoint_choice.encoded_bytes,
            ),
            (
                "edge_id".to_string(),
                edge_id_choice.encoding,
                edge_id_choice.plain_bytes,
                edge_id_choice.encoded_bytes,
            ),
            (
                "lengths".to_string(),
                degrees_choice.encoding,
                degrees_choice.plain_bytes,
                degrees_choice.encoded_bytes,
            ),
        ]
    }

    /// Iterate timestamp-visible entries across all rows.
    pub fn iter(&self, ts: Timestamp) -> ImmutableCsrIterator<'_> {
        ImmutableCsrIterator::new(self, ts)
    }

    /// Iterate timestamp-visible entries of one row without allocating.
    ///
    /// Zero-copy counterpart of `edges_of`: walks the packed hot slice and
    /// assembles each record with its cold half inline, so per-vertex scans
    /// over frozen groups never touch the allocator. Out-of-range rows yield
    /// an empty iterator.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> FrozenRowIter<'_> {
        let (start, end) = self.row_window(src_vid).unwrap_or((0, 0));
        FrozenRowIter {
            hot: &self.hot_entries[start..end],
            cold: &self.cold_entries[start..end],
            ts,
            idx: 0,
        }
    }

    /// Iterate every physically stored entry, including tombstoned ones.
    pub fn iter_all(&self) -> ImmutableCsrIterator<'_> {
        ImmutableCsrIterator::new_all(self)
    }

    /// Dump to bytes, version 1.
    ///
    /// Format:
    /// - format_version (u32 = 1)
    /// - rows (u64)
    /// - edge_count (u64)
    /// - entries_len (u64)
    /// - encoded degrees column
    /// - encoded endpoints column
    /// - encoded ranks column
    /// - encoded edge ids column
    /// - encoded create stamps column
    /// - encoded delete stamps column
    ///
    /// Row offsets are memory-only and never persisted; load rebuilds them.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based dump into `out` without an intermediate owned buffer.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let mut scratch = super::mutable_csr::persistence::CsrDumpScratch::new();
        self.dump_into_with_scratch(out, &mut scratch);
    }

    /// Dump reusing caller-owned column buffers.
    ///
    /// Same bytes as [`Self::dump_into`]; a checkpoint over many groups pays
    /// one allocation per column instead of one per group. The scratch holds
    /// no state between calls.
    pub fn dump_into_with_scratch(
        &self,
        out: &mut Vec<u8>,
        scratch: &mut super::mutable_csr::persistence::CsrDumpScratch,
    ) {
        out.extend_from_slice(&IMMUTABLE_CSR_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.degrees.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.hot_entries.len() as u64).to_le_bytes());
        let (_, degrees_payload) = encode_topology_u32_column(&self.degrees);
        out.extend_from_slice(&degrees_payload);
        scratch.fill_from_split(&self.hot_entries, &self.cold_entries);
        let (_, endpoints_payload) = encode_topology_u32_column(scratch.endpoints());
        out.extend_from_slice(&endpoints_payload);
        let (_, ranks_payload) = encode_topology_i64_column(scratch.ranks());
        out.extend_from_slice(&ranks_payload);
        let (_, edge_ids_payload) = encode_topology_u64_column(scratch.edge_ids());
        out.extend_from_slice(&edge_ids_payload);
        let (_, create_payload) = encode_topology_u64_column(scratch.creates());
        out.extend_from_slice(&create_payload);
        let (_, delete_payload) = encode_topology_u64_column(scratch.deletes());
        out.extend_from_slice(&delete_payload);
    }

    /// Load from bytes, version 1 only.
    ///
    /// Rejects short headers, version mismatches, column length mismatches,
    /// out-of-range row windows, edge count mismatches and trailing bytes.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 32 {
            return Err(StorageError::deserialize_error(
                "frozen CSR data too short for header",
            ));
        }
        let mut offset = 0usize;
        let format_version = read_u32_le(data, &mut offset)?;
        if format_version != IMMUTABLE_CSR_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported frozen CSR format version: {format_version}"
            )));
        }
        let rows = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let entries_len = read_u64_le(data, &mut offset)? as usize;
        let degrees = decode_topology_u32_column(data, &mut offset)?;
        if degrees.len() != rows {
            return Err(StorageError::deserialize_error(
                "frozen CSR degree column length mismatch",
            ));
        }
        let endpoints = decode_topology_u32_column(data, &mut offset)?;
        let ranks = decode_topology_i64_column(data, &mut offset)?;
        let edge_ids = decode_topology_u64_column(data, &mut offset)?;
        let create_stamps = decode_topology_u64_column(data, &mut offset)?;
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != entries_len
            || ranks.len() != entries_len
            || edge_ids.len() != entries_len
            || create_stamps.len() != entries_len
            || delete_stamps.len() != entries_len
        {
            return Err(StorageError::deserialize_error(
                "frozen CSR neighbor column length mismatch",
            ));
        }
        let mut hot_entries = Vec::with_capacity(entries_len);
        let mut cold_entries = Vec::with_capacity(entries_len);
        let mut recomputed: u64 = 0;
        for index in 0..entries_len {
            let hot = HotNbr {
                endpoint: endpoints[index],
                rank: ranks[index],
                edge_id: EdgeId(edge_ids[index]),
            };
            let cold = ColdStamps {
                create_ts: create_stamps[index],
                delete_ts: delete_stamps[index],
            };
            if hot.edge_id != INVALID_EDGE_ID && cold.is_live() {
                recomputed += 1;
            }
            hot_entries.push(hot);
            cold_entries.push(cold);
        }
        let mut covered = 0usize;
        for degree in &degrees {
            covered = covered.saturating_add(*degree as usize);
        }
        if covered != entries_len {
            return Err(StorageError::deserialize_error(format!(
                "frozen CSR row window mismatch: degrees cover {} entries, payload holds {}",
                covered, entries_len
            )));
        }
        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "frozen CSR edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in frozen CSR payload",
            ));
        }
        self.hot_entries = hot_entries;
        self.cold_entries = cold_entries;
        self.degrees = degrees;
        self.edge_count = edge_count;
        self.rebuild_offsets();
        Ok(())
    }
}

impl Default for ImmutableCsr {
    fn default() -> Self {
        Self::new()
    }
}

impl CsrBase for ImmutableCsr {
    fn vertex_capacity(&self) -> usize {
        self.degrees.len()
    }

    fn edge_count(&self) -> u64 {
        self.edge_count
    }

    fn dump(&self) -> Vec<u8> {
        ImmutableCsr::dump(self)
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        ImmutableCsr::dump_into(self, out);
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        ImmutableCsr::load(self, data)
    }
}

impl MutableCsrTrait for ImmutableCsr {
    fn insert_edge(
        &mut self,
        _src_vid: u32,
        _dst: VertexId,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        Err(frozen_error())
    }

    fn delete_edge(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(frozen_error())
    }

    fn delete_edge_by_dst(&mut self, _src_vid: u32, _dst: VertexId, _ts: Timestamp) -> usize {
        0
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        ImmutableCsr::locate_edge(self, src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(frozen_error())
    }

    fn revert_delete_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        false
    }

    fn delete_edge_by_offset(
        &mut self,
        _src_vid: u32,
        _offset: i32,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(frozen_error())
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        ImmutableCsr::nbr_at_offset(self, src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        ImmutableCsr::get_edge_physical(self, src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        ImmutableCsr::physical_edges_of(self, src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        ImmutableCsr::fill_physical_into(self, src_vid, out);
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        ImmutableCsr::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        ImmutableCsr::primary_contains(self, src_vid, edge_id)
    }

    fn remove_edge(&mut self, _src_vid: u32, _edge_id: EdgeId) -> bool {
        false
    }

    fn revert_delete_by_edge_id(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> bool {
        false
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        ImmutableCsr::get_edge(self, src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        ImmutableCsr::edges_of(self, src_vid, ts)
    }

    fn compact_vertex_with_reporting(
        &mut self,
        _vid: u32,
        _cutoff: Timestamp,
        _on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        0
    }

    fn reclaimable_count(&self, _vid: u32, _cutoff: Timestamp) -> usize {
        0
    }

    fn vertex_needs_compact(&self, _vid: u32, _cutoff: Timestamp) -> bool {
        false
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        ImmutableCsr::vertex_census(self, vid)
    }

    fn vertex_reclaim_probe(&self, _vid: u32, _cutoff: Timestamp) -> (usize, usize) {
        (0, 0)
    }

    fn row_gap(&self, _vid: u32) -> usize {
        0
    }

    fn row_density(&self, _vid: u32) -> f32 {
        1.0
    }

    fn rebalance_row(&mut self, _vid: u32) -> bool {
        true
    }

    fn used_memory_size(&self) -> usize {
        ImmutableCsr::used_memory_size(self)
    }
}

/// Iterator over one frozen row, yielding assembled entries.
///
/// The packed row is already contiguous, so iteration is a filtered
/// hot-slice walk with no allocation and no pointer chasing. Records are
/// assembled by value because the halves live in separate slices.
pub struct FrozenRowIter<'a> {
    hot: &'a [HotNbr],
    cold: &'a [ColdStamps],
    ts: Timestamp,
    idx: usize,
}

impl<'a> Iterator for FrozenRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.hot.len() {
            let nbr = Nbr::from_parts(self.hot[self.idx], self.cold[self.idx]);
            self.idx += 1;
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }
        None
    }
}

/// Iterator over frozen rows, yielding local vertex ids.
pub struct ImmutableCsrIterator<'a> {
    csr: &'a ImmutableCsr,
    ts: Timestamp,
    include_deleted: bool,
    row: usize,
    idx: usize,
}

impl<'a> ImmutableCsrIterator<'a> {
    fn new(csr: &'a ImmutableCsr, ts: Timestamp) -> Self {
        Self {
            csr,
            ts,
            include_deleted: false,
            row: 0,
            idx: 0,
        }
    }

    fn new_all(csr: &'a ImmutableCsr) -> Self {
        Self {
            csr,
            ts: 0,
            include_deleted: true,
            row: 0,
            idx: 0,
        }
    }
}

impl<'a> Iterator for ImmutableCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.row < self.csr.degrees.len() {
            let end = self.csr.offsets[self.row] as usize + self.csr.degrees[self.row] as usize;
            while self.idx < end {
                let nbr = self.csr.slot_at(self.idx).unwrap();
                self.idx += 1;
                if self.include_deleted || nbr.is_alive_at(self.ts) {
                    return Some((VertexId::from_int64(self.row as i64), nbr));
                }
            }
            self.row += 1;
            if self.row < self.csr.offsets.len() {
                self.idx = self.csr.offsets[self.row] as usize;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed_endpoint(endpoint: u32, rank: i64) -> VertexId {
        VertexId::edge_endpoint_key(endpoint, rank)
    }

    /// Logical-content comparison: same multiset of entries in frozen row
    /// order, independent of the source physical order.
    fn sorted_physical(entries: Vec<Nbr>) -> Vec<Nbr> {
        let mut sorted = entries;
        sorted.sort_by(|a, b| super::frozen_row_key(a).cmp(&super::frozen_row_key(b)));
        sorted
    }

    fn sample_mutable() -> MutableCsr {
        let mut csr = MutableCsr::with_capacity(8, 64);
        csr.insert_edge(0, packed_endpoint(10, 0), EdgeId(1), 1)
            .unwrap();
        csr.insert_edge(0, packed_endpoint(11, 0), EdgeId(2), 1)
            .unwrap();
        csr.insert_edge(3, packed_endpoint(12, 1), EdgeId(3), 2)
            .unwrap();
        csr.delete_edge(0, EdgeId(2), 5).unwrap();
        csr
    }

    #[test]
    fn pack_preserves_logical_reads_in_sorted_order() {
        let mutable = sample_mutable();
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        assert_eq!(frozen.vertex_capacity(), mutable.vertex_capacity());
        assert_eq!(frozen.edge_count(), mutable.edge_count());
        for vid in 0..8u32 {
            // Logical equivalence, not byte order: frozen rows are sorted.
            assert_eq!(
                frozen.physical_edges_of(vid),
                sorted_physical(mutable.physical_edges_of(vid))
            );
            for ts in [1u64, 2, 4, 5, 6] {
                assert_eq!(
                    sorted_physical(frozen.edges_of(vid, ts)),
                    sorted_physical(mutable.edges_of(vid, ts))
                );
            }
            // Same live/dead split; capacity differs by construction:
            // frozen rows carry no reserved gaps.
            let (f_live, f_dead, f_cap) = frozen.vertex_census(vid);
            let (m_live, m_dead, _) = mutable.vertex_census(vid);
            assert_eq!((f_live, f_dead), (m_live, m_dead));
            assert_eq!(f_cap, frozen.physical_edges_of(vid).len());
            assert_eq!(
                frozen.get_edge(vid, packed_endpoint(10, 0), 6),
                mutable.get_edge(vid, packed_endpoint(10, 0), 6)
            );
        }
        assert_eq!(
            frozen.get_edge_physical(0, packed_endpoint(10, 0)),
            mutable.get_edge_physical(0, packed_endpoint(10, 0))
        );
        assert!(frozen.primary_contains(0, EdgeId(1)));
        assert!(!frozen.primary_contains(0, EdgeId(999)));
        assert!(frozen.has_physical_entries(0));
        assert!(!frozen.has_physical_entries(1));
        // Offsets address the sorted row: position 0 is the smallest key.
        let row0 = frozen.physical_edges_of(0);
        for (pos, nbr) in row0.iter().enumerate() {
            assert_eq!(frozen.nbr_at_offset(0, pos as i32), Some(*nbr));
        }
        assert_eq!(frozen.nbr_at_offset(0, 99), None);
        assert_eq!(frozen.nbr_at_offset(9, 0), None);
        assert_eq!(
            frozen.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr),
            mutable.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr)
        );
    }

    #[test]
    fn pack_single_matches_mutable_reads() {
        let mut single = SingleMutableCsr::with_capacity(4);
        single
            .insert_edge(1, packed_endpoint(20, 0), EdgeId(7), 3)
            .unwrap();
        let frozen = ImmutableCsr::pack_single_from(&single);
        assert_eq!(frozen.vertex_capacity(), 4);
        assert_eq!(frozen.edge_count(), 1);
        assert_eq!(frozen.edges_of(1, 3), single.edges_of(1, 3));
        assert_eq!(
            frozen.get_edge(1, packed_endpoint(20, 0), 9),
            single.get_edge(1, packed_endpoint(20, 0), 9)
        );
        assert!(frozen.physical_edges_of(0).is_empty());
    }

    #[test]
    fn frozen_writes_are_rejected() {
        let mut frozen = ImmutableCsr::pack_from_mutable(&sample_mutable());
        assert!(frozen
            .insert_edge(0, packed_endpoint(30, 0), EdgeId(100), 9)
            .is_err());
        assert!(frozen.delete_edge(0, EdgeId(1), 9).is_err());
        assert_eq!(frozen.delete_edge_by_dst(0, packed_endpoint(10, 0), 9), 0);
        assert!(frozen.delete_edge_by_offset(0, 0, 9).is_err());
        assert!(frozen
            .delete_edge_at_position(0, EdgePosition::Primary { slot: 0 }, EdgeId(1), 9)
            .is_err());
        assert!(!frozen.remove_edge(0, EdgeId(1)));
        assert!(!frozen.revert_delete_by_edge_id(0, EdgeId(2), 9));
        assert!(!frozen.revert_delete_by_offset(0, 0, 9));
        assert!(!frozen.revert_delete_at_position(
            0,
            EdgePosition::Primary { slot: 0 },
            EdgeId(2),
            9
        ));
        assert_eq!(
            frozen.compact_vertex_with_reporting(0, 9, &mut |_, _| {}),
            0
        );
        assert_eq!(frozen.reclaimable_count(0, 9), 0);
        assert!(!frozen.vertex_needs_compact(0, 9));
        assert_eq!(frozen.row_gap(0), 0);
        assert_eq!(frozen.row_density(0), 1.0);
        assert!(frozen.rebalance_row(0));
    }

    #[test]
    fn dump_load_roundtrip_restores_reads() {
        let frozen = ImmutableCsr::pack_from_mutable(&sample_mutable());
        let bytes = frozen.dump();
        let mut loaded = ImmutableCsr::new();
        loaded.load(&bytes).unwrap();
        assert_eq!(loaded.vertex_capacity(), frozen.vertex_capacity());
        assert_eq!(loaded.edge_count(), frozen.edge_count());
        for vid in 0..8u32 {
            assert_eq!(loaded.physical_edges_of(vid), frozen.physical_edges_of(vid));
            assert_eq!(loaded.edges_of(vid, 6), frozen.edges_of(vid, 6));
        }
        let mut scratch = super::super::mutable_csr::persistence::CsrDumpScratch::new();
        let mut via_scratch = Vec::new();
        loaded.dump_into_with_scratch(&mut via_scratch, &mut scratch);
        assert_eq!(via_scratch, bytes);
    }

    #[test]
    fn load_rejects_damage() {
        let bytes = ImmutableCsr::pack_from_mutable(&sample_mutable()).dump();
        let mut loaded = ImmutableCsr::new();
        assert!(loaded.load(&[]).is_err());
        assert!(loaded.load(&bytes[..10]).is_err());
        let mut bad_version = bytes.clone();
        bad_version[0] = 99;
        assert!(loaded.load(&bad_version).is_err());
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(loaded.load(&trailing).is_err());
    }

    #[test]
    fn iterators_match_mutable_counts() {
        let mutable = sample_mutable();
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        assert_eq!(frozen.iter(6).count(), mutable.iter(6).count());
        assert_eq!(frozen.iter_all().count(), mutable.iter_all().count());
        // Full scans stay linear; frozen walks sorted rows, so compare the
        // same logical multiset instead of the walk order.
        let mut frozen_pairs: Vec<(VertexId, EdgeId)> = frozen
            .iter_all()
            .map(|(vid, nbr)| (vid, nbr.edge_id))
            .collect();
        let mut mutable_pairs: Vec<(VertexId, EdgeId)> = mutable
            .iter_all()
            .map(|(vid, nbr)| (vid, nbr.edge_id))
            .collect();
        frozen_pairs.sort_by_key(|(vid, edge)| (vid.as_int64().unwrap_or(0), edge.0));
        mutable_pairs.sort_by_key(|(vid, edge)| (vid.as_int64().unwrap_or(0), edge.0));
        assert_eq!(frozen_pairs, mutable_pairs);
    }

    #[test]
    fn empty_table_packs_and_loads() {
        let mutable = MutableCsr::with_capacity(4, 16);
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        assert_eq!(frozen.edge_count(), 0);
        assert_eq!(frozen.vertex_capacity(), 4);
        assert!(frozen.edges_of(0, 1).is_empty());
        let bytes = frozen.dump();
        let mut loaded = ImmutableCsr::new();
        loaded.load(&bytes).unwrap();
        assert_eq!(loaded.edge_count(), 0);
        assert!(loaded.used_memory_size() > 0);
    }

    #[test]
    fn packed_rows_are_sorted_and_sentinel_free() {
        let mut mutable = MutableCsr::with_capacity(4, 64);
        // Inserted out of key order on purpose.
        for (endpoint, edge) in [(50, 1), (10, 2), (30, 3), (20, 4), (40, 5)] {
            mutable
                .insert_edge(0, packed_endpoint(endpoint, 0), EdgeId(edge), 1)
                .unwrap();
        }
        mutable.delete_edge(0, EdgeId(3), 4).unwrap();
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        let row = frozen.physical_edges_of(0);
        assert_eq!(row.len(), 5);
        let keys: Vec<(u32, i64)> = row.iter().map(|nbr| (nbr.endpoint, nbr.rank)).collect();
        assert_eq!(keys, vec![(10, 0), (20, 0), (30, 0), (40, 0), (50, 0)]);
        assert!(row.iter().all(|nbr| nbr.edge_id != INVALID_EDGE_ID));
        // Tombstone kept and still filtered by timestamp.
        assert_eq!(frozen.edges_of(0, 5).len(), 4);
        assert_eq!(frozen.edges_of(0, 3).len(), 5);
    }

    #[test]
    fn wide_row_point_queries_match_mutable() {
        let mut mutable = MutableCsr::with_capacity(2, 256);
        // Monotonic timestamps: earliest-created equals first-inserted, so
        // the frozen version rule and the mutable order agree everywhere.
        let mut endpoints: Vec<u32> = (0..120).collect();
        endpoints.reverse();
        for (i, endpoint) in endpoints.iter().enumerate() {
            mutable
                .insert_edge(
                    0,
                    packed_endpoint(*endpoint, (i % 3) as i64),
                    EdgeId(1000 + i as u64),
                    1 + i as u64,
                )
                .unwrap();
        }
        for i in (0..120usize).step_by(7) {
            mutable
                .delete_edge(0, EdgeId(1000 + i as u64), 1000)
                .unwrap();
        }
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        for endpoint in [0u32, 1, 59, 60, 119] {
            for rank in [0i64, 1, 2] {
                for ts in [1u64, 60, 500, 999, 1000, 2000] {
                    let key = packed_endpoint(endpoint, rank);
                    assert_eq!(
                        frozen.get_edge(0, key, ts),
                        mutable.get_edge(0, key, ts),
                        "endpoint={endpoint} rank={rank} ts={ts}"
                    );
                    assert_eq!(
                        frozen.get_edge_physical(0, key),
                        mutable.get_edge_physical(0, key),
                        "physical endpoint={endpoint} rank={rank}"
                    );
                }
            }
        }
        assert_eq!(
            sorted_physical(frozen.edges_of(0, 2000)),
            sorted_physical(mutable.edges_of(0, 2000))
        );
    }

    #[test]
    fn same_key_versions_select_earliest_created() {
        let mut mutable = MutableCsr::with_capacity(2, 16);
        let key = packed_endpoint(42, 0);
        // Same key twice via delete plus reinsert with a backdated create
        // stamp: insertion order and create order disagree on purpose.
        mutable.insert_edge(0, key, EdgeId(1), 5).unwrap();
        mutable.delete_edge(0, EdgeId(1), 9).unwrap();
        mutable.insert_edge(0, key, EdgeId(2), 3).unwrap();
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        // Both versions visible at ts=6: mutable answers insertion-first,
        // frozen answers earliest-created per the documented frozen rule.
        assert_eq!(
            mutable.get_edge(0, key, 6).map(|nbr| nbr.edge_id),
            Some(EdgeId(1))
        );
        assert_eq!(
            frozen.get_edge(0, key, 6).map(|nbr| nbr.edge_id),
            Some(EdgeId(2))
        );
        // After the first version's tombstone closes, both agree again.
        assert_eq!(
            frozen.get_edge(0, key, 9).map(|nbr| nbr.edge_id),
            mutable.get_edge(0, key, 9).map(|nbr| nbr.edge_id)
        );
        assert_eq!(
            frozen.get_edge(0, key, 9).map(|nbr| nbr.edge_id),
            Some(EdgeId(2))
        );
    }
}
