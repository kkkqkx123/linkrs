use graphdb_core::{StorageError, StorageResult};

use super::super::csr_trait::CsrBase;
use super::overflow::{PureOverflowChunk, PureOverflowStorage};
use super::{PureTopologyCsr, INVALID_EDGE_ID};
use crate::persistence::{read_u32_le, read_u64_le};

impl PureTopologyCsr {
    fn dump_columns(out: &mut Vec<u8>, values: &[u32]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn dump_columns_u64(out: &mut Vec<u8>, values: &[u64]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn load_columns_u32(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u32>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(4);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "pure CSR u32 column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&data[*offset..*offset + 4]);
            *offset += 4;
            out.push(u32::from_le_bytes(buf));
        }
        Ok(out)
    }

    fn load_columns_u64(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "pure CSR u64 column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&data[*offset..*offset + 8]);
            *offset += 8;
            out.push(u64::from_le_bytes(buf));
        }
        Ok(out)
    }
}

impl CsrBase for PureTopologyCsr {
    fn vertex_capacity(&self) -> usize {
        PureTopologyCsr::vertex_capacity(self)
    }

    fn edge_count(&self) -> u64 {
        self.edge_count
    }

    fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        self.dump_into(&mut result);
        result
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&(self.rows.adj_offsets.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.edge_count.to_le_bytes());
        out.extend_from_slice(&(self.endpoints.len() as u64).to_le_bytes());

        Self::dump_columns(out, &self.rows.degrees);
        Self::dump_columns(out, &self.endpoints);
        Self::dump_columns_u64(out, &self.edge_ids);

        for vid in 0..self.rows.adj_offsets.len() {
            let chunks = self.overflow_chunks.get(vid as u32);
            out.extend_from_slice(&(chunks.map_or(0, Vec::len) as u32).to_le_bytes());
            if let Some(chunks) = chunks {
                for chunk in chunks {
                    out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                    Self::dump_columns(out, &chunk.endpoints);
                    Self::dump_columns_u64(out, &chunk.edge_ids);
                }
            }
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 24 {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr data too short for header",
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "Pure CSR dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;

        let mut offset = 0usize;

        let vertex_capacity = read_u64_le(data, &mut offset)? as usize;
        let edge_count = read_u64_le(data, &mut offset)?;
        let primary_len = read_u64_le(data, &mut offset)? as usize;

        let degrees = Self::load_columns_u32(data, &mut offset)?;
        let endpoints = Self::load_columns_u32(data, &mut offset)?;
        let edge_ids = Self::load_columns_u64(data, &mut offset)?;

        if degrees.len() != vertex_capacity {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr degrees length mismatch",
            ));
        }
        if endpoints.len() != primary_len || edge_ids.len() != primary_len {
            return Err(StorageError::deserialize_error(
                "PureTopologyCsr neighbor column length mismatch",
            ));
        }

        let mut overflow_chunks = PureOverflowStorage::new();
        let mut overflow_capacity = 0usize;
        let mut recomputed: u64 = 0;
        for &eid in &edge_ids {
            if eid != INVALID_EDGE_ID.0 {
                recomputed += 1;
            }
        }

        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk_len = read_u32_le(data, &mut offset)? as usize;
                let chunk_endpoints = Self::load_columns_u32(data, &mut offset)?;
                let chunk_edge_ids = Self::load_columns_u64(data, &mut offset)?;
                if chunk_endpoints.len() != chunk_len || chunk_edge_ids.len() != chunk_len {
                    return Err(StorageError::deserialize_error(
                        "PureTopologyCsr overflow chunk column length mismatch",
                    ));
                }
                for &eid in &chunk_edge_ids {
                    if eid != INVALID_EDGE_ID.0 {
                        recomputed += 1;
                    }
                }
                overflow_capacity = overflow_capacity.saturating_add(chunk_len);
                chunks.push(PureOverflowChunk {
                    endpoints: chunk_endpoints,
                    edge_ids: chunk_edge_ids,
                });
            }
            if !chunks.is_empty() {
                overflow_chunks.insert(vid as u32, chunks);
            }
        }

        if recomputed != edge_count {
            return Err(StorageError::deserialize_error(format!(
                "PureTopologyCsr edge count mismatch: stored={}, recomputed={}",
                edge_count, recomputed
            )));
        }

        self.rows.adj_offsets.resize(vertex_capacity, 0);
        self.rows.degrees.resize(vertex_capacity, 0);
        self.rows.primary_capacities.resize(vertex_capacity, 0);

        let mut running_offset = 0u32;
        for (vid, &degree) in degrees.iter().enumerate() {
            self.rows.adj_offsets[vid] = running_offset;
            self.rows.degrees[vid] = degree;
            self.rows.primary_capacities[vid] = degree;
            running_offset += degree;
        }

        self.endpoints = endpoints;
        self.edge_ids = edge_ids;
        self.total_edge_capacity = self.endpoints.len().saturating_add(overflow_capacity);
        self.overflow_chunks = overflow_chunks;
        self.edge_count = edge_count;
        self.primary_sorted = vec![false; vertex_capacity];

        self.live_sets.clear();
        self.live_sets.ensure_capacity(vertex_capacity);
        self.rebuild_live_sets();

        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in pure CSR payload",
            ));
        }

        Ok(())
    }
}
