use graphdb_core::{StorageError, StorageResult};

use crate::persistence::{read_u32_le, read_u64_le};

use super::super::csr_shared::SegmentedTable;
use super::super::csr_trait::CsrBase;
use super::{BundledCsr, BundledOverflowValues};

impl BundledCsr {
    fn dump_valid_bits(out: &mut Vec<u8>, valid: &[bool]) {
        out.extend_from_slice(&(valid.len() as u32).to_le_bytes());
        for chunk in valid.chunks(8) {
            let mut byte = 0u8;
            for (i, v) in chunk.iter().enumerate() {
                if *v {
                    byte |= 1u8 << i;
                }
            }
            out.push(byte);
        }
    }

    fn load_valid_bits(data: &[u8], offset: &mut usize) -> StorageResult<Vec<bool>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.div_ceil(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "bundled CSR valid-bit column too short",
            ));
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let byte = data[*offset + i / 8];
            out.push(byte & (1u8 << (i % 8)) != 0);
        }
        *offset += need;
        // Padding bits must be zero, otherwise the payload is corrupt.
        if !count.is_multiple_of(8) {
            let last = data[*offset - 1];
            let used = count % 8;
            if last >> used != 0 {
                return Err(StorageError::deserialize_error(
                    "bundled CSR valid-bit padding corrupt",
                ));
            }
        }
        Ok(out)
    }

    fn dump_values_u64(out: &mut Vec<u8>, values: &[u64]) {
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for &v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn load_values_u64(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
        let count = read_u32_le(data, offset)? as usize;
        let need = count.saturating_mul(8);
        if data.len().saturating_sub(*offset) < need {
            return Err(StorageError::deserialize_error(
                "bundled CSR value column too short",
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

impl CsrBase for BundledCsr {
    fn vertex_capacity(&self) -> usize {
        self.topology.vertex_capacity()
    }

    fn edge_count(&self) -> u64 {
        self.topology.edge_count()
    }

    fn dump(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.dump_into(&mut out);
        out
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.len() < 12 {
            return Err(StorageError::deserialize_error(
                "bundled csr: data too short".to_string(),
            ));
        }
        let (body, trailer) = data.split_at(data.len() - 4);
        let mut stored_bytes = [0u8; 4];
        stored_bytes.copy_from_slice(trailer);
        let stored = u32::from_le_bytes(stored_bytes);
        let computed = crc32fast::hash(body);
        if stored != computed {
            return Err(StorageError::deserialize_error(format!(
                "bundled csr: dump CRC mismatch: stored={:#x} computed={:#x}",
                stored, computed
            )));
        }
        let data = body;
        let mut offset = 0usize;
        let topo_len = read_u64_le(data, &mut offset)? as usize;
        if data.len().saturating_sub(offset) < topo_len {
            return Err(StorageError::deserialize_error(
                "bundled csr: topology payload truncated",
            ));
        }
        self.topology.load(&data[offset..offset + topo_len])?;
        offset += topo_len;

        let primary_len = self.topology.endpoints.len();
        let values = Self::load_values_u64(data, &mut offset)?;
        let valid = Self::load_valid_bits(data, &mut offset)?;
        if values.len() != primary_len || valid.len() != primary_len {
            return Err(StorageError::deserialize_error(
                "bundled csr: primary value column length mismatch",
            ));
        }
        let vertex_capacity = self.topology.vertex_capacity();
        let mut overflow_values = SegmentedTable::new();
        overflow_values.ensure_capacity(vertex_capacity);
        for vid in 0..vertex_capacity {
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let topo_count = self
                .topology
                .overflow_chunks
                .get(vid as u32)
                .map_or(0, Vec::len);
            if chunk_count != topo_count {
                return Err(StorageError::deserialize_error(
                    "bundled csr: overflow chunk count mismatch",
                ));
            }
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let chunk_len = read_u32_le(data, &mut offset)? as usize;
                let chunk_values = Self::load_values_u64(data, &mut offset)?;
                let chunk_valid = Self::load_valid_bits(data, &mut offset)?;
                if chunk_values.len() != chunk_len || chunk_valid.len() != chunk_len {
                    return Err(StorageError::deserialize_error(
                        "bundled csr: overflow value chunk length mismatch",
                    ));
                }
                chunks.push(BundledOverflowValues {
                    values: chunk_values,
                    valid: chunk_valid,
                });
            }
            // Cross-check lengths against the topology chunks.
            if let Some(topo) = self.topology.overflow_chunks.get(vid as u32) {
                for (a, b) in topo.iter().zip(chunks.iter()) {
                    if a.len() != b.len() {
                        return Err(StorageError::deserialize_error(
                            "bundled csr: overflow value length diverges from topology",
                        ));
                    }
                }
            }
            if !chunks.is_empty() {
                let slot = overflow_values.slot_mut(vid as u32);
                *slot = Some(chunks);
            }
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "bundled csr: trailing bytes after payload",
            ));
        }
        self.primary_values = values;
        self.primary_valid = valid;
        self.overflow_values = overflow_values;
        Ok(())
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        let start = out.len();
        let topo = self.topology.dump();
        out.extend_from_slice(&(topo.len() as u64).to_le_bytes());
        out.extend_from_slice(&topo);
        Self::dump_values_u64(out, &self.primary_values);
        Self::dump_valid_bits(out, &self.primary_valid);
        for vid in 0..self.topology.vertex_capacity() {
            match self.overflow_values.get(vid as u32) {
                Some(chunks) => {
                    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
                    for chunk in chunks {
                        out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                        Self::dump_values_u64(out, &chunk.values);
                        Self::dump_valid_bits(out, &chunk.valid);
                    }
                }
                None => out.extend_from_slice(&0u32.to_le_bytes()),
            }
        }
        let crc = crc32fast::hash(&out[start..]);
        out.extend_from_slice(&crc.to_le_bytes());
    }
}
