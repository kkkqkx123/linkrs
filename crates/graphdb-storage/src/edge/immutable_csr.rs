//! Frozen immutable CSR topology.
//!
//! Packed read-only form of one group's adjacency: a single contiguous
//! neighbor segment plus a per-row degree table. Empty rows hold no slots.
//! There is no capacity array, no overflow chain, no live index and no lock:
//! a frozen group pays only its entries plus two small per-row arrays.
//!
//! A frozen group keeps every neighbor byte of the mutable group it was
//! packed from, including edge ids, both timestamps and tombstones. Freezing
//! changes only the physical layout, so timestamp-filtered reads observe the
//! same entries before and after. Visibility authority stays above this
//! layer; row stamps are physical replicas as in the mutable form.
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
    CsrBase, EdgeId, EdgePosition, MutableCsr, MutableCsrTrait, Nbr, SingleMutableCsr, Timestamp,
    VertexId, INVALID_EDGE_ID,
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

/// Packed immutable adjacency of one group.
///
/// `entries` holds every row back to back in row order; `degrees[row]` is the
/// row length and `offsets[row]` its start inside `entries`. Empty rows
/// contribute no slots. `offsets` is memory-only state rebuilt by packing and
/// by loading.
#[derive(Debug, Clone)]
pub struct ImmutableCsr {
    entries: Vec<Nbr>,
    degrees: Vec<u32>,
    offsets: Vec<u32>,
    edge_count: u64,
}

impl ImmutableCsr {
    /// Empty table with no rows.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            degrees: Vec::new(),
            offsets: Vec::new(),
            edge_count: 0,
        }
    }

    /// Drop every entry, keeping no rows.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.degrees.clear();
        self.offsets.clear();
        self.edge_count = 0;
    }

    /// Row count of the packed table.
    pub fn vertex_capacity(&self) -> usize {
        self.degrees.len()
    }

    /// Live edge count of the packed table.
    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    /// Pack every physical entry of a mutable CSR, primary rows and overflow
    /// chains merged per row in scan order.
    ///
    /// Content-preserving: tombstones and gap sentinels travel verbatim, so
    /// every read observes the same entries as the source table.
    pub fn pack_from_mutable(csr: &MutableCsr) -> Self {
        let rows = csr.vertex_capacity();
        let mut entries = Vec::with_capacity(csr.edge_count() as usize);
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            csr.fill_physical_into(local as u32, &mut row_buf);
            degrees.push(row_buf.len() as u32);
            for nbr in &row_buf {
                if nbr.edge_id != INVALID_EDGE_ID && nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                }
                entries.push(*nbr);
            }
        }
        let mut packed = Self {
            entries,
            degrees,
            offsets: Vec::with_capacity(rows),
            edge_count: live,
        };
        packed.rebuild_offsets();
        packed
    }

    /// Pack every physical entry of a single-edge CSR.
    ///
    /// Rows hold at most one entry; the packed form is uniform with the
    /// multi-edge pack so one frozen type serves both strategies.
    pub fn pack_single_from(csr: &SingleMutableCsr) -> Self {
        let rows = csr.vertex_capacity();
        let mut entries = Vec::new();
        let mut degrees = Vec::with_capacity(rows);
        let mut row_buf = Vec::new();
        let mut live = 0u64;
        for local in 0..rows {
            csr.fill_physical_into(local as u32, &mut row_buf);
            degrees.push(row_buf.len() as u32);
            for nbr in &row_buf {
                if nbr.edge_id != INVALID_EDGE_ID && nbr.delete_ts == Timestamp::MAX {
                    live += 1;
                }
                entries.push(*nbr);
            }
        }
        let mut packed = Self {
            entries,
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
        if start.saturating_add(degree) > self.entries.len() {
            return None;
        }
        Some((start, start + degree))
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
    pub fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        let Some((start, end)) = self.row_window(src_vid) else {
            return Vec::new();
        };
        self.entries[start..end]
            .iter()
            .filter(|nbr| nbr.is_alive_at(ts))
            .copied()
            .collect()
    }

    /// First timestamp-visible entry matching an endpoint key.
    pub fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        self.entries[start..end].iter().find_map(|nbr| {
            if nbr.endpoint == decoded_endpoint && nbr.rank == decoded_rank && nbr.is_alive_at(ts) {
                Some(*nbr)
            } else {
                None
            }
        })
    }

    /// First live entry matching an endpoint key without consulting snapshots.
    pub fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        let (decoded_endpoint, decoded_rank) = decode_endpoint_pair(dst);
        let (start, end) = self.row_window(src_vid)?;
        self.entries[start..end].iter().find_map(|nbr| {
            if nbr.endpoint == decoded_endpoint
                && nbr.rank == decoded_rank
                && nbr.edge_id != INVALID_EDGE_ID
                && nbr.delete_ts == Timestamp::MAX
            {
                Some(*nbr)
            } else {
                None
            }
        })
    }

    /// Every physically stored entry of one row without timestamp filtering.
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
        out.extend_from_slice(&self.entries[start..end]);
    }

    /// Visit every physically stored entry of one row without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((start, end)) = self.row_window(src_vid) else {
            return;
        };
        for nbr in &self.entries[start..end] {
            if !f(*nbr) {
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
        self.entries.get(start + offset as usize).copied()
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
        self.entries[start..end]
            .iter()
            .any(|nbr| nbr.edge_id == edge_id)
    }

    /// Locate the first entry with `edge_id`, returning its packed slot.
    pub fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        let (start, end) = self.row_window(src_vid)?;
        for (slot, nbr) in self.entries[start..end].iter().enumerate() {
            if nbr.edge_id == edge_id {
                return Some((EdgePosition::Primary { slot: slot as u32 }, *nbr));
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
        for nbr in &self.entries[start..end] {
            if nbr.delete_ts == Timestamp::MAX {
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
        self.entries
            .len()
            .saturating_mul(std::mem::size_of::<Nbr>())
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
        let endpoints: Vec<u32> = self.entries.iter().map(|nbr| nbr.endpoint).collect();
        let edge_ids: Vec<u64> = self.entries.iter().map(|nbr| nbr.edge_id.0).collect();
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
        out.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        let (_, degrees_payload) = encode_topology_u32_column(&self.degrees);
        out.extend_from_slice(&degrees_payload);
        scratch.fill_from(&self.entries);
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
        let mut entries = Vec::with_capacity(entries_len);
        let mut recomputed: u64 = 0;
        for index in 0..entries_len {
            let mut nbr = Nbr::with_timestamps(
                endpoints[index],
                ranks[index],
                EdgeId(edge_ids[index]),
                delete_stamps[index],
            );
            nbr.create_ts = create_stamps[index];
            if nbr.edge_id != INVALID_EDGE_ID && nbr.delete_ts == Timestamp::MAX {
                recomputed += 1;
            }
            entries.push(nbr);
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
        self.entries = entries;
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
                let nbr = self.csr.entries[self.idx];
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
    fn pack_preserves_physical_and_filtered_reads() {
        let mutable = sample_mutable();
        let frozen = ImmutableCsr::pack_from_mutable(&mutable);
        assert_eq!(frozen.vertex_capacity(), mutable.vertex_capacity());
        assert_eq!(frozen.edge_count(), mutable.edge_count());
        for vid in 0..8u32 {
            assert_eq!(
                frozen.physical_edges_of(vid),
                mutable.physical_edges_of(vid)
            );
            for ts in [1u64, 2, 4, 5, 6] {
                assert_eq!(frozen.edges_of(vid, ts), mutable.edges_of(vid, ts));
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
        assert_eq!(frozen.nbr_at_offset(0, 0), mutable.nbr_at_offset(0, 0));
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
        let frozen_pairs: Vec<(VertexId, EdgeId)> = frozen
            .iter_all()
            .map(|(vid, nbr)| (vid, nbr.edge_id))
            .collect();
        let mutable_pairs: Vec<(VertexId, EdgeId)> = mutable
            .iter_all()
            .map(|(vid, nbr)| (vid, nbr.edge_id))
            .collect();
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
}
