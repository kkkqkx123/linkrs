//! Column-level large-object overflow area.
//!
//! Payloads above the overflow threshold live outside the main column file
//! in a `<col>.overflow` sidecar. In memory the store is an append-only
//! buffer; flush persists it and load rebuilds the index. Deletes never
//! reclaim entries eagerly; flush rebuilds the file from live rows only.

use std::collections::HashMap;
use std::io::Read;

use linkrs_core::{StorageError, StorageResult, Value};

use super::Column;

/// Default payload size above which a string spills to the overflow file.
pub const DEFAULT_OVERFLOW_THRESHOLD: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OverflowHandle {
    pub entry_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct OverflowEntry {
    pub offset: u64,
    pub len: u32,
}

#[derive(Debug, Clone, Default)]
pub struct OverflowStore {
    pending: Vec<u8>,
    index: Vec<OverflowEntry>,
    threshold: usize,
}

impl OverflowStore {
    pub fn new(threshold: usize) -> Self {
        Self {
            pending: Vec::new(),
            index: Vec::new(),
            threshold,
        }
    }

    pub fn set_threshold(&mut self, threshold: usize) {
        self.threshold = threshold;
    }

    pub fn should_overflow(&self, len: usize) -> bool {
        len > self.threshold
    }

    /// Whether overflow routing applies to `data_type`.
    ///
    /// Unified by payload bytes, not a type whitelist: every variable-length
    /// type spills when its serialized payload exceeds the threshold. Fixed
    /// small types never spill. `usize::MAX` disables routing entirely.
    pub fn routes_for(data_type: &linkrs_core::DataType) -> bool {
        if !crate::vertex::column::is_variable_length_type(data_type) {
            return false;
        }
        true
    }
}

/// Serialized overflow payload bytes for one value (without length prefix).
///
/// Unified threshold input: strings, blobs, vectors, JSON, geography and
/// opaque containers all measure their stored bytes here. Fixed small values
/// and nulls yield `None` and never spill. Disabled thresholds
/// (`usize::MAX`) are checked by the caller via `should_overflow`.
pub(crate) fn overflow_payload_bytes(value: &linkrs_core::Value) -> Option<Vec<u8>> {
    use linkrs_core::Value;
    match value {
        Value::String(s) => Some(s.as_bytes().to_vec()),
        Value::FixedString(s) => Some(s.as_bytes().to_vec()),
        Value::Blob(b) => Some(b.clone()),
        Value::Vector(v) => {
            let dense = v.to_dense();
            let mut out = Vec::with_capacity(dense.len() * 4);
            for f in dense {
                out.extend_from_slice(&f.to_le_bytes());
            }
            Some(out)
        }
        Value::Json(j) => Some(j.as_str().as_bytes().to_vec()),
        Value::JsonB(j) => Some(j.to_json_string().as_bytes().to_vec()),
        Value::Geography(g) => postcard::to_allocvec(g).ok(),
        Value::Struct(_)
        | Value::Array(_)
        | Value::List(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::DataSet(_)
        | Value::Vertex(_)
        | Value::Edge(_)
        | Value::EdgeHeader(_)
        | Value::Path(_)
        | Value::Interval(_)
        | Value::Decimal128(_)
        | Value::VertexId(_)
        | Value::EdgeId(_) => postcard::to_allocvec(value).ok(),
        _ => None,
    }
}

/// Decode overflow payload bytes for one column type.
///
/// Strict companion of the variable-width decoder: invalid UTF-8,
/// dimension mismatches and undecodable containers are storage errors
/// carrying no row context here; callers attach column and row.
pub(crate) fn decode_overflow_payload(
    data_type: &linkrs_core::DataType,
    bytes: Vec<u8>,
) -> linkrs_core::StorageResult<linkrs_core::Value> {
    use linkrs_core::{DataType, StorageError, Value};
    match data_type {
        DataType::Blob => Ok(Value::Blob(bytes)),
        DataType::String => String::from_utf8(bytes)
            .map(Value::string)
            .map_err(|e| StorageError::deserialize_error(format!("overflow UTF-8: {}", e))),
        DataType::FixedString(_) => String::from_utf8(bytes)
            .map(Value::FixedString)
            .map_err(|e| StorageError::deserialize_error(format!("overflow UTF-8: {}", e))),
        DataType::Vector | DataType::VectorDense(_) | DataType::VectorSparse(_) => {
            if !bytes.len().is_multiple_of(4) {
                return Err(StorageError::deserialize_error(format!(
                    "overflow vector length {} not a multiple of 4",
                    bytes.len()
                )));
            }
            let dim = bytes.len() / 4;
            if let DataType::VectorDense(expected) = data_type {
                if *expected > 0 && dim != *expected {
                    return Err(StorageError::deserialize_error(format!(
                        "overflow vector dimension mismatch: need {}, got {}",
                        expected, dim
                    )));
                }
            }
            let mut out = Vec::with_capacity(dim);
            for i in 0..dim {
                let chunk: [u8; 4] = bytes[i * 4..(i + 1) * 4].try_into().map_err(|_| {
                    StorageError::deserialize_error(
                        "overflow vector component undecodable".to_string(),
                    )
                })?;
                out.push(f32::from_le_bytes(chunk));
            }
            Ok(Value::Vector(linkrs_core::value::VectorValue::dense(out)))
        }
        DataType::Json => {
            let s = String::from_utf8(bytes).map_err(|e| {
                StorageError::deserialize_error(format!("overflow JSON UTF-8: {}", e))
            })?;
            linkrs_core::value::Json::parse(&s)
                .map(|j| Value::Json(Box::new(j)))
                .map_err(|e| StorageError::deserialize_error(format!("overflow JSON: {}", e)))
        }
        DataType::JsonB => {
            let s = String::from_utf8(bytes).map_err(|e| {
                StorageError::deserialize_error(format!("overflow JSONB UTF-8: {}", e))
            })?;
            linkrs_core::value::JsonB::parse(&s)
                .map(|jb| Value::JsonB(Box::new(jb)))
                .map_err(|e| StorageError::deserialize_error(format!("overflow JSONB: {}", e)))
        }
        DataType::Geography => postcard::from_bytes::<linkrs_core::value::Geography>(&bytes)
            .map(Value::Geography)
            .map_err(|e| StorageError::deserialize_error(format!("overflow geography: {}", e))),
        DataType::Struct(_)
        | DataType::Array(_)
        | DataType::List(_)
        | DataType::Map(_)
        | DataType::Set(_)
        | DataType::DataSet
        | DataType::Vertex
        | DataType::Edge
        | DataType::Path
        | DataType::Interval
        | DataType::Decimal128
        | DataType::Decimal { .. }
        | DataType::Union(_) => postcard::from_bytes::<Value>(&bytes)
            .map_err(|e| StorageError::deserialize_error(format!("overflow opaque: {}", e))),
        _ => String::from_utf8(bytes)
            .map(Value::string)
            .map_err(|e| StorageError::deserialize_error(format!("overflow UTF-8: {}", e))),
    }
}

impl OverflowStore {
    pub fn append(&mut self, bytes: &[u8]) -> StorageResult<OverflowHandle> {
        let len = u32::try_from(bytes.len()).map_err(|_| {
            StorageError::invalid_input(format!(
                "overflow payload of {} bytes exceeds the 4GiB entry limit",
                bytes.len()
            ))
        })?;
        let entry_id =
            u32::try_from(self.index.len()).map_err(|_| StorageError::capacity_exceeded())?;
        let offset = self.pending.len() as u64;
        self.pending.extend_from_slice(bytes);
        self.index.push(OverflowEntry { offset, len });
        Ok(OverflowHandle { entry_id })
    }

    pub fn get(&self, handle: &OverflowHandle) -> Option<Vec<u8>> {
        let entry = self.index.get(handle.entry_id as usize)?;
        let start = entry.offset as usize;
        let end = (entry.offset as usize).checked_add(entry.len as usize)?;
        if end <= self.pending.len() {
            return Some(self.pending[start..end].to_vec());
        }
        None
    }

    pub fn memory_usage(&self) -> usize {
        self.pending.len() + self.index.len() * std::mem::size_of::<OverflowEntry>()
    }

    /// Rebuild from live payloads only; used by flush to drop garbage.
    pub fn rebuild_from_live(&mut self, live: &[Vec<u8>]) -> StorageResult<()> {
        self.pending.clear();
        self.index.clear();
        for payload in live {
            self.append(payload)?;
        }
        Ok(())
    }

    pub fn flush_to_sidecar_buffer(&self, buf: &mut Vec<u8>) -> StorageResult<()> {
        let start = buf.len();
        let count = u32::try_from(self.index.len()).map_err(|_| {
            StorageError::invalid_input(format!(
                "overflow entry count {} exceeds the sidecar format limit",
                self.index.len()
            ))
        })?;
        buf.extend_from_slice(&count.to_le_bytes());
        // Threshold persists at full width: a configured threshold above
        // 4GiB must round-trip instead of truncating into a smaller value.
        buf.extend_from_slice(&(self.threshold as u64).to_le_bytes());
        for entry in &self.index {
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.extend_from_slice(&entry.len.to_le_bytes());
        }
        buf.extend_from_slice(&self.pending);
        let crc = crc32fast::hash(&buf[start..]);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(())
    }

    pub fn load_from_bytes(&mut self, bytes: &[u8]) -> StorageResult<()> {
        if bytes.len() < 16 {
            return Err(StorageError::deserialize_error(
                "overflow section too small".to_string(),
            ));
        }
        let stored_crc = u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().map_err(|_| {
            StorageError::deserialize_error("overflow section CRC tail malformed".to_string())
        })?);
        let computed = crc32fast::hash(&bytes[..bytes.len() - 4]);
        if stored_crc != computed {
            return Err(StorageError::deserialize_error(format!(
                "overflow section CRC mismatch: stored={:#x} computed={:#x}",
                stored_crc, computed
            )));
        }
        let mut cursor = &bytes[..bytes.len() - 4];
        let mut u32b = [0u8; 4];
        cursor.read_exact(&mut u32b)?;
        let count = u32::from_le_bytes(u32b) as usize;
        let mut u64b = [0u8; 8];
        cursor.read_exact(&mut u64b)?;
        self.threshold = u64::from_le_bytes(u64b) as usize;
        self.index.clear();
        for _ in 0..count {
            let mut off = [0u8; 8];
            cursor.read_exact(&mut off)?;
            cursor.read_exact(&mut u32b)?;
            self.index.push(OverflowEntry {
                offset: u64::from_le_bytes(off),
                len: u32::from_le_bytes(u32b),
            });
        }
        self.pending = cursor.to_vec();
        Ok(())
    }
}

impl Column {
    /// Base value for encoding inputs and persisted buffers: overflow rows
    /// contribute their inline placeholder (the payload travels in the
    /// sidecar). Point reads use `get`, which serves the side store.
    pub(super) fn encoding_base_value(&self, row_idx: usize) -> Option<Value> {
        let chunks = self.chunks.read();
        self.encoding_base_value_in(&chunks, row_idx)
    }

    /// Collect every overflow mapping as global rows. The segment latch is
    /// released before touching the side store.
    pub(super) fn collect_overflow_rows(&self) -> Vec<(usize, OverflowHandle)> {
        let chunks = self.chunks.read();
        let mut rows = Vec::new();
        for chunk in chunks.iter() {
            let state = chunk.read_state();
            rows.extend(
                state
                    .overflow_rows
                    .iter()
                    .map(|(local, handle)| (chunk.row_offset + *local as usize, *handle)),
            );
        }
        rows
    }

    /// Rebuild the overflow store from live rows only (flush-time GC).
    /// Exclusive-only (flush path): it replaces the store and repartitions
    /// every mapping.
    pub fn rebuild_overflow(&self) -> StorageResult<()> {
        let rows = self.collect_overflow_rows();
        if rows.is_empty() {
            return Ok(());
        }
        let threshold = self.overflow_threshold();
        // Main buffers hold placeholders for overflow rows, so payloads are
        // re-read from the side store itself; rows whose payload vanished
        // (overwritten with a short value since) are dropped.
        let mut live_rows: Vec<usize> = Vec::new();
        let mut live_payloads: Vec<Vec<u8>> = Vec::new();
        {
            let store = self.overflow_store.lock();
            for (row, handle) in &rows {
                match store.get(handle) {
                    Some(payload) if payload.len() > threshold => {
                        live_rows.push(*row);
                        live_payloads.push(payload);
                    }
                    _ => {}
                }
            }
        }
        let mut order: Vec<usize> = (0..live_rows.len()).collect();
        order.sort_by_key(|&i| live_rows[i]);
        let sorted_payloads: Vec<Vec<u8>> =
            order.iter().map(|&i| live_payloads[i].clone()).collect();
        self.overflow_store
            .lock()
            .rebuild_from_live(&sorted_payloads)?;
        // Rebuild preserves row order, so entry ids follow the sorted rows.
        let mut sorted_rows: Vec<usize> = live_rows;
        sorted_rows.sort_unstable();
        let capacity = self.chunk_capacity();
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().overflow_rows.clear();
        }
        for (entry_id, row) in sorted_rows.into_iter().enumerate() {
            let handle = OverflowHandle {
                entry_id: u32::try_from(entry_id).map_err(|_| {
                    StorageError::invalid_input(format!(
                        "overflow entry count {} exceeds the handle limit",
                        entry_id
                    ))
                })?,
            };
            if let Some(chunk) = chunks.get(row / capacity.max(1)) {
                if row >= chunk.row_offset && row < chunk.row_offset + chunk.row_count {
                    chunk
                        .write_state()
                        .overflow_rows
                        .insert((row - chunk.row_offset) as u32, handle);
                }
            }
        }
        Ok(())
    }

    /// Serialize overflow state for the `<col>.overflow` sidecar.
    pub fn serialize_overflow(&self) -> StorageResult<Vec<u8>> {
        let mut store_buf = Vec::new();
        {
            let store = self.overflow_store.lock();
            store.flush_to_sidecar_buffer(&mut store_buf)?;
        }
        let mut rows = self.collect_overflow_rows();
        let mut buf = Vec::new();
        buf.push(1u8);
        buf.extend_from_slice(&(store_buf.len() as u32).to_le_bytes());
        buf.extend_from_slice(&store_buf);
        buf.extend_from_slice(&(rows.len() as u32).to_le_bytes());
        rows.sort_by_key(|(row, _)| *row);
        for (row, handle) in rows {
            buf.extend_from_slice(&(row as u32).to_le_bytes());
            buf.extend_from_slice(&handle.entry_id.to_le_bytes());
        }
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_le_bytes());
        Ok(buf)
    }

    /// Restore overflow state from a sidecar buffer.
    pub fn load_overflow_bytes(&self, bytes: &[u8]) -> StorageResult<()> {
        if bytes.len() < 10 {
            return Err(StorageError::deserialize_error(
                "overflow sidecar too small".to_string(),
            ));
        }
        let crc_tail: [u8; 4] = bytes[bytes.len() - 4..].try_into().map_err(|_| {
            StorageError::deserialize_error("overflow sidecar CRC tail malformed".to_string())
        })?;
        let stored_crc = u32::from_le_bytes(crc_tail);
        let computed = crc32fast::hash(&bytes[..bytes.len() - 4]);
        if stored_crc != computed {
            return Err(StorageError::deserialize_error(format!(
                "overflow sidecar CRC mismatch: stored={:#x} computed={:#x}",
                stored_crc, computed
            )));
        }
        let mut cursor = &bytes[..bytes.len() - 4];
        let mut ver = [0u8; 1];
        cursor.read_exact(&mut ver)?;
        if ver[0] != 1 {
            return Err(StorageError::deserialize_error(format!(
                "unsupported overflow version {}",
                ver[0]
            )));
        }
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let store_len = u32::from_le_bytes(len_buf) as usize;
        if store_len > cursor.len() {
            return Err(StorageError::deserialize_error(
                "overflow sidecar truncated".to_string(),
            ));
        }
        let mut store = OverflowStore::new(self.overflow_threshold());
        store.load_from_bytes(&cursor[..store_len])?;
        cursor = &cursor[store_len..];
        cursor.read_exact(&mut len_buf)?;
        let map_len = u32::from_le_bytes(len_buf) as usize;
        let mut rows = HashMap::new();
        for _ in 0..map_len {
            let mut b = [0u8; 4];
            cursor.read_exact(&mut b)?;
            let row = u32::from_le_bytes(b) as usize;
            cursor.read_exact(&mut b)?;
            let entry = u32::from_le_bytes(b);
            rows.insert(row, OverflowHandle { entry_id: entry });
        }
        *self.overflow_store.lock() = store;
        // Repartition the global mappings into their owning segments.
        let capacity = self.chunk_capacity();
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().overflow_rows.clear();
        }
        for (row, handle) in rows {
            if let Some(chunk) = chunks.get(row / capacity.max(1)) {
                if row >= chunk.row_offset && row < chunk.row_offset + chunk.row_count {
                    chunk
                        .write_state()
                        .overflow_rows
                        .insert((row - chunk.row_offset) as u32, handle);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_get_roundtrip() {
        let mut store = OverflowStore::new(10);
        assert!(store.should_overflow(11));
        assert!(!store.should_overflow(10));
        let h = store
            .append(b"hello world, large payload")
            .expect("small payload appends");
        assert_eq!(store.get(&h).unwrap(), b"hello world, large payload");
    }

    #[test]
    fn sidecar_buffer_roundtrip_with_crc() {
        let mut store = OverflowStore::new(4);
        let h1 = store
            .append(b"first large value")
            .expect("small payload appends");
        let h2 = store
            .append(b"second large value")
            .expect("small payload appends");
        let mut buf = Vec::new();
        store.flush_to_sidecar_buffer(&mut buf).unwrap();
        let mut loaded = OverflowStore::new(1024);
        loaded.load_from_bytes(&buf).unwrap();
        assert_eq!(loaded.get(&h1).unwrap(), b"first large value");
        assert_eq!(loaded.get(&h2).unwrap(), b"second large value");
        assert_eq!(loaded.index.len(), 2);
    }

    #[test]
    fn rebuild_drops_garbage() {
        let mut store = OverflowStore::new(4);
        let _ = store.append(b"dead payload here");
        store
            .rebuild_from_live(&[b"live payload!!".to_vec()])
            .expect("small payload rebuilds");
        assert_eq!(store.index.len(), 1);
        assert_eq!(
            store.get(&OverflowHandle { entry_id: 0 }).unwrap(),
            b"live payload!!"
        );
    }
}
