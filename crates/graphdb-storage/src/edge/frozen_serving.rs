//! Frozen group mmap serving file.
//!
//! Derived read-only cache beside the authoritative checkpoint: while the
//! checkpoint stays the source of truth, a frozen group can also serve
//! queries straight from a memory-mapped flat column file, paging entries in
//! on demand instead of decoding the whole group into the heap at open.
//!
//! File layout, everything little-endian:
//! - magic (u32), format version (u32)
//! - rows (u64), entries (u64), live edge count (u64)
//! - five `(offset u64, length u64)` column descriptors: degrees, endpoints,
//!   ranks, edge ids, delete stamps
//! - degrees: `rows` u32 values
//! - endpoints: `entries` u32 values
//! - ranks: `entries` i64 values
//! - edge ids, delete stamps: `entries` u64 values each
//!
//! Column widths are fixed, so any slot is addressable by index with one
//! little-endian decode and no full-file decode. Row offsets are rebuilt in
//! memory on open and never persisted, mirroring the heap frozen form.
//!
//! The serving payload carries a trailing CRC32 covering every preceding
//! byte, using the same checksum pattern as the heap checkpoint dumps. Open
//! rejects structural mismatches (magic, version, out-of-range descriptors,
//! length disagreements, trailing bytes) and CRC mismatches alike, and the
//! caller falls back to the authoritative checkpoint, optionally
//! regenerating the serving file. Stale files are never upgraded:
//! a bad cache is discarded and rebuilt. Writers use a sibling temp file
//! plus atomic rename, so readers only ever observe complete files.
//!
//! The mapped handle is reference-counted (`MappedFrozen` clones share one
//! mapping), so readers holding a view keep the file alive across serving
//! file replacement. Single-writer discipline applies: the checkpoint flush
//! syncs base file and serving file together, and flushing a non-frozen
//! group removes its serving file. On Linux the mapping carries a transparent
//! huge page hint; a rejected hint falls back to base pages without failing
//! the open.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphdb_core::{StorageError, StorageResult};

use super::csr_shared::decode_endpoint_pair;
use super::immutable_csr::IMMUTABLE_CSR_FORMAT_VERSION;
use super::mutable_csr::serialization::{
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use super::{ColdStamps, EdgePosition, HotNbr, ImmutableCsr, Nbr, INVALID_EDGE_ID};
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

/// Serving file magic: `b"GCSR"`.
pub(crate) const SERVING_MAGIC: u32 = 0x52534347;
/// Serving file format version. Only version 2 is read, never converted.
/// Version 2 appends a trailing CRC32 over every preceding byte.
pub(crate) const SERVING_FORMAT_VERSION: u32 = 2;
/// Header bytes: magic + version + rows + entries + edge count + five
/// `(offset, length)` descriptors.
pub(crate) const SERVING_HEADER_LEN: usize = 8 + 24 + 5 * 16;
/// Trailing checksum bytes covering the header plus all columns.
pub(crate) const SERVING_CRC_LEN: usize = 4;

fn serving_error(message: String) -> StorageError {
    StorageError::deserialize_error(message)
}

/// Sibling serving-file path for a group base file.
pub fn serving_path_for(base: &Path) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(".serving");
    base.with_file_name(name)
}

/// Byte range of one flat column inside the serving file.
#[derive(Debug, Clone, Copy, Default)]
struct ColumnRange {
    start: usize,
    len: usize,
}

impl ColumnRange {
    fn end(&self) -> usize {
        self.start.saturating_add(self.len)
    }
}

/// Column ranges of a validated serving file.
#[derive(Debug, Clone, Copy, Default)]
struct ServingColumns {
    degrees: ColumnRange,
    endpoints: ColumnRange,
    ranks: ColumnRange,
    edge_ids: ColumnRange,
    deletes: ColumnRange,
}

fn read_u32_le_at(bytes: &[u8], offset: usize) -> StorageResult<u32> {
    let end = offset.saturating_add(4);
    let chunk = bytes
        .get(offset..end)
        .ok_or_else(|| serving_error(format!("serving file truncated at byte {offset}")))?;
    Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

fn read_u64_le_at(bytes: &[u8], offset: usize) -> StorageResult<u64> {
    let end = offset.saturating_add(8);
    let chunk = bytes
        .get(offset..end)
        .ok_or_else(|| serving_error(format!("serving file truncated at byte {offset}")))?;
    Ok(u64::from_le_bytes([
        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
    ]))
}

/// Parse and validate the serving header, returning row/entry counts, the
/// stored live edge count and the column ranges.
fn parse_header(bytes: &[u8]) -> StorageResult<(usize, usize, u64, ServingColumns)> {
    if bytes.len() < SERVING_HEADER_LEN + SERVING_CRC_LEN {
        return Err(serving_error(format!(
            "serving file too short for header: {} bytes",
            bytes.len()
        )));
    }
    let (body, trailer) = bytes.split_at(bytes.len() - SERVING_CRC_LEN);
    let stored = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let computed = crc32fast::hash(body);
    if stored != computed {
        return Err(serving_error(format!(
            "serving file CRC mismatch: stored={stored:#x} computed={computed:#x}"
        )));
    }
    let bytes = body;
    let magic = read_u32_le_at(bytes, 0)?;
    if magic != SERVING_MAGIC {
        return Err(serving_error(format!(
            "serving file magic mismatch: {magic:#010x}"
        )));
    }
    let version = read_u32_le_at(bytes, 4)?;
    if version != SERVING_FORMAT_VERSION {
        return Err(serving_error(format!(
            "unsupported serving file version: {version}"
        )));
    }
    let rows = read_u64_le_at(bytes, 8)? as usize;
    let entries = read_u64_le_at(bytes, 16)? as usize;
    let edge_count = read_u64_le_at(bytes, 24)?;
    let mut cursor = 32usize;
    let mut ranges = [ColumnRange::default(); 5];
    for range in &mut ranges {
        let start = read_u64_le_at(bytes, cursor)? as usize;
        let len = read_u64_le_at(bytes, cursor + 8)? as usize;
        *range = ColumnRange { start, len };
        cursor += 16;
    }
    let expected = [
        rows.saturating_mul(4),
        entries.saturating_mul(4),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
    ];
    let mut poll_end = SERVING_HEADER_LEN;
    for (range, want) in ranges.iter().zip(expected) {
        if range.len != want {
            return Err(serving_error(format!(
                "serving column length mismatch: holds {} bytes, layout needs {want}",
                range.len
            )));
        }
        if range.start != poll_end {
            return Err(serving_error(format!(
                "serving column gap or overlap at byte {}",
                range.start
            )));
        }
        poll_end = range.end();
    }
    if bytes.len() != poll_end {
        return Err(serving_error(format!(
            "serving file size mismatch: holds {} bytes, layout needs {poll_end}",
            bytes.len()
        )));
    }
    let columns = ServingColumns {
        degrees: ranges[0],
        endpoints: ranges[1],
        ranks: ranges[2],
        edge_ids: ranges[3],
        deletes: ranges[4],
    };
    Ok((rows, entries, edge_count, columns))
}

/// Write the serving file for one frozen group.
///
/// Encodes the packed columns flat and swaps the file in atomically through
/// a sibling temp file, so concurrent readers never observe a partial file.
pub fn write_serving_file(frozen: &ImmutableCsr, path: &Path) -> StorageResult<()> {
    let hot = frozen.packed_hot();
    let cold = frozen.packed_cold();
    let degrees = frozen.packed_degrees();
    debug_assert_eq!(hot.len(), cold.len());
    let rows = degrees.len();
    let entries = hot.len();
    let mut ranges = [ColumnRange::default(); 5];
    let mut cursor = SERVING_HEADER_LEN;
    let lens = [
        rows.saturating_mul(4),
        entries.saturating_mul(4),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
    ];
    for (range, len) in ranges.iter_mut().zip(lens) {
        *range = ColumnRange { start: cursor, len };
        cursor += len;
    }
    let mut bytes = Vec::with_capacity(cursor);
    bytes.extend_from_slice(&SERVING_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&SERVING_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(rows as u64).to_le_bytes());
    bytes.extend_from_slice(&(entries as u64).to_le_bytes());
    bytes.extend_from_slice(&frozen.edge_count().to_le_bytes());
    for range in &ranges {
        bytes.extend_from_slice(&(range.start as u64).to_le_bytes());
        bytes.extend_from_slice(&(range.len as u64).to_le_bytes());
    }
    for degree in degrees {
        bytes.extend_from_slice(&degree.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.endpoint.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.rank.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.edge_id.0.to_le_bytes());
    }
    for cold_stamps in cold {
        bytes.extend_from_slice(&cold_stamps.delete_ts.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), cursor);
    let crc = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    let tmp = path.with_extension("serving.tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| StorageError::io_error(format!("serving file write failed: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| StorageError::io_error(format!("serving file swap failed: {e}")))?;
    Ok(())
}

/// Memory-mapped read view of one frozen group serving file.
///
/// Clones share one mapping through the reference count, so snapshotting the
/// handle for a reader is cheap and the mapping outlives serving file
/// replacement while readers hold it. The query surface mirrors
/// [`ImmutableCsr`] so variant dispatch treats both identically; writes are
/// rejected the same way.
#[derive(Debug, Clone)]
pub struct MappedFrozen {
    map: Arc<memmap2::Mmap>,
    columns: ServingColumns,
    offsets: Arc<Vec<u32>>,
    rows: usize,
    entries: usize,
    edge_count: u64,
}

impl MappedFrozen {
    /// Open and validate a serving file. Any structural problem is an error;
    /// callers fall back to the authoritative checkpoint.
    pub fn open(path: &Path) -> StorageResult<Self> {
        let file = File::open(path)
            .map_err(|e| StorageError::io_error(format!("serving file open failed: {e}")))?;
        let map = unsafe { memmap2::Mmap::map(&file) }
            .map_err(|e| StorageError::io_error(format!("serving file map failed: {e}")))?;
        // Ask for transparent huge pages on this read-only scan-heavy
        // mapping. The hint is best-effort: rejection falls back to base
        // pages without failing the open.
        #[cfg(target_os = "linux")]
        let _ = map.advise(memmap2::Advice::HugePage);
        Self::from_map(Arc::new(map))
    }

    /// Open the serving file, rebuilding it from `frozen` when it is missing
    /// or fails validation.
    pub fn open_or_rebuild(path: &Path, frozen: &ImmutableCsr) -> StorageResult<Self> {
        match Self::open(path) {
            Ok(mapped) => Ok(mapped),
            Err(_) => {
                write_serving_file(frozen, path)?;
                Self::open(path)
            }
        }
    }

    fn from_map(map: Arc<memmap2::Mmap>) -> StorageResult<Self> {
        let (rows, entries, edge_count, columns) = parse_header(&map)?;
        let mut offsets = Vec::with_capacity(rows);
        let mut base = 0u32;
        for row in 0..rows {
            let degree = read_u32_le_at(&map, columns.degrees.start + row * 4)?;
            offsets.push(base);
            base = base.saturating_add(degree);
        }
        if base as usize != entries {
            return Err(serving_error(format!(
                "serving row window mismatch: degrees cover {base} entries, payload holds {entries}"
            )));
        }
        Ok(Self {
            map,
            columns,
            offsets: Arc::new(offsets),
            rows,
            entries,
            edge_count,
        })
    }

    /// Row count of the mapped table.
    pub fn vertex_capacity(&self) -> usize {
        self.rows
    }

    /// Live edge count stored at write time.
    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    #[inline]
    fn degree_at(&self, row: usize) -> u32 {
        // Validated at open: degree column covers every row.
        read_u32_le_at(&self.map, self.columns.degrees.start + row * 4)
            .expect("serving degree column validated at open")
    }

    /// Raw little-endian bytes of one serving column.
    ///
    /// Column ranges are validated at open, so the slice always covers the
    /// column. Row scans take these slices once per row and decode the row
    /// window with chunk iteration instead of paying one bounds-checked
    /// scalar read per slot.
    #[inline]
    fn column_bytes(&self, range: ColumnRange) -> &[u8] {
        self.map
            .get(range.start..range.end())
            .expect("serving column range validated at open")
    }

    /// Raw bytes of one entry column restricted to a validated row window.
    #[inline]
    fn row_column_bytes(
        &self,
        range: ColumnRange,
        width: usize,
        start: usize,
        end: usize,
    ) -> &[u8] {
        self.column_bytes(range)
            .get(start * width..end * width)
            .expect("serving row window validated against column ranges")
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
        let endpoints = self.row_column_bytes(self.columns.endpoints, 4, start, end);
        let ranks = self.row_column_bytes(self.columns.ranks, 8, start, end);
        let edge_ids = self.row_column_bytes(self.columns.edge_ids, 8, start, end);
        out.reserve(end - start);
        let rank_chunks = ranks.chunks_exact(8);
        let id_chunks = edge_ids.chunks_exact(8);
        for (endpoint, rank, edge_id) in endpoints
            .chunks_exact(4)
            .zip(rank_chunks)
            .zip(id_chunks)
            .map(|((e, r), id)| {
                (
                    u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
                    i64::from_le_bytes([r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]]),
                    u64::from_le_bytes([id[0], id[1], id[2], id[3], id[4], id[5], id[6], id[7]]),
                )
            })
        {
            out.push(HotNbr {
                endpoint,
                rank,
                edge_id: EdgeId(edge_id),
            });
        }
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
        for delete in deletes.chunks_exact(8) {
            out.push(ColdStamps {
                delete_ts: u64::from_le_bytes([
                    delete[0], delete[1], delete[2], delete[3], delete[4], delete[5], delete[6],
                    delete[7],
                ]),
            });
        }
    }

    #[inline]
    fn endpoint_at(&self, idx: usize) -> u32 {
        read_u32_le_at(&self.map, self.columns.endpoints.start + idx * 4)
            .expect("serving endpoint column validated at open")
    }

    #[inline]
    fn rank_at(&self, idx: usize) -> i64 {
        read_u64_le_at(&self.map, self.columns.ranks.start + idx * 8)
            .expect("serving rank column validated at open") as i64
    }

    #[inline]
    fn edge_id_at(&self, idx: usize) -> EdgeId {
        EdgeId(
            read_u64_le_at(&self.map, self.columns.edge_ids.start + idx * 8)
                .expect("serving edge-id column validated at open"),
        )
    }

    #[inline]
    fn delete_at(&self, idx: usize) -> Timestamp {
        read_u64_le_at(&self.map, self.columns.deletes.start + idx * 8)
            .expect("serving delete column validated at open")
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

    fn row_window(&self, src_vid: u32) -> Option<(usize, usize)> {
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
    fn key_range(&self, start: usize, end: usize, endpoint: u32, rank: i64) -> (usize, usize) {
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

    /// Row length of one vertex. Out-of-range rows report zero.
    pub fn row_degree(&self, src_vid: u32) -> usize {
        let idx = src_vid as usize;
        if idx >= self.rows {
            0
        } else {
            self.degree_at(idx) as usize
        }
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
        let endpoints = self.row_column_bytes(self.columns.endpoints, 4, start, end);
        let ranks = self.row_column_bytes(self.columns.ranks, 8, start, end);
        let edge_ids = self.row_column_bytes(self.columns.edge_ids, 8, start, end);
        let deletes = self.row_column_bytes(self.columns.deletes, 8, start, end);
        out.reserve(end - start);
        let mut rank_chunks = ranks.chunks_exact(8);
        let mut id_chunks = edge_ids.chunks_exact(8);
        let mut delete_chunks = deletes.chunks_exact(8);
        for endpoint in endpoints.chunks_exact(4) {
            let rank = rank_chunks.next().expect("rank column matches row window");
            let edge_id = id_chunks.next().expect("edge-id column matches row window");
            let delete = delete_chunks
                .next()
                .expect("delete column matches row window");
            let nbr = Nbr {
                endpoint: u32::from_le_bytes([endpoint[0], endpoint[1], endpoint[2], endpoint[3]]),
                rank: i64::from_le_bytes([
                    rank[0], rank[1], rank[2], rank[3], rank[4], rank[5], rank[6], rank[7],
                ]),
                edge_id: EdgeId(u64::from_le_bytes([
                    edge_id[0], edge_id[1], edge_id[2], edge_id[3], edge_id[4], edge_id[5],
                    edge_id[6], edge_id[7],
                ])),
                delete_ts: u64::from_le_bytes([
                    delete[0], delete[1], delete[2], delete[3], delete[4], delete[5], delete[6],
                    delete[7],
                ]),
            };
            if nbr.is_alive_at(ts) {
                out.push(nbr);
            }
        }
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
        let endpoints = self.row_column_bytes(self.columns.endpoints, 4, start, end);
        let ranks = self.row_column_bytes(self.columns.ranks, 8, start, end);
        let edge_ids = self.row_column_bytes(self.columns.edge_ids, 8, start, end);
        let deletes = self.row_column_bytes(self.columns.deletes, 8, start, end);
        out.reserve(end - start);
        let mut rank_chunks = ranks.chunks_exact(8);
        let mut id_chunks = edge_ids.chunks_exact(8);
        let mut delete_chunks = deletes.chunks_exact(8);
        for endpoint in endpoints.chunks_exact(4) {
            let rank = rank_chunks.next().expect("rank column matches row window");
            let edge_id = id_chunks.next().expect("edge-id column matches row window");
            let delete = delete_chunks
                .next()
                .expect("delete column matches row window");
            out.push(Nbr {
                endpoint: u32::from_le_bytes([endpoint[0], endpoint[1], endpoint[2], endpoint[3]]),
                rank: i64::from_le_bytes([
                    rank[0], rank[1], rank[2], rank[3], rank[4], rank[5], rank[6], rank[7],
                ]),
                edge_id: EdgeId(u64::from_le_bytes([
                    edge_id[0], edge_id[1], edge_id[2], edge_id[3], edge_id[4], edge_id[5],
                    edge_id[6], edge_id[7],
                ])),
                delete_ts: u64::from_le_bytes([
                    delete[0], delete[1], delete[2], delete[3], delete[4], delete[5], delete[6],
                    delete[7],
                ]),
            });
        }
    }

    /// Visit every physically stored entry of one row without allocating.
    ///
    /// Decodes from one slice per column instead of one scalar read per slot.
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
        let endpoints = self.row_column_bytes(self.columns.endpoints, 4, start, end);
        let ranks = self.row_column_bytes(self.columns.ranks, 8, start, end);
        let edge_ids = self.row_column_bytes(self.columns.edge_ids, 8, start, end);
        let deletes = self.row_column_bytes(self.columns.deletes, 8, start, end);
        let mut rank_chunks = ranks.chunks_exact(8);
        let mut id_chunks = edge_ids.chunks_exact(8);
        let mut delete_chunks = deletes.chunks_exact(8);
        for endpoint in endpoints.chunks_exact(4) {
            let rank = rank_chunks.next().expect("rank column matches row window");
            let edge_id = id_chunks.next().expect("edge-id column matches row window");
            let delete = delete_chunks
                .next()
                .expect("delete column matches row window");
            let nbr = Nbr {
                endpoint: u32::from_le_bytes([endpoint[0], endpoint[1], endpoint[2], endpoint[3]]),
                rank: i64::from_le_bytes([
                    rank[0], rank[1], rank[2], rank[3], rank[4], rank[5], rank[6], rank[7],
                ]),
                edge_id: EdgeId(u64::from_le_bytes([
                    edge_id[0], edge_id[1], edge_id[2], edge_id[3], edge_id[4], edge_id[5],
                    edge_id[6], edge_id[7],
                ])),
                delete_ts: u64::from_le_bytes([
                    delete[0], delete[1], delete[2], delete[3], delete[4], delete[5], delete[6],
                    delete[7],
                ]),
            };
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
        let endpoints = self.row_column_bytes(self.columns.endpoints, 4, start, end);
        let ranks = self.row_column_bytes(self.columns.ranks, 8, start, end);
        let edge_ids = self.row_column_bytes(self.columns.edge_ids, 8, start, end);
        let mut rank_chunks = ranks.chunks_exact(8);
        let mut id_chunks = edge_ids.chunks_exact(8);
        for endpoint in endpoints.chunks_exact(4) {
            let rank = rank_chunks.next().expect("rank column matches row window");
            let edge_id = id_chunks.next().expect("edge-id column matches row window");
            let hot = HotNbr {
                endpoint: u32::from_le_bytes([endpoint[0], endpoint[1], endpoint[2], endpoint[3]]),
                rank: i64::from_le_bytes([
                    rank[0], rank[1], rank[2], rank[3], rank[4], rank[5], rank[6], rank[7],
                ]),
                edge_id: EdgeId(u64::from_le_bytes([
                    edge_id[0], edge_id[1], edge_id[2], edge_id[3], edge_id[4], edge_id[5],
                    edge_id[6], edge_id[7],
                ])),
            };
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

    /// Approximate owned memory: the mapping itself lives in the page cache
    /// and is shared, so only the rebuilt offsets and the handle count.
    pub fn used_memory_size(&self) -> usize {
        self.offsets
            .len()
            .saturating_mul(std::mem::size_of::<u32>())
            .saturating_add(std::mem::size_of::<Self>())
    }

    /// Encoding report for the persisted neighbor columns, same shape as the
    /// heap frozen report.
    ///
    /// Offline use only: decodes every column once. Never call on the query
    /// path; row scans use the bulk slice fills instead.
    pub fn topology_encoding_report(
        &self,
    ) -> Vec<(
        String,
        super::mutable_csr::serialization::TopologyColumnEncoding,
        usize,
        usize,
    )> {
        let mut endpoints = Vec::with_capacity(self.entries);
        let mut edge_ids = Vec::with_capacity(self.entries);
        for idx in 0..self.entries {
            endpoints.push(self.endpoint_at(idx));
            edge_ids.push(self.edge_id_at(idx).0);
        }
        let degrees: Vec<u32> = (0..self.rows).map(|row| self.degree_at(row)).collect();
        let (endpoint_choice, _) = encode_topology_u32_column(&endpoints);
        let (edge_id_choice, _) = encode_topology_u64_column(&edge_ids);
        let (degrees_choice, _) = encode_topology_u32_column(&degrees);
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

    /// Iterate timestamp-visible entries across all rows without materializing
    /// any row: each item decodes one slot on demand from the mapping.
    pub fn iter(&self, ts: Timestamp) -> MappedFrozenIterator {
        MappedFrozenIterator::filtered(self.clone(), ts)
    }

    /// Iterate every physically stored entry, including tombstoned ones.
    pub fn iter_all(&self) -> MappedFrozenIterator {
        MappedFrozenIterator::all(self.clone())
    }

    /// Iterate timestamp-visible entries of one row without allocating.
    ///
    /// Detached-snapshot contract: the serving file embeds the full
    /// create/delete stamp history, so point-in-time reads inside the
    /// snapshot are decided from the embedded stamps alone, without the
    /// table version authority. Live tables must never use this path for
    /// visibility; they decide through `EdgeStore::is_visible`.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> MappedFrozenRowIter {
        let (start, end) = self.row_window(src_vid).unwrap_or((0, 0));
        MappedFrozenRowIter {
            view: self.clone(),
            idx: start,
            end,
            ts,
        }
    }

    /// Authoritative checkpoint bytes rebuilt from the mapping: same version
    /// 2 layout as the heap frozen dump, so a mapped group flushes
    /// indistinguishably from a heap frozen group.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based authoritative dump without an intermediate owned buffer.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let mut scratch = super::mutable_csr::persistence::CsrDumpScratch::new();
        self.dump_into_with_scratch(out, &mut scratch);
    }

    /// Authoritative dump reusing caller-owned column buffers.
    pub fn dump_into_with_scratch(
        &self,
        out: &mut Vec<u8>,
        scratch: &mut super::mutable_csr::persistence::CsrDumpScratch,
    ) {
        let start = out.len();
        out.extend_from_slice(&IMMUTABLE_CSR_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.rows as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.entries as u64).to_le_bytes());
        let mut degrees = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            degrees.push(self.degree_at(row));
        }
        let (_, degrees_payload) = encode_topology_u32_column(&degrees);
        out.extend_from_slice(&degrees_payload);
        let mut hot = Vec::with_capacity(self.entries);
        let mut cold = Vec::with_capacity(self.entries);
        for idx in 0..self.entries {
            hot.push(HotNbr {
                endpoint: self.endpoint_at(idx),
                rank: self.rank_at(idx),
                edge_id: self.edge_id_at(idx),
            });
            cold.push(ColdStamps {
                delete_ts: self.delete_at(idx),
            });
        }
        scratch.fill_from_split(&hot, &cold);
        let (_, endpoints_payload) = encode_topology_u32_column(scratch.endpoints());
        out.extend_from_slice(&endpoints_payload);
        let (_, ranks_payload) = encode_topology_i64_column(scratch.ranks());
        out.extend_from_slice(&ranks_payload);
        let (_, edge_ids_payload) = encode_topology_u64_column(scratch.edge_ids());
        out.extend_from_slice(&edge_ids_payload);
        let (_, delete_payload) = encode_topology_u64_column(scratch.deletes());
        out.extend_from_slice(&delete_payload);
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }
}

fn mapped_frozen_error() -> StorageError {
    StorageError::invalid_operation(
        "mapped frozen CSR group rejects writes: unfreeze the group before writing".to_string(),
    )
}

impl super::CsrBase for MappedFrozen {
    fn vertex_capacity(&self) -> usize {
        MappedFrozen::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        MappedFrozen::edge_count(self)
    }

    fn dump(&self) -> Vec<u8> {
        MappedFrozen::dump(self)
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        MappedFrozen::dump_into(self, out);
    }

    fn load(&mut self, _data: &[u8]) -> StorageResult<()> {
        // Mapped views load from serving files, never from byte payloads:
        // callers open a fresh view and replace the variant instead.
        Err(serving_error(
            "mapped frozen view loads from a serving file, not from bytes".to_string(),
        ))
    }
}

impl super::MutableCsrTrait for MappedFrozen {
    fn insert_edge(
        &mut self,
        _src_vid: u32,
        _dst: VertexId,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<()> {
        Err(mapped_frozen_error())
    }

    fn delete_edge(
        &mut self,
        _src_vid: u32,
        _edge_id: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(mapped_frozen_error())
    }

    fn delete_edge_by_dst(&mut self, _src_vid: u32, _dst: VertexId, _ts: Timestamp) -> usize {
        0
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        MappedFrozen::locate_edge(self, src_vid, edge_id)
    }

    fn delete_edge_at_position(
        &mut self,
        _src_vid: u32,
        _position: EdgePosition,
        _expected: EdgeId,
        _ts: Timestamp,
    ) -> StorageResult<bool> {
        Err(mapped_frozen_error())
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
        Err(mapped_frozen_error())
    }

    fn revert_delete_by_offset(&mut self, _src_vid: u32, _offset: i32, _ts: Timestamp) -> bool {
        false
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        MappedFrozen::nbr_at_offset(self, src_vid, offset)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        MappedFrozen::get_edge_physical(self, src_vid, dst)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        MappedFrozen::physical_edges_of(self, src_vid)
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        MappedFrozen::fill_physical_into(self, src_vid, out);
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        MappedFrozen::has_physical_entries(self, vid)
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        MappedFrozen::primary_contains(self, src_vid, edge_id)
    }

    fn rollback_insert(&mut self, _src_vid: u32, _edge_id: EdgeId) -> bool {
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
        MappedFrozen::get_edge(self, src_vid, dst, ts)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        MappedFrozen::edges_of(self, src_vid, ts)
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
        MappedFrozen::vertex_census(self, vid)
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
        MappedFrozen::used_memory_size(self)
    }
}

/// Owned per-row iterator over a mapped view: holds a counted handle instead
/// of borrowed slices, so the mapping stays alive for the whole walk and no
/// row is ever materialized.
#[derive(Debug, Clone)]
pub struct MappedFrozenRowIter {
    view: MappedFrozen,
    idx: usize,
    end: usize,
    ts: Timestamp,
}

impl Iterator for MappedFrozenRowIter {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        while self.idx < self.end {
            let nbr = self.view.slot_at(self.idx)?;
            self.idx += 1;
            if nbr.is_alive_at(self.ts) {
                return Some(nbr);
            }
        }
        None
    }
}

/// Owned whole-table iterator over a mapped view, yielding local vertex ids.
/// Timestamp-filtered unless built for the physical walk.
#[derive(Debug, Clone)]
pub struct MappedFrozenIterator {
    view: MappedFrozen,
    ts: Timestamp,
    include_deleted: bool,
    row: usize,
    idx: usize,
}

impl MappedFrozenIterator {
    fn filtered(view: MappedFrozen, ts: Timestamp) -> Self {
        let start = view.offsets.first().copied().unwrap_or(0) as usize;
        Self {
            view,
            ts,
            include_deleted: false,
            row: 0,
            idx: start,
        }
    }

    fn all(view: MappedFrozen) -> Self {
        let mut iter = Self::filtered(view, 0);
        iter.include_deleted = true;
        iter
    }
}

impl Iterator for MappedFrozenIterator {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        while self.row < self.view.rows {
            let end = self.view.offsets[self.row] as usize + self.view.degree_at(self.row) as usize;
            while self.idx < end {
                let nbr = self.view.slot_at(self.idx)?;
                self.idx += 1;
                if self.include_deleted || nbr.is_alive_at(self.ts) {
                    return Some((VertexId::from_int64(self.row as i64), nbr));
                }
            }
            self.row += 1;
            if self.row < self.view.rows {
                self.idx = self.view.offsets[self.row] as usize;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::MutableCsrTrait;

    fn sample_frozen() -> ImmutableCsr {
        use super::super::MutableCsr;
        let mut csr = MutableCsr::with_capacity(8, 64);
        let key = |endpoint: u32| VertexId::edge_endpoint_key(endpoint, 0);
        csr.insert_edge(0, key(30), EdgeId(1), 1).unwrap();
        csr.insert_edge(0, key(10), EdgeId(2), 1).unwrap();
        csr.insert_edge(0, key(20), EdgeId(3), 2).unwrap();
        csr.delete_edge(0, EdgeId(3), 5).unwrap();
        csr.insert_edge(3, key(12), EdgeId(4), 2).unwrap();
        ImmutableCsr::pack_from_mutable(&csr)
    }

    fn serving_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "linkrs_serving_test_{name}_{}.bin",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn mapped_reads_match_heap_frozen() {
        let frozen = sample_frozen();
        let path = serving_path("match");
        write_serving_file(&frozen, &path).unwrap();
        let mapped = MappedFrozen::open(&path).unwrap();
        assert_eq!(mapped.vertex_capacity(), frozen.vertex_capacity());
        assert_eq!(mapped.edge_count(), frozen.edge_count());
        for vid in 0..8u32 {
            assert_eq!(mapped.physical_edges_of(vid), frozen.physical_edges_of(vid));
            for ts in [1u64, 2, 4, 5, 6] {
                assert_eq!(mapped.edges_of(vid, ts), frozen.edges_of(vid, ts));
            }
            assert_eq!(mapped.vertex_census(vid), frozen.vertex_census(vid));
            assert_eq!(mapped.row_degree(vid), frozen.row_degree(vid));
        }
        let key = VertexId::edge_endpoint_key(10, 0);
        assert_eq!(mapped.get_edge(0, key, 6), frozen.get_edge(0, key, 6));
        assert_eq!(
            mapped.get_edge_physical(0, key),
            frozen.get_edge_physical(0, key)
        );
        assert_eq!(
            mapped.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr),
            frozen.locate_edge(0, EdgeId(1)).map(|(_, nbr)| nbr)
        );
        assert!(mapped.primary_contains(0, EdgeId(1)));
        // Same authoritative bytes as the heap dump: flushes are identical.
        assert_eq!(mapped.dump(), frozen.dump());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mapped_iterators_match_heap_counts() {
        let frozen = sample_frozen();
        let path = serving_path("iter");
        write_serving_file(&frozen, &path).unwrap();
        let mapped = MappedFrozen::open(&path).unwrap();
        assert_eq!(mapped.iter(6).count(), frozen.iter(6).count());
        assert_eq!(mapped.iter_all().count(), frozen.iter_all().count());
        let mapped_row: Vec<Nbr> = mapped.iter_edges_of(0, 6).collect();
        let heap_row: Vec<Nbr> = frozen.iter_edges_of(0, 6).collect();
        assert_eq!(mapped_row, heap_row);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hugepage_hint_keeps_serving_openable() {
        let frozen = sample_frozen();
        let path = serving_path("hugepage");
        write_serving_file(&frozen, &path).unwrap();
        // The open path carries a best-effort huge-page hint. It must never
        // fail the open: rejection falls back to base pages.
        let mapped = MappedFrozen::open(&path).unwrap();
        assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mapped_writes_are_rejected() {
        let frozen = sample_frozen();
        let path = serving_path("rejected");
        write_serving_file(&frozen, &path).unwrap();
        let mut mapped = MappedFrozen::open(&path).unwrap();
        let key = VertexId::edge_endpoint_key(99, 0);
        assert!(mapped.insert_edge(0, key, EdgeId(100), 9).is_err());
        assert!(mapped.delete_edge(0, EdgeId(1), 9).is_err());
        assert_eq!(mapped.delete_edge_by_dst(0, key, 9), 0);
        assert!(!mapped.rollback_insert(0, EdgeId(1)));
        assert!(!mapped.revert_delete_by_edge_id(0, EdgeId(1), 9));
        assert_eq!(mapped.reclaimable_count(0, 9), 0);
        assert!(!mapped.vertex_needs_compact(0, 9));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_falls_back_to_rebuild() {
        let frozen = sample_frozen();
        let path = serving_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(MappedFrozen::open(&path).is_err());
        let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
        assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_serving_file_is_rejected() {
        let frozen = sample_frozen();
        let path = serving_path("corrupt");
        write_serving_file(&frozen, &path).unwrap();
        for mutate in [
            |bytes: &mut Vec<u8>| bytes[0] ^= 0xff,
            |bytes: &mut Vec<u8>| bytes[4] = 99,
            |bytes: &mut Vec<u8>| bytes.truncate(bytes.len() / 2),
            |bytes: &mut Vec<u8>| bytes.push(0),
        ] {
            let mut bytes = std::fs::read(&path).unwrap();
            mutate(&mut bytes);
            std::fs::write(&path, &bytes).unwrap();
            assert!(MappedFrozen::open(&path).is_err());
        }
        // A bad cache rebuilds cleanly from the authority.
        let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
        assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn payload_bit_flip_fails_checksum() {
        let frozen = sample_frozen();
        let path = serving_path("payload_crc");
        write_serving_file(&frozen, &path).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > SERVING_HEADER_LEN + SERVING_CRC_LEN);
        let mid = SERVING_HEADER_LEN + 1;
        bytes[mid] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        let err = MappedFrozen::open(&path).expect_err("payload corruption must fail");
        assert!(err.to_string().contains("CRC"));
        let mapped = MappedFrozen::open_or_rebuild(&path, &frozen).unwrap();
        assert_eq!(mapped.physical_edges_of(0), frozen.physical_edges_of(0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_table_serves() {
        use super::super::MutableCsr;
        let frozen = ImmutableCsr::pack_from_mutable(&MutableCsr::with_capacity(4, 16));
        let path = serving_path("empty");
        write_serving_file(&frozen, &path).unwrap();
        let mapped = MappedFrozen::open(&path).unwrap();
        assert_eq!(mapped.edge_count(), 0);
        assert!(mapped.edges_of(0, 1).is_empty());
        assert_eq!(mapped.dump(), frozen.dump());
        let _ = std::fs::remove_file(&path);
    }
}
