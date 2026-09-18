use std::sync::atomic::Ordering;

use super::super::{EdgeId, Nbr};
use super::overflow::OverflowStorage;
use super::serialization::{
    decode_overflow_chunk, decode_topology_i64_column, decode_topology_u32_column,
    decode_topology_u64_column, encode_overflow_chunk, encode_topology_i64_column,
    encode_topology_u32_column, encode_topology_u64_column, TopologyColumnEncoding,
    MUTABLE_CSR_FORMAT_VERSION,
};
use super::MutableCsr;
use crate::persistence::{read_u32_le, read_u64_le};
use graphdb_core::{StorageError, StorageResult};

impl MutableCsr {
    /// Dump to bytes, version 4.
    ///
    /// Header columns (offsets, degrees, capacities) and primary neighbor
    /// columns (endpoints, ranks, edge ids, stamps) persist through the
    /// integer column path with per-column bit-packing or run-length
    /// encoding and a narrow plain fallback. Overflow chunks use the same
    /// column path per chunk instead of plain neighbor records. Version 3
    /// and older payloads are rejected on load, never converted.
    ///
    /// Format:
    /// - format_version (u32 = 4)
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
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    /// Borrow-based dump into `out` without an intermediate owned buffer.
    /// Byte-identical to `dump`; checkpoint writes use this so no whole-group
    /// clone or temporary dump buffer is needed.
    pub fn dump_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&MUTABLE_CSR_FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.adj_offsets.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.load(Ordering::Relaxed).to_le_bytes());
        out.extend_from_slice(&(self.nbr_list.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.overflow_chunk_edges as u64).to_le_bytes());

        let (_, offsets_payload) = encode_topology_u32_column(&self.adj_offsets);
        out.extend_from_slice(&offsets_payload);
        let (_, degrees_payload) = encode_topology_u32_column(&self.degrees);
        out.extend_from_slice(&degrees_payload);
        let (_, caps_payload) = encode_topology_u32_column(&self.primary_capacities);
        out.extend_from_slice(&caps_payload);

        {
            let endpoints: Vec<u32> = self.nbr_list.iter().map(|nbr| nbr.endpoint).collect();
            let (_, endpoints_payload) = encode_topology_u32_column(&endpoints);
            out.extend_from_slice(&endpoints_payload);
        }
        {
            let ranks: Vec<i64> = self.nbr_list.iter().map(|nbr| nbr.rank).collect();
            let (_, ranks_payload) = encode_topology_i64_column(&ranks);
            out.extend_from_slice(&ranks_payload);
        }
        {
            let edge_ids: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.edge_id.0).collect();
            let (_, edge_ids_payload) = encode_topology_u64_column(&edge_ids);
            out.extend_from_slice(&edge_ids_payload);
        }
        {
            let create_stamps: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.create_ts).collect();
            let (_, create_payload) = encode_topology_u64_column(&create_stamps);
            out.extend_from_slice(&create_payload);
        }
        {
            let delete_stamps: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.delete_ts).collect();
            let (_, delete_payload) = encode_topology_u64_column(&delete_stamps);
            out.extend_from_slice(&delete_payload);
        }

        for vid in 0..self.adj_offsets.len() {
            let chunks = self.overflow_chunks.get(&(vid as u32));
            out.extend_from_slice(&(chunks.map_or(0, Vec::len) as u32).to_le_bytes());
            if let Some(chunks) = chunks {
                for chunk in chunks {
                    encode_overflow_chunk(chunk, out);
                }
            }
        }
    }

    /// Encoding report for the persisted topology columns.
    ///
    /// Measures the winning encoding per column without changing in-memory
    /// state. Neighbor and edge-id columns are reported first, offset and
    /// length columns follow; all use the integer column path only.
    pub fn topology_encoding_report(&self) -> Vec<(String, TopologyColumnEncoding, usize, usize)> {
        let endpoints: Vec<u32> = self.nbr_list.iter().map(|nbr| nbr.endpoint).collect();
        let edge_ids: Vec<u64> = self.nbr_list.iter().map(|nbr| nbr.edge_id.0).collect();
        let (endpoint_choice, _) = encode_topology_u32_column(&endpoints);
        let (edge_id_choice, _) = encode_topology_u64_column(&edge_ids);
        let (offsets_choice, _) = encode_topology_u32_column(&self.adj_offsets);
        let (degrees_choice, _) = encode_topology_u32_column(&self.degrees);
        let (caps_choice, _) = encode_topology_u32_column(&self.primary_capacities);
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

    /// Load from bytes, version 4 only.
    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 36 {
            return Err(StorageError::deserialize_error(
                "CSR data too short for header",
            ));
        }

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

        let adj_offsets = decode_topology_u32_column(data, &mut offset)?;
        let degrees = decode_topology_u32_column(data, &mut offset)?;
        let primary_capacities = decode_topology_u32_column(data, &mut offset)?;
        if adj_offsets.len() != vertex_capacity
            || degrees.len() != vertex_capacity
            || primary_capacities.len() != vertex_capacity
        {
            return Err(StorageError::deserialize_error(
                "Mutable CSR header column length mismatch",
            ));
        }
        let endpoints = decode_topology_u32_column(data, &mut offset)?;
        let ranks = decode_topology_i64_column(data, &mut offset)?;
        let edge_ids = decode_topology_u64_column(data, &mut offset)?;
        let create_stamps = decode_topology_u64_column(data, &mut offset)?;
        let delete_stamps = decode_topology_u64_column(data, &mut offset)?;
        if endpoints.len() != primary_len
            || ranks.len() != primary_len
            || edge_ids.len() != primary_len
            || create_stamps.len() != primary_len
            || delete_stamps.len() != primary_len
        {
            return Err(StorageError::deserialize_error(
                "Mutable CSR neighbor column length mismatch",
            ));
        }
        let mut nbr_list = Vec::with_capacity(primary_len);
        for index in 0..primary_len {
            let mut nbr = Nbr::with_timestamps(
                endpoints[index],
                ranks[index],
                EdgeId(edge_ids[index]),
                delete_stamps[index],
            );
            nbr.create_ts = create_stamps[index];
            nbr_list.push(nbr);
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
        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk = decode_overflow_chunk(data, &mut offset)?;
                if chunk.len() > overflow_chunk_edges {
                    return Err(StorageError::deserialize_error(
                        "Mutable CSR overflow chunk exceeds configured chunk size",
                    ));
                }
                overflow_capacity = overflow_capacity.saturating_add(chunk.capacity().max(1));
                chunks.push(chunk);
            }
            if !chunks.is_empty() {
                overflow_chunks.insert(vid as u32, chunks);
            }
        }

        self.total_edge_capacity = nbr_list.len().saturating_add(overflow_capacity);
        self.adj_offsets = adj_offsets;
        self.degrees = degrees;
        self.primary_capacities = primary_capacities;
        self.overflow_chunks = overflow_chunks;
        self.overflow_chunk_edges = overflow_chunk_edges;
        self.nbr_list = nbr_list;
        self.edge_count.store(edge_count, Ordering::Relaxed);
        self.rebuild_live_sets();
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in mutable CSR payload",
            ));
        }

        Ok(())
    }
}
