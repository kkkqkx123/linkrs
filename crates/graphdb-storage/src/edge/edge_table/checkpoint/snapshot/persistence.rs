use super::format::snapshot_error;
use super::MappedFrozen;
use crate::edge::mutable_csr::serialization::{
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use crate::edge::{ColdStamps, HotNbr};
use graphdb_core::StorageResult;

impl MappedFrozen {
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
        crate::edge::mutable_csr::serialization::TopologyColumnEncoding,
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

    /// Authoritative checkpoint bytes rebuilt from the mapping: same
    /// layout as the heap frozen dump, so a mapped group flushes
    /// indistinguishably from a heap frozen group. Valued sidecars rebuild
    /// the value column and validity bytes the same way.
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based authoritative dump without an intermediate owned buffer.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        let mut scratch = crate::edge::mutable_csr::persistence::CsrDumpScratch::new();
        self.dump_into_with_scratch(out, &mut scratch);
    }

    /// Authoritative dump reusing caller-owned column buffers.
    pub fn dump_into_with_scratch(
        &self,
        out: &mut Vec<u8>,
        scratch: &mut crate::edge::mutable_csr::persistence::CsrDumpScratch,
    ) {
        let start = out.len();
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
        let valued = self.has_valued_entries();
        out.extend_from_slice(&(u64::from(valued)).to_le_bytes());
        if valued {
            let mut values = Vec::with_capacity(self.entries);
            for idx in 0..self.entries {
                values.push(self.value_at(idx));
            }
            let (_, values_payload) = encode_topology_u64_column(&values);
            out.extend_from_slice(&values_payload);
            let validity = self.column_bytes(self.columns.validity);
            out.extend_from_slice(&(validity.len() as u64).to_le_bytes());
            out.extend_from_slice(validity);
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }
}

impl crate::edge::CsrBase for MappedFrozen {
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
        // Mapped views load from snapshot files, never from byte payloads:
        // callers open a fresh view and replace the variant instead.
        Err(snapshot_error(
            "mapped frozen view loads from a snapshot file, not from bytes".to_string(),
        ))
    }
}
