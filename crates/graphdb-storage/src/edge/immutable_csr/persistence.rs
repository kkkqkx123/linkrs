use super::super::csr_trait::CsrBase;
use super::super::mutable_csr::serialization::{
    decode_topology_i64_column, decode_topology_u32_column, decode_topology_u64_column,
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
    TopologyColumnEncoding,
};
use super::super::{ColdStamps, EdgeId, HotNbr, INVALID_EDGE_ID};
use super::ImmutableCsr;
use crate::persistence::read_u64_le;
use graphdb_core::{StorageError, StorageResult};

impl ImmutableCsr {
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

    /// Dump to bytes.
    ///
    /// Format:
    /// - rows (u64)
    /// - edge_count (u64)
    /// - entries_len (u64)
    /// - encoded degrees column
    /// - encoded endpoints column
    /// - encoded ranks column
    /// - encoded edge ids column
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
        let mut scratch = super::super::mutable_csr::persistence::CsrDumpScratch::new();
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
        scratch: &mut super::super::mutable_csr::persistence::CsrDumpScratch,
    ) {
        let start = out.len();
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
        let (_, delete_payload) = encode_topology_u64_column(scratch.deletes());
        out.extend_from_slice(&delete_payload);
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    /// Load from bytes.
    ///
    /// Rejects short headers, CRC mismatches, column
    /// length mismatches, out-of-range row windows, edge count mismatches
    /// and trailing bytes.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 32 {
            return Err(StorageError::deserialize_error(
                "frozen CSR data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "frozen CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;
        let mut offset = 0usize;
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
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != entries_len
            || ranks.len() != entries_len
            || edge_ids.len() != entries_len
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
