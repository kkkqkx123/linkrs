use super::super::{ColdStamps, EdgeId, HotNbr, INVALID_EDGE_ID};
use super::live_set::LiveKeySet;
use super::overflow::OverflowStorage;
use super::serialization::{
    decode_overflow_chunk, decode_topology_i64_column, decode_topology_u32_column,
    decode_topology_u64_column, encode_overflow_chunk, encode_topology_i64_column,
    encode_topology_u32_column, encode_topology_u64_column, TopologyColumnEncoding,
    MUTABLE_CSR_FORMAT_VERSION,
};
use super::write::EdgePosition;
use super::MutableCsr;
use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

/// Reusable neighbor-column buffers for checkpoint dumps.
///
/// One scratch serves a whole checkpoint: each group clears and refills the
/// buffers instead of allocating four temporary columns, so repeated dumps
/// keep peak allocation to one column set.
#[derive(Debug, Default)]
pub struct CsrDumpScratch {
    endpoints: Vec<u32>,
    ranks: Vec<i64>,
    edge_ids: Vec<u64>,
    deletes: Vec<u64>,
}

impl CsrDumpScratch {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn endpoints(&self) -> &[u32] {
        &self.endpoints
    }

    pub(crate) fn ranks(&self) -> &[i64] {
        &self.ranks
    }

    pub(crate) fn edge_ids(&self) -> &[u64] {
        &self.edge_ids
    }

    pub(crate) fn deletes(&self) -> &[u64] {
        &self.deletes
    }

    pub(crate) fn fill_from_split(&mut self, hot: &[HotNbr], cold: &[ColdStamps]) {
        debug_assert_eq!(hot.len(), cold.len());
        self.endpoints.clear();
        self.ranks.clear();
        self.edge_ids.clear();
        self.deletes.clear();
        self.endpoints.reserve(hot.len());
        self.ranks.reserve(hot.len());
        self.edge_ids.reserve(hot.len());
        self.deletes.reserve(hot.len());
        for slot in hot {
            self.endpoints.push(slot.endpoint);
            self.ranks.push(slot.rank);
            self.edge_ids.push(slot.edge_id.0);
        }
        for stamp in cold {
            self.deletes.push(stamp.delete_ts);
        }
    }
}

impl MutableCsr {
    /// Dump to bytes, current encoded version.
    ///
    /// Header columns (offsets, degrees, capacities) and primary neighbor
    /// columns (endpoints, ranks, edge ids, stamps) persist through the
    /// integer column path with per-column bit-packing or run-length
    /// encoding and a narrow plain fallback. Overflow chunks use the same
    /// column path per chunk instead of plain neighbor records.
    ///
    /// Format:
    /// - format marker (u32 = encoded or raw mode, see serialization)
    /// - vertex_capacity (u64)
    /// - edge_count (u64)
    /// - primary_len (u64)
    /// - overflow_chunk_edges (u64)
    /// - encoded offsets column
    /// - encoded degrees column
    /// - encoded capacities column
    /// - encoded endpoints column
    /// - encoded ranks column
    /// - encoded edge ids column
    /// - encoded create stamps column
    /// - encoded delete stamps column
    /// - per-vertex overflow chunks (column-encoded per chunk)
    /// - payload CRC32 (u32, little-endian, over all preceding bytes)
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based dump into `out` without an intermediate owned buffer.
    /// Byte-identical to `dump`; checkpoint writes use this so no whole-group
    /// clone or temporary dump buffer is needed.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let mut scratch = CsrDumpScratch::new();
        self.dump_into_with_scratch(out, &mut scratch);
    }

    /// Dump reusing caller-owned column buffers.
    ///
    /// Same bytes as [`Self::dump_into`] but the four neighbor-column
    /// buffers are cleared and refilled instead of reallocated, so a
    /// checkpoint over many groups pays one allocation per column instead
    /// of one per group. The scratch holds no state between calls.
    pub fn dump_into_with_scratch(&self, out: &mut Vec<u8>, scratch: &mut CsrDumpScratch) {
        let start = out.len();
        out.extend_from_slice(&MUTABLE_CSR_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.rows.adj_offsets.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.hot_list.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.overflow_chunk_edges as u64).to_le_bytes());

        let (_, offsets_payload) = encode_topology_u32_column(&self.rows.adj_offsets);
        out.extend_from_slice(&offsets_payload);
        let (_, degrees_payload) = encode_topology_u32_column(&self.rows.degrees);
        out.extend_from_slice(&degrees_payload);
        let (_, caps_payload) = encode_topology_u32_column(&self.rows.primary_capacities);
        out.extend_from_slice(&caps_payload);

        scratch.fill_from_split(&self.hot_list, &self.cold_list);
        let (_, endpoints_payload) = encode_topology_u32_column(&scratch.endpoints);
        out.extend_from_slice(&endpoints_payload);
        let (_, ranks_payload) = encode_topology_i64_column(&scratch.ranks);
        out.extend_from_slice(&ranks_payload);
        let (_, edge_ids_payload) = encode_topology_u64_column(&scratch.edge_ids);
        out.extend_from_slice(&edge_ids_payload);
        let (_, delete_payload) = encode_topology_u64_column(&scratch.deletes);
        out.extend_from_slice(&delete_payload);

        for vid in 0..self.rows.adj_offsets.len() {
            let chunks = self.overflow_chunks.get(vid as u32);
            out.extend_from_slice(&(chunks.map_or(0, Vec::len) as u32).to_le_bytes());
            if let Some(chunks) = chunks {
                for chunk in chunks {
                    encode_overflow_chunk(chunk, out);
                }
            }
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    /// Encoding report for the persisted topology columns.
    ///
    /// Measures the winning encoding per column without changing in-memory
    /// state. Neighbor and edge-id columns are reported first, offset and
    /// length columns follow; all use the integer column path only.
    pub fn topology_encoding_report(&self) -> Vec<(String, TopologyColumnEncoding, usize, usize)> {
        let endpoints: Vec<u32> = self.hot_list.iter().map(|hot| hot.endpoint).collect();
        let edge_ids: Vec<u64> = self.hot_list.iter().map(|hot| hot.edge_id.0).collect();
        let (endpoint_choice, _) = encode_topology_u32_column(&endpoints);
        let (edge_id_choice, _) = encode_topology_u64_column(&edge_ids);
        let (offsets_choice, _) = encode_topology_u32_column(&self.rows.adj_offsets);
        let (degrees_choice, _) = encode_topology_u32_column(&self.rows.degrees);
        let (caps_choice, _) = encode_topology_u32_column(&self.rows.primary_capacities);
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
                "offsets".to_string(),
                offsets_choice.encoding,
                offsets_choice.plain_bytes,
                offsets_choice.encoded_bytes,
            ),
            (
                "lengths".to_string(),
                degrees_choice.encoding,
                degrees_choice.plain_bytes,
                degrees_choice.encoded_bytes,
            ),
            (
                "capacities".to_string(),
                caps_choice.encoding,
                caps_choice.plain_bytes,
                caps_choice.encoded_bytes,
            ),
        ]
    }

    /// Load from bytes. Only the single format marker is accepted; any
    /// other marker is corrupt. Strict validation below is shared by every
    /// payload that passes the marker gate.
    /// The trailing CRC32 is verified before any parsing so a truncated or
    /// bit-rotted checkpoint fails fast instead of decoding garbage.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 40 {
            return Err(StorageError::deserialize_error(
                "CSR data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;

        self.reset_baseline_counters();

        let mut offset = 0usize;

        let format_version = read_u32_le(data, &mut offset)?;
        if format_version != MUTABLE_CSR_FORMAT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported mutable CSR format version: {format_version}"
            )));
        }
        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let primary_len = read_u64_le(data, &mut offset)? as usize;
        let overflow_chunk_edges = read_u64_le(data, &mut offset)? as usize;
        if overflow_chunk_edges == 0 {
            return Err(StorageError::deserialize_error(
                "Mutable CSR overflow chunk size must be greater than zero",
            ));
        }

        let (adj_offsets, degrees, primary_capacities) = (
            decode_topology_u32_column(data, &mut offset)?,
            decode_topology_u32_column(data, &mut offset)?,
            decode_topology_u32_column(data, &mut offset)?,
        );
        if adj_offsets.len() != vertex_capacity
            || degrees.len() != vertex_capacity
            || primary_capacities.len() != vertex_capacity
        {
            return Err(StorageError::deserialize_error(
                "Mutable CSR header column length mismatch",
            ));
        }
        let (endpoints, ranks, edge_ids, delete_stamps) = (
            decode_topology_u32_column(data, &mut offset)?,
            decode_topology_i64_column(data, &mut offset)?,
            decode_topology_u64_column(data, &mut offset)?,
            decode_topology_u64_column(data, &mut offset)?,
        );
        if endpoints.len() != primary_len
            || ranks.len() != primary_len
            || edge_ids.len() != primary_len
            || delete_stamps.len() != primary_len
        {
            return Err(StorageError::deserialize_error(
                "Mutable CSR neighbor column length mismatch",
            ));
        }
        let mut hot_list = Vec::with_capacity(primary_len);
        let mut cold_list = Vec::with_capacity(primary_len);
        for index in 0..primary_len {
            hot_list.push(HotNbr {
                endpoint: endpoints[index],
                rank: ranks[index],
                edge_id: EdgeId(edge_ids[index]),
            });
            cold_list.push(ColdStamps {
                delete_ts: delete_stamps[index],
            });
        }

        // Addressing consistency: every row window must land inside the
        // neighbor list and the degree must fit the reserved capacity. A
        // length-aligned but out-of-range payload is damage, rejected here
        // instead of panicking on the hot path later.
        for vid in 0..vertex_capacity {
            let offset = adj_offsets[vid] as usize;
            let degree = degrees[vid] as usize;
            let capacity = primary_capacities[vid] as usize;
            if degree > capacity
                || offset.saturating_add(degree) > primary_len
                || offset.saturating_add(capacity) > primary_len
            {
                return Err(StorageError::deserialize_error(format!(
                    "Mutable CSR row {} out of range: offset={} degree={} capacity={} primary_len={}",
                    vid, offset, degree, capacity, primary_len
                )));
            }
        }

        let mut overflow_chunks = OverflowStorage::new();
        let mut overflow_capacity = 0usize;
        let mut live_keys: Vec<Vec<((u32, i64), EdgePosition)>> = vec![Vec::new(); vertex_capacity];
        let mut recomputed: u64 = 0;
        for (hot, cold) in hot_list.iter().zip(cold_list.iter()) {
            if hot.edge_id != INVALID_EDGE_ID && cold.is_live() {
                recomputed += 1;
            }
        }
        for (vid, keys) in live_keys.iter_mut().enumerate() {
            let row_offset = adj_offsets[vid] as usize;
            let degree = degrees[vid] as usize;
            for i in 0..degree {
                match (hot_list.get(row_offset + i), cold_list.get(row_offset + i)) {
                    (Some(hot), Some(cold)) if hot.edge_id != INVALID_EDGE_ID && cold.is_live() => {
                        keys.push((
                            (hot.endpoint, hot.rank),
                            EdgePosition::Primary { slot: i as u32 },
                        ));
                    }
                    _ => {}
                }
            }
        }
        for (vid, row_keys) in live_keys.iter_mut().enumerate() {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for chunk_idx in 0..chunk_count {
                let chunk = decode_overflow_chunk(data, &mut offset)?;
                if chunk.len() > overflow_chunk_edges {
                    return Err(StorageError::deserialize_error(
                        "Mutable CSR overflow chunk exceeds configured chunk size",
                    ));
                }
                for (slot_idx, (hot, cold)) in
                    chunk.hot_slice().iter().zip(chunk.cold_slice()).enumerate()
                {
                    if hot.edge_id != INVALID_EDGE_ID && cold.is_live() {
                        recomputed += 1;
                        row_keys.push((
                            (hot.endpoint, hot.rank),
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        ));
                    }
                }
                overflow_capacity = overflow_capacity.saturating_add(chunk.capacity().max(1));
                chunks.push(chunk);
            }
            if !chunks.is_empty() {
                overflow_chunks.insert(vid as u32, chunks);
            }
        }

        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "Mutable CSR edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }

        self.total_edge_capacity = hot_list.len().saturating_add(overflow_capacity);
        self.rows.adj_offsets = adj_offsets;
        self.rows.degrees = degrees;
        self.rows.primary_capacities = primary_capacities;
        self.overflow_chunks = overflow_chunks;
        self.overflow_chunk_edges = overflow_chunk_edges;
        self.hot_list = hot_list;
        self.cold_list = cold_list;
        self.edge_count = edge_count;
        self.reuse_hint = vec![super::core::REUSE_HINT_UNKNOWN; vertex_capacity];
        self.live_counts = vec![0; vertex_capacity];
        self.tombstone_counts = vec![0; vertex_capacity];
        // Row order is memory-only and never persists: every load starts
        // unsorted so later sorts re-observe the order. Plan caches must not
        // reuse this flag across restarts.
        self.primary_sorted = vec![false; vertex_capacity];
        self.live_sets.clear();
        self.live_sets.ensure_capacity(vertex_capacity);
        for (vid, keys) in live_keys.into_iter().enumerate() {
            // Narrow rows stay set-free; only wide rows pay for the index.
            if keys.len() > super::live_set::LIVE_SET_WIDTH_BOUND {
                self.live_sets
                    .insert(vid as u32, LiveKeySet::from_positions(keys));
            }
        }
        // Rebuild incremental live/tombstone counts from loaded data.
        for vid in 0..vertex_capacity {
            let degree = self.rows.degrees[vid] as usize;
            let offset = self.rows.adj_offsets[vid] as usize;
            let mut live = 0u32;
            let mut tombstones = 0u32;
            for i in 0..degree {
                if let Some(cold) = self.cold_list.get(offset + i) {
                    if cold.is_live() {
                        live += 1;
                    } else {
                        tombstones += 1;
                    }
                }
            }
            if let Some(chunks) = self.overflow_chunks.get(vid as u32) {
                for chunk in chunks {
                    for cold in chunk.cold_slice() {
                        if cold.is_live() {
                            live += 1;
                        } else {
                            tombstones += 1;
                        }
                    }
                }
            }
            self.live_counts[vid] = live;
            self.tombstone_counts[vid] = tombstones;
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in mutable CSR payload",
            ));
        }

        Ok(())
    }
}
