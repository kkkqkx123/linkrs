use linkrs_core::{DataType, StorageError, StorageResult, Value};

use super::{ensure_bitmap_len, ColumnStorage};
use bitvec::prelude::*;

/// Column storage for variable-length types (String, and future Bytes/JSON).
///
/// Values are stored as concatenated byte data with an offsets array.
/// Each value is prefixed with its length (8 bytes, little-endian).
/// O(1) random access via the offsets array.
#[derive(Debug, Clone)]
pub struct VariableWidthColumn {
    pub(super) data: Vec<u8>,
    /// Narrowed row offsets: chunk-local byte positions fit `u32` because a
    /// single chunk window holds at most one capacity of rows and large
    /// payloads spill to the overflow side store. `u32::MAX` marks null.
    pub(super) offsets: Vec<u32>,
    pub(super) null_bitmap: Option<BitVec<u8, Lsb0>>,
    pub(super) row_count: usize,
    pub(super) data_type: DataType,
    /// O(1) count of null rows, maintained incrementally (see
    /// [`FixedWidthColumn::null_count`]).
    pub(super) null_count: usize,
    /// Bytes superseded by overwrites and left in `data` until a rebuild
    /// reuses them. Updates mark the old span reusable here; rebuilds
    /// compact it away first so old bytes never linger permanently.
    pub(super) wasted_bytes: usize,
}

impl VariableWidthColumn {
    pub fn new(data_type: DataType, nullable: bool) -> Self {
        Self {
            data: Vec::new(),
            offsets: Vec::new(),
            null_bitmap: if nullable { Some(BitVec::new()) } else { None },
            row_count: 0,
            data_type,
            null_count: 0,
            wasted_bytes: 0,
        }
    }

    /// Compact superseded spans by rebuilding the payload from live offsets.
    ///
    /// Rebuilds prefer reusing the wasted area first: live payloads are
    /// repacked densely and `wasted_bytes` resets, so overwritten bytes never
    /// linger permanently.
    pub fn compact(&mut self) {
        if self.wasted_bytes == 0 {
            return;
        }
        let mut packed = Vec::with_capacity(self.data.len().saturating_sub(self.wasted_bytes));
        let mut new_offsets = Vec::with_capacity(self.offsets.len());
        let mut coerced = 0usize;
        for (idx, off) in self.offsets.iter().enumerate() {
            if idx >= self.row_count || *off == u32::MAX {
                new_offsets.push(u32::MAX);
                continue;
            }
            let start = *off as usize;
            if start + 8 > self.data.len() {
                new_offsets.push(u32::MAX);
                coerced += 1;
                continue;
            }
            let len = u64::from_le_bytes(self.data[start..start + 8].try_into().unwrap_or([0u8; 8]))
                as usize;
            if start + 8 + len > self.data.len() {
                new_offsets.push(u32::MAX);
                coerced += 1;
                continue;
            }
            let pos = packed.len() as u32;
            packed.extend_from_slice(&self.data[start..start + 8 + len]);
            new_offsets.push(pos);
        }
        if coerced > 0 {
            log::warn!(
                "variable column compact coerced {} corrupt spans to null",
                coerced
            );
        }
        self.data = packed;
        self.offsets = new_offsets;
        self.wasted_bytes = 0;
    }
}

impl ColumnStorage for VariableWidthColumn {
    fn get(&self, row_idx: usize) -> Option<Value> {
        match self.try_get(row_idx) {
            Ok(value) => value,
            Err(e) => {
                log::warn!(
                    "variable column row {} lenient read failed: {}; reading as missing",
                    row_idx,
                    e
                );
                None
            }
        }
    }

    fn try_get(&self, row_idx: usize) -> StorageResult<Option<Value>> {
        if self.is_null(row_idx) {
            return Ok(None);
        }
        if row_idx >= self.row_count {
            return Ok(None);
        }
        if row_idx >= self.offsets.len() {
            return Ok(None);
        }
        let start_u32 = self.offsets[row_idx];
        if start_u32 == u32::MAX {
            return Ok(None);
        }
        let start = start_u32 as usize;
        if start + 8 > self.data.len() {
            return Err(StorageError::deserialize_error(format!(
                "variable column truncated length prefix at row {}",
                row_idx
            )));
        }
        let len_bytes: [u8; 8] = self.data[start..start + 8].try_into().map_err(|_| {
            StorageError::deserialize_error(format!(
                "variable column length prefix undecodable at row {}",
                row_idx
            ))
        })?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        if start + 8 + len > self.data.len() {
            return Err(StorageError::deserialize_error(format!(
                "variable column truncated payload at row {}: need {} bytes",
                row_idx, len
            )));
        }
        let bytes = &self.data[start + 8..start + 8 + len];
        if matches!(self.data_type, DataType::Geography) {
            let geo =
                postcard::from_bytes::<linkrs_core::value::Geography>(bytes).map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "geography payload undecodable at row {}: {}",
                        row_idx, e
                    ))
                })?;
            Ok(Some(Value::Geography(geo)))
        } else if matches!(
            self.data_type,
            DataType::Vector | DataType::VectorDense(_) | DataType::VectorSparse(_)
        ) {
            if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
                return Err(StorageError::deserialize_error(format!(
                    "vector payload length {} not a multiple of 4 at row {}",
                    bytes.len(),
                    row_idx
                )));
            }
            let dim = bytes.len() / std::mem::size_of::<f32>();
            if let DataType::VectorDense(expected) = &self.data_type {
                if *expected > 0 && dim != *expected {
                    return Err(StorageError::deserialize_error(format!(
                        "vector dimension mismatch at row {}: need {}, got {}",
                        row_idx, expected, dim
                    )));
                }
            }
            let mut data = Vec::with_capacity(dim);
            for i in 0..dim {
                let chunk: [u8; 4] = bytes[i * 4..(i + 1) * 4].try_into().map_err(|_| {
                    StorageError::deserialize_error(format!(
                        "vector component {} undecodable at row {}",
                        i, row_idx
                    ))
                })?;
                data.push(f32::from_le_bytes(chunk));
            }
            Ok(Some(Value::Vector(VectorValue::dense(data))))
        } else if matches!(self.data_type, DataType::Json) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "json payload invalid UTF-8 at row {}: {}",
                    row_idx, e
                ))
            })?;
            let j = linkrs_core::value::Json::parse(&s).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "json payload unparsable at row {}: {}",
                    row_idx, e
                ))
            })?;
            Ok(Some(Value::Json(Box::new(j))))
        } else if matches!(self.data_type, DataType::JsonB) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "jsonb payload invalid UTF-8 at row {}: {}",
                    row_idx, e
                ))
            })?;
            let jb = linkrs_core::value::JsonB::parse(&s).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "jsonb payload unparsable at row {}: {}",
                    row_idx, e
                ))
            })?;
            Ok(Some(Value::JsonB(Box::new(jb))))
        } else if matches!(self.data_type, DataType::FixedString(_)) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "fixed string payload invalid UTF-8 at row {}: {}",
                    row_idx, e
                ))
            })?;
            Ok(Some(Value::FixedString(s)))
        } else if matches!(
            self.data_type,
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
                | DataType::Union(_)
        ) {
            let v = postcard::from_bytes::<Value>(bytes).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "opaque container undecodable at row {}: {}",
                    row_idx, e
                ))
            })?;
            Ok(Some(v))
        } else if matches!(self.data_type, DataType::Blob) {
            Ok(Some(Value::Blob(bytes.to_vec())))
        } else {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "string payload invalid UTF-8 at row {}: {}",
                    row_idx, e
                ))
            })?;
            Ok(Some(Value::string(s)))
        }
    }

    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        if let DataType::FixedString(limit) = &self.data_type {
            if let Some(v) = value {
                let len = match v {
                    Value::FixedString(s) => Some(s.len()),
                    Value::String(s) => Some(s.len()),
                    _ => None,
                };
                if let Some(len) = len {
                    if len > *limit {
                        return Err(StorageError::invalid_input(format!(
                            "FixedString({}) cannot hold {} bytes",
                            limit, len
                        )));
                    }
                }
            }
        }
        // Wide or unsized dense vectors stay on this base with an explicit
        // dimension check so a wrong-dimension write fails before appending
        // payload instead of persisting a corrupt length prefix.
        if let DataType::VectorDense(dim) = &self.data_type {
            if *dim > 0 {
                if let Some(Value::Vector(vec)) = value {
                    let actual = vec.dimension();
                    if actual != *dim {
                        return Err(StorageError::invalid_input(format!(
                            "VectorDense({}) cannot hold dimension {}",
                            dim, actual
                        )));
                    }
                }
            }
        }
        let was_null = self
            .null_bitmap
            .as_ref()
            .map(|b| row_idx < b.len() && b[row_idx])
            .unwrap_or(false);

        let old_count = self.row_count;
        while self.offsets.len() <= row_idx {
            self.offsets.push(u32::MAX);
        }
        if let Some(ref mut bitmap) = self.null_bitmap {
            ensure_bitmap_len(bitmap, row_idx + 1);
            for gap in old_count..row_idx {
                if !bitmap[gap] {
                    bitmap.set(gap, true);
                }
            }
        }

        match value {
            Some(v) => {
                if row_idx < self.offsets.len() {
                    let prev = self.offsets[row_idx];
                    if prev != u32::MAX {
                        let start = prev as usize;
                        if start + 8 <= self.data.len() {
                            let len = u64::from_le_bytes(
                                self.data[start..start + 8].try_into().unwrap_or([0u8; 8]),
                            ) as usize;
                            if start + 8 + len <= self.data.len() {
                                self.wasted_bytes = self.wasted_bytes.saturating_add(8 + len);
                            }
                        }
                    }
                }
                let start = self.data.len();
                write_variable_value(&mut self.data, v)?;
                self.offsets[row_idx] = u32::try_from(start).map_err(|_| {
                    StorageError::invalid_input(format!(
                        "variable column offset exceeds u32 range at row {}",
                        row_idx
                    ))
                })?;

                if let Some(ref mut bitmap) = self.null_bitmap {
                    bitmap.set(row_idx, false);
                }
            }
            None => {
                self.offsets[row_idx] = u32::MAX;

                if let Some(ref mut bitmap) = self.null_bitmap {
                    bitmap.set(row_idx, true);
                }
            }
        }

        if self.null_bitmap.is_some() {
            let gaps = row_idx.saturating_sub(old_count);
            self.null_count += gaps;
            let becomes_null = value.is_none_or(|v| v.is_null());
            if row_idx < old_count {
                if becomes_null && !was_null {
                    self.null_count += 1;
                } else if !becomes_null && was_null {
                    self.null_count = self.null_count.saturating_sub(1);
                }
            } else if becomes_null {
                self.null_count += 1;
            }
        }

        if row_idx >= self.row_count {
            self.row_count = row_idx + 1;
        }

        Ok(())
    }

    fn len(&self) -> usize {
        self.row_count
    }

    fn is_null(&self, row_idx: usize) -> bool {
        self.null_bitmap
            .as_ref()
            .map(|b| row_idx < b.len() && b[row_idx])
            .unwrap_or(false)
    }

    fn reserve(&mut self, additional: usize) {
        self.offsets.reserve(additional);
        // String payloads vary per row; reserve the per-row length prefix
        // overhead plus a small slack so extend_from_slice rarely reallocs.
        self.data.reserve(additional * 16);
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.reserve(additional);
        }
    }

    fn memory_usage(&self) -> usize {
        let data_size = self.data.len();
        let offsets_size = self.offsets.len() * std::mem::size_of::<u32>();
        let bitmap_size = self
            .null_bitmap
            .as_ref()
            .map(|b| b.as_raw_slice().len())
            .unwrap_or(0);
        data_size + offsets_size + bitmap_size
    }

    fn clear(&mut self) {
        self.data.clear();
        self.offsets.clear();
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.clear();
        }
        self.row_count = 0;
        self.null_count = 0;
        self.wasted_bytes = 0;
    }

    fn resize(&mut self, new_count: usize) {
        let old_count = self.row_count;
        if new_count < old_count {
            self.compact();
        }
        self.offsets.resize(new_count, u32::MAX);
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.resize(new_count, false);
            for i in old_count..new_count {
                bitmap.set(i, true);
            }
        }
        if let Some(bitmap) = &self.null_bitmap {
            if new_count >= old_count {
                self.null_count += new_count - old_count;
            } else {
                self.null_count = bitmap.count_ones();
            }
        }
        self.row_count = new_count;
    }

    fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>> {
        self.null_bitmap.as_ref()
    }

    fn null_count(&self) -> usize {
        self.null_count
    }

    fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        offsets: Vec<u32>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) {
        self.data = data;
        self.null_bitmap = null_bitmap_raw.map(|raw| {
            let mut bv = BitVec::from_vec(raw);
            bv.resize(bitmap_bit_len, false);
            bv
        });
        self.null_count = self
            .null_bitmap
            .as_ref()
            .map(|b| b.count_ones())
            .unwrap_or(0);
        self.wasted_bytes = 0;
        if !offsets.is_empty() {
            self.offsets = offsets;
            self.row_count = self.offsets.len();
        } else {
            self.offsets.clear();
            self.row_count = 0;
        }
    }

    fn get_flush_data(&self) -> (Vec<u8>, Vec<u32>, Option<BitVec<u8, Lsb0>>) {
        let offsets: Vec<u32> = self.offsets.clone();
        (self.data.clone(), offsets, self.null_bitmap.clone())
    }
}

pub(crate) fn write_variable_value(data: &mut Vec<u8>, value: &Value) -> StorageResult<()> {
    match value {
        Value::String(s) => {
            let bytes = s.as_bytes();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(bytes);
        }
        Value::FixedString(s) => {
            let bytes = s.as_bytes();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(bytes);
        }
        Value::Blob(b) => {
            let len = b.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(b);
        }
        Value::Geography(geo) => {
            let bytes = postcard::to_allocvec(geo).map_err(|e| {
                StorageError::invalid_input(format!("Failed to serialize Geography: {}", e))
            })?;
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(&bytes);
        }
        Value::Vector(vec) => {
            let dense = vec.to_dense();
            let bytes = dense
                .iter()
                .flat_map(|f| f.to_le_bytes())
                .collect::<Vec<u8>>();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(&bytes);
        }
        Value::Json(j) => {
            let bytes = j.as_str().as_bytes();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(bytes);
        }
        Value::JsonB(j) => {
            let text = j.to_json_string();
            let bytes = text.as_bytes();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(bytes);
        }
        // Opaque complex values serialize the whole `Value` via postcard
        // (serde single-track format, same as the undo log). No per-type
        // compression or statistics pruning applies.
        Value::Struct(_)
        | Value::Array(_)
        | Value::List(_)
        | Value::Map(_)
        | Value::Set(_)
        | Value::DataSet(_)
        | Value::Vertex(_)
        | Value::Edge(_)
        | Value::Path(_)
        | Value::Interval(_)
        | Value::Decimal128(_)
        | Value::VertexId(_)
        | Value::EdgeId(_) => {
            let bytes = postcard::to_allocvec(value).map_err(|e| {
                StorageError::invalid_input(format!("Failed to serialize composite value: {}", e))
            })?;
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(&bytes);
        }
        _ => {
            return Err(StorageError::type_mismatch(
                value.data_type(),
                value.data_type(),
            ));
        }
    }
    Ok(())
}

use linkrs_core::value::VectorValue;
