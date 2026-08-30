use graphdb_core::{DataType, StorageError, StorageResult, Value};

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
    pub(super) offsets: Vec<usize>,
    pub(super) null_bitmap: Option<BitVec<u8, Lsb0>>,
    pub(super) row_count: usize,
    pub(super) data_type: DataType,
    /// O(1) count of null rows, maintained incrementally (see
    /// [`FixedWidthColumn::null_count`]).
    pub(super) null_count: usize,
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
        }
    }
}

impl ColumnStorage for VariableWidthColumn {
    fn get(&self, row_idx: usize) -> Option<Value> {
        if self.is_null(row_idx) {
            return None;
        }

        if row_idx >= self.offsets.len() {
            return None;
        }

        let start = self.offsets[row_idx];
        if start == usize::MAX {
            return None;
        }

        if start + 8 > self.data.len() {
            return None;
        }

        let len_bytes: [u8; 8] = self.data[start..start + 8].try_into().ok()?;
        let len = u64::from_le_bytes(len_bytes) as usize;

        if start + 8 + len > self.data.len() {
            return None;
        }

        let bytes = &self.data[start + 8..start + 8 + len];
        if matches!(self.data_type, DataType::Geography) {
            postcard::from_bytes::<graphdb_core::value::Geography>(bytes)
                .ok()
                .map(Value::Geography)
        } else if matches!(self.data_type, DataType::Vector) {
            if bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
                let dim = bytes.len() / std::mem::size_of::<f32>();
                let mut data = Vec::with_capacity(dim);
                for i in 0..dim {
                    let chunk: [u8; 4] = bytes[i * 4..(i + 1) * 4].try_into().ok()?;
                    data.push(f32::from_le_bytes(chunk));
                }
                Some(Value::Vector(VectorValue::dense(data)))
            } else {
                None
            }
        } else if matches!(self.data_type, DataType::Json) {
            let s = String::from_utf8(bytes.to_vec()).ok()?;
            graphdb_core::value::Json::parse(&s)
                .ok()
                .map(|j| Value::Json(Box::new(j)))
        } else if matches!(self.data_type, DataType::JsonB) {
            let s = String::from_utf8(bytes.to_vec()).ok()?;
            graphdb_core::value::JsonB::parse(&s)
                .ok()
                .map(|jb| Value::JsonB(Box::new(jb)))
        } else if matches!(self.data_type, DataType::Struct(_) | DataType::Array(_)) {
            // Composite values are stored as postcard-encoded whole `Value`s
            // (the serde single-track format).
            postcard::from_bytes::<Value>(bytes).ok()
        } else {
            String::from_utf8(bytes.to_vec()).ok().map(Value::string)
        }
    }

    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let was_null = self
            .null_bitmap
            .as_ref()
            .map(|b| row_idx < b.len() && b[row_idx])
            .unwrap_or(false);

        while self.offsets.len() <= row_idx {
            self.offsets.push(self.data.len());
        }

        match value {
            Some(v) => {
                let start = self.data.len();
                write_variable_value(&mut self.data, v)?;
                self.offsets[row_idx] = start;

                if let Some(ref mut bitmap) = self.null_bitmap {
                    ensure_bitmap_len(bitmap, row_idx + 1);
                    bitmap.set(row_idx, false);
                }
            }
            None => {
                self.offsets[row_idx] = usize::MAX;

                if let Some(ref mut bitmap) = self.null_bitmap {
                    ensure_bitmap_len(bitmap, row_idx + 1);
                    bitmap.set(row_idx, true);
                }
            }
        }

        if self.null_bitmap.is_some() {
            let becomes_null = value.is_none_or(|v| v.is_null());
            if becomes_null {
                if !was_null {
                    self.null_count += 1;
                }
            } else if was_null {
                self.null_count = self.null_count.saturating_sub(1);
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
        let offsets_size = self.offsets.len() * std::mem::size_of::<usize>();
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
    }

    fn resize(&mut self, new_count: usize) {
        let old_count = self.row_count;
        self.offsets.resize(new_count, self.data.len());
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
        offsets: Vec<u64>,
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
        if !offsets.is_empty() {
            self.offsets = offsets.into_iter().map(|o| o as usize).collect();
            self.row_count = self.offsets.len();
        } else {
            self.offsets.clear();
            self.row_count = 0;
        }
    }

    fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        let offsets: Vec<u64> = self.offsets.iter().map(|&o| o as u64).collect();
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
        // Composite values (Struct/Array) serialize the whole `Value` via
        // postcard (serde single-track format, same as the undo log).
        Value::Struct(_) | Value::Array(_) => {
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

use graphdb_core::value::VectorValue;
