//! Persistence for `SingleMutableCsr`: dump and load with CRC32 trailer.

use crate::persistence::read_u64_le;
use linkrs_core::{StorageError, StorageResult};

use super::super::mutable_csr::serialization::{
    decode_topology_i64_column, decode_topology_u32_column, decode_topology_u64_column,
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use super::super::{ColdStamps, EdgeId, HotNbr, INVALID_EDGE_ID};
use super::{SingleMutableCsr, SingleSegment};

impl SingleMutableCsr {
    /// Dump with integer column encoding for neighbor and edge-id columns.
    ///
    /// Offsets are trivial for the single-edge layout (slot index equals row),
    /// so only neighbor, rank, edge-id and stamp columns go through the
    /// column path with a narrow plain fallback.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based dump into `out`, byte-identical to `dump`. The payload
    /// carries a trailing CRC32 trailer verified on load.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        let slot_count = self.vertex_capacity;
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(slot_count as u64).to_le_bytes());

        {
            let endpoints: Vec<u32> = (0..slot_count)
                .map(|i| self.hot_at(i).map(|h| h.endpoint).unwrap_or(0))
                .collect();
            let (_, endpoints_payload) = encode_topology_u32_column(&endpoints);
            out.extend_from_slice(&endpoints_payload);
        }
        {
            let ranks: Vec<i64> = (0..slot_count)
                .map(|i| self.hot_at(i).map(|h| h.rank).unwrap_or(0))
                .collect();
            let (_, ranks_payload) = encode_topology_i64_column(&ranks);
            out.extend_from_slice(&ranks_payload);
        }
        {
            let edge_ids: Vec<u64> = (0..slot_count)
                .map(|i| {
                    self.hot_at(i)
                        .map(|h| h.edge_id.0)
                        .unwrap_or(INVALID_EDGE_ID.0)
                })
                .collect();
            let (_, edge_ids_payload) = encode_topology_u64_column(&edge_ids);
            out.extend_from_slice(&edge_ids_payload);
        }
        {
            let delete_stamps: Vec<u64> = (0..slot_count)
                .map(|i| {
                    self.cold_at(i)
                        .map(|c| c.delete_ts)
                        .unwrap_or(ColdStamps::dead_gap().delete_ts)
                })
                .collect();
            let (_, delete_payload) = encode_topology_u64_column(&delete_stamps);
            out.extend_from_slice(&delete_payload);
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    /// Reserved slot memory plus struct overhead.
    ///
    /// Per-shape accounting by design: the single-slot layout owns no offset
    /// arrays, live sets or overflow maps, so its shard-level total is
    /// narrower than the multi-edge layout. Cross-shape comparison belongs at
    /// the table layer after the shared authority estimate is added, never at
    /// the shard level directly.
    pub fn used_memory_size(&self) -> usize {
        self.sparse_memory_bytes() + std::mem::size_of::<Self>()
    }

    /// Load the single persisted layout. The trailing
    /// CRC32 is verified before any parsing.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 16 {
            return Err(StorageError::deserialize_error(
                "Single CSR data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "Single CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;

        let mut offset = 0usize;

        let edge_count = read_u64_le(data, &mut offset)?;
        let slot_count = read_u64_le(data, &mut offset)? as usize;

        let endpoints = decode_topology_u32_column(data, &mut offset)?;
        let ranks = decode_topology_i64_column(data, &mut offset)?;
        let edge_ids = decode_topology_u64_column(data, &mut offset)?;
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != slot_count
            || ranks.len() != slot_count
            || edge_ids.len() != slot_count
            || delete_stamps.len() != slot_count
        {
            return Err(StorageError::deserialize_error(
                "Single CSR column length mismatch",
            ));
        }
        let mut new_csr = SingleMutableCsr::with_capacity(slot_count);
        let mut recomputed = 0u64;
        for index in 0..slot_count {
            let hot = HotNbr {
                endpoint: endpoints[index],
                rank: ranks[index],
                edge_id: EdgeId(edge_ids[index]),
            };
            let cold = ColdStamps {
                delete_ts: delete_stamps[index],
            };
            let (seg, off) = Self::locate(index);
            if hot.edge_id != INVALID_EDGE_ID {
                let segment =
                    new_csr.segments[seg].get_or_insert_with(|| Box::new(SingleSegment::fresh()));
                segment.hot[off] = hot;
                segment.cold[off] = cold;
                new_csr.set_present(index, true);
                if cold.is_live() {
                    recomputed += 1;
                }
            }
        }
        new_csr.edge_count = edge_count;
        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "Single CSR edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "Single CSR has trailing bytes: unsupported format",
            ));
        }

        *self = new_csr;

        Ok(())
    }
}
