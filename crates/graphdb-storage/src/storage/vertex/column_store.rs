//! Column Store
//!
//! Columnar storage for vertex properties.
//! Each column stores values of a single property type.
//!
//! The storage is split into two variants:
//! - `FixedWidthColumn`: For fixed-length types (Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, Uuid)
//! - `VariableWidthColumn`: For variable-length types (String)
//! - `Column`: Public wrapper that selects the appropriate variant at construction time

use crate::core::value::{DateTimeValue, DateValue, TimeValue, VectorValue};
use crate::core::{DataType, StorageError, StorageResult, Value};

use crate::storage::encoding::{
    ColumnEncoding, ColumnStats, CompressionConfig, CompressionSelector, EncodingType, FsstColumn,
    FsstEncoder,
};
use crate::utils::NullBitmap;
use bitvec::prelude::*;

/// Unified column storage interface.
pub trait ColumnStorage: Send + Sync + std::fmt::Debug {
    fn get(&self, row_idx: usize) -> Option<Value>;
    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn is_null(&self, row_idx: usize) -> bool;
    fn memory_usage(&self) -> usize;
    fn clear(&mut self);
    fn resize(&mut self, new_count: usize);
    fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>>;
    fn null_count(&self) -> usize;
    fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    );
    fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>);
    /// Extract data for a specific row range [start_row, end_row).
    /// Returns the same format as `get_flush_data()` but only for the given rows.
    fn get_flush_data_range(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>);
}

/// Returns the element size for fixed-width data types.
/// Returns 0 for variable-length types.
pub fn element_size(data_type: &DataType) -> usize {
    match data_type {
        DataType::Bool => 1,
        DataType::SmallInt => 2,
        DataType::Int => 4,
        DataType::BigInt => 8,
        DataType::Float => 4,
        DataType::Double => 8,
        DataType::Date => 12,
        DataType::Time => 8,
        DataType::DateTime | DataType::Timestamp => 28,
        DataType::Uuid => 16,
        _ => 0,
    }
}

/// Returns true if the data type is variable-length.
pub fn is_variable_length_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::String
            | DataType::Geography
            | DataType::List
            | DataType::Map
            | DataType::Set
            | DataType::Vertex
            | DataType::Edge
            | DataType::Path
            | DataType::Vector
            | DataType::VectorDense(_)
            | DataType::VectorSparse(_)
            | DataType::DataSet
            | DataType::Json
            | DataType::JsonB
            | DataType::Interval
            | DataType::Null
    )
}

// ---------------------------------------------------------------------------
// FixedWidthColumn
// ---------------------------------------------------------------------------

/// Column storage for fixed-width (primitive) types.
///
/// Values are stored in a flat `Vec<u8>` with direct offset calculation:
/// `offset = row_idx * element_size`.
/// This provides O(1) random access without any branching on type.
#[derive(Debug, Clone)]
pub struct FixedWidthColumn {
    data: Vec<u8>,
    data_type: DataType,
    element_size: usize,
    null_bitmap: Option<BitVec<u8, Lsb0>>,
    row_count: usize,
}

impl FixedWidthColumn {
    pub fn new(data_type: DataType, nullable: bool) -> Self {
        let elem_size = element_size(&data_type);
        Self {
            data: Vec::new(),
            data_type: data_type.clone(),
            element_size: elem_size,
            null_bitmap: if nullable { Some(BitVec::new()) } else { None },
            row_count: 0,
        }
    }
}

impl ColumnStorage for FixedWidthColumn {
    fn get(&self, row_idx: usize) -> Option<Value> {
        if self.is_null(row_idx) {
            return None;
        }

        let offset = row_idx * self.element_size;
        if offset + self.element_size > self.data.len() {
            return None;
        }

        let raw = read_fixed_value(&self.data, offset, self.element_size)?;
        Some(convert_to_type(raw, &self.data_type))
    }

    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let offset = row_idx * self.element_size;
        if offset + self.element_size > self.data.len() {
            self.data.resize(offset + self.element_size, 0);
        }

        match value {
            Some(v) => {
                write_fixed_value(&mut self.data, offset, self.element_size, v)?;
                if let Some(ref mut bitmap) = self.null_bitmap {
                    ensure_bitmap_len(bitmap, row_idx + 1);
                    bitmap.set(row_idx, false);
                }
            }
            None => {
                if let Some(ref mut bitmap) = self.null_bitmap {
                    ensure_bitmap_len(bitmap, row_idx + 1);
                    bitmap.set(row_idx, true);
                }
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

    fn memory_usage(&self) -> usize {
        let data_size = self.data.len();
        let bitmap_size = self
            .null_bitmap
            .as_ref()
            .map(|b| b.as_raw_slice().len())
            .unwrap_or(0);
        data_size + bitmap_size
    }

    fn clear(&mut self) {
        self.data.clear();
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.clear();
        }
        self.row_count = 0;
    }

    fn resize(&mut self, new_count: usize) {
        let old_count = self.row_count;
        self.data.resize(new_count * self.element_size, 0);
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.resize(new_count, false);
            for i in old_count..new_count {
                bitmap.set(i, true);
            }
        }
        self.row_count = new_count;
    }

    fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>> {
        self.null_bitmap.as_ref()
    }

    fn null_count(&self) -> usize {
        self.null_bitmap
            .as_ref()
            .map(|b| b.count_ones())
            .unwrap_or(0)
    }

    fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        _offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) {
        self.data = data;
        let elem_size = self.element_size.max(1);
        let remainder = self.data.len() % elem_size;
        if remainder != 0 {
            self.data
                .resize(self.data.len() + (elem_size - remainder), 0);
        }
        self.null_bitmap = null_bitmap_raw.map(|raw| {
            let mut bv = BitVec::from_vec(raw);
            bv.resize(bitmap_bit_len, false);
            bv
        });
        self.row_count = self.data.len() / elem_size;
    }

    fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        (self.data.clone(), Vec::new(), self.null_bitmap.clone())
    }

    fn get_flush_data_range(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        let start_byte = start_row * self.element_size;
        let end_byte = std::cmp::min(end_row * self.element_size, self.data.len());
        let data = if end_byte > start_byte {
            self.data[start_byte..end_byte].to_vec()
        } else {
            Vec::new()
        };

        let bitmap = self.null_bitmap.as_ref().map(|b| {
            let mut chunk = BitVec::with_capacity(end_row - start_row);
            for i in start_row..std::cmp::min(end_row, b.len()) {
                chunk.push(b[i]);
            }
            chunk.resize(end_row - start_row, false);
            chunk
        });

        (data, Vec::new(), bitmap)
    }
}

// ---------------------------------------------------------------------------
// VariableWidthColumn
// ---------------------------------------------------------------------------

/// Column storage for variable-length types (String, and future Bytes/JSON).
///
/// Values are stored as concatenated byte data with an offsets array.
/// Each value is prefixed with its length (8 bytes, little-endian).
/// O(1) random access via the offsets array.
#[derive(Debug, Clone)]
pub struct VariableWidthColumn {
    data: Vec<u8>,
    offsets: Vec<usize>,
    null_bitmap: Option<BitVec<u8, Lsb0>>,
    row_count: usize,
    data_type: DataType,
}

impl VariableWidthColumn {
    pub fn new(data_type: DataType, nullable: bool) -> Self {
        Self {
            data: Vec::new(),
            offsets: Vec::new(),
            null_bitmap: if nullable { Some(BitVec::new()) } else { None },
            row_count: 0,
            data_type,
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
            serde_json::from_slice::<crate::core::value::Geography>(bytes)
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
        } else {
            String::from_utf8(bytes.to_vec()).ok().map(Value::String)
        }
    }

    fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
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
        self.row_count = new_count;
    }

    fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>> {
        self.null_bitmap.as_ref()
    }

    fn null_count(&self) -> usize {
        self.null_bitmap
            .as_ref()
            .map(|b| b.count_ones())
            .unwrap_or(0)
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

    fn get_flush_data_range(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        let mut data = Vec::new();
        let mut offsets: Vec<u64> = Vec::new();
        let mut null_flags: Vec<bool> = Vec::new();

        for row in start_row..end_row {
            if row < self.offsets.len() && !self.is_null(row) {
                let entry_start = self.offsets[row];
                let entry_len = if row + 1 < self.offsets.len()
                    && self.offsets[row + 1] != usize::MAX
                    && self.offsets[row + 1] > 0
                {
                    self.offsets[row + 1] - entry_start
                } else {
                    self.data.len() - entry_start
                };
                offsets.push(data.len() as u64);
                data.extend_from_slice(&self.data[entry_start..entry_start + entry_len]);
                null_flags.push(false);
            } else {
                offsets.push(data.len() as u64);
                null_flags.push(true);
            }
        }

        let bitmap = self.null_bitmap.as_ref().map(|_| {
            let mut chunk = BitVec::with_capacity(null_flags.len());
            for &flag in &null_flags {
                chunk.push(flag);
            }
            chunk
        });

        (data, offsets, bitmap)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (shared between Fixed and Variable)
// ---------------------------------------------------------------------------

fn ensure_bitmap_len(bitmap: &mut BitVec<u8, Lsb0>, min_len: usize) {
    if bitmap.len() < min_len {
        bitmap.resize(min_len, false);
    }
}

fn write_fixed_value(
    data: &mut [u8],
    offset: usize,
    element_size: usize,
    value: &Value,
) -> StorageResult<()> {
    let required_size = match value {
        Value::Bool(_) => 1,
        Value::SmallInt(_) => 2,
        Value::Int(_) => 4,
        Value::BigInt(_) => 8,
        Value::Float(_) => 4,
        Value::Double(_) => 8,
        Value::Date(_) => 12,
        Value::Time(_) => 8,
        Value::DateTime(_) => 28,
        _ => {
            return Err(StorageError::type_mismatch(
                value.data_type(),
                value.data_type(),
            ));
        }
    };

    if offset + required_size > data.len() {
        return Err(StorageError::invalid_input(format!(
            "Column data buffer too small: offset={}, required_size={}, data_len={}, element_size={}",
            offset, required_size, data.len(), element_size
        )));
    }

    match value {
        Value::Bool(b) => {
            data[offset] = if *b { 1 } else { 0 };
        }
        Value::SmallInt(i) => {
            data[offset..offset + 2].copy_from_slice(&i.to_le_bytes());
        }
        Value::Int(i) => {
            data[offset..offset + 4].copy_from_slice(&i.to_le_bytes());
        }
        Value::BigInt(i) => {
            data[offset..offset + 8].copy_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            data[offset..offset + 4].copy_from_slice(&f.to_le_bytes());
        }
        Value::Double(d) => {
            data[offset..offset + 8].copy_from_slice(&d.to_le_bytes());
        }
        Value::Date(d) => {
            data[offset..offset + 4].copy_from_slice(&d.year.to_le_bytes());
            data[offset + 4..offset + 8].copy_from_slice(&d.month.to_le_bytes());
            data[offset + 8..offset + 12].copy_from_slice(&d.day.to_le_bytes());
        }
        Value::Time(t) => {
            let micros = t.hour as u64 * 3_600_000_000
                + t.minute as u64 * 60_000_000
                + t.sec as u64 * 1_000_000
                + t.microsec as u64;
            data[offset..offset + 8].copy_from_slice(&micros.to_le_bytes());
        }
        Value::DateTime(dt) => {
            data[offset..offset + 4].copy_from_slice(&dt.year.to_le_bytes());
            data[offset + 4..offset + 8].copy_from_slice(&dt.month.to_le_bytes());
            data[offset + 8..offset + 12].copy_from_slice(&dt.day.to_le_bytes());
            data[offset + 12..offset + 16].copy_from_slice(&dt.hour.to_le_bytes());
            data[offset + 16..offset + 20].copy_from_slice(&dt.minute.to_le_bytes());
            data[offset + 20..offset + 24].copy_from_slice(&dt.sec.to_le_bytes());
            data[offset + 24..offset + 28].copy_from_slice(&dt.microsec.to_le_bytes());
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

fn write_variable_value(data: &mut Vec<u8>, value: &Value) -> StorageResult<()> {
    match value {
        Value::String(s) => {
            let bytes = s.as_bytes();
            let len = bytes.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(bytes);
        }
        Value::Geography(geo) => {
            let json = serde_json::to_vec(geo).map_err(|e| {
                StorageError::invalid_input(format!("Failed to serialize Geography: {}", e))
            })?;
            let len = json.len() as u64;
            data.extend_from_slice(&len.to_le_bytes());
            data.extend_from_slice(&json);
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
        _ => {
            return Err(StorageError::type_mismatch(
                value.data_type(),
                value.data_type(),
            ));
        }
    }
    Ok(())
}

fn read_fixed_value(data: &[u8], offset: usize, element_size: usize) -> Option<Value> {
    if offset + element_size > data.len() {
        return None;
    }

    match element_size {
        1 => Some(Value::Bool(data[offset] != 0)),
        2 => {
            let bytes: [u8; 2] = data[offset..offset + 2].try_into().ok()?;
            Some(Value::SmallInt(i16::from_le_bytes(bytes)))
        }
        4 => {
            let bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
            Some(Value::Int(i32::from_le_bytes(bytes)))
        }
        8 => {
            let bytes: [u8; 8] = data[offset..offset + 8].try_into().ok()?;
            Some(Value::BigInt(i64::from_le_bytes(bytes)))
        }
        12 => {
            let year_bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
            let month_bytes: [u8; 4] = data[offset + 4..offset + 8].try_into().ok()?;
            let day_bytes: [u8; 4] = data[offset + 8..offset + 12].try_into().ok()?;
            Some(Value::Date(DateValue {
                year: i32::from_le_bytes(year_bytes),
                month: u32::from_le_bytes(month_bytes),
                day: u32::from_le_bytes(day_bytes),
            }))
        }
        28 => {
            let year_bytes: [u8; 4] = data[offset..offset + 4].try_into().ok()?;
            let month_bytes: [u8; 4] = data[offset + 4..offset + 8].try_into().ok()?;
            let day_bytes: [u8; 4] = data[offset + 8..offset + 12].try_into().ok()?;
            let hour_bytes: [u8; 4] = data[offset + 12..offset + 16].try_into().ok()?;
            let minute_bytes: [u8; 4] = data[offset + 16..offset + 20].try_into().ok()?;
            let sec_bytes: [u8; 4] = data[offset + 20..offset + 24].try_into().ok()?;
            let microsec_bytes: [u8; 4] = data[offset + 24..offset + 28].try_into().ok()?;
            Some(Value::DateTime(DateTimeValue {
                year: i32::from_le_bytes(year_bytes),
                month: u32::from_le_bytes(month_bytes),
                day: u32::from_le_bytes(day_bytes),
                hour: u32::from_le_bytes(hour_bytes),
                minute: u32::from_le_bytes(minute_bytes),
                sec: u32::from_le_bytes(sec_bytes),
                microsec: u32::from_le_bytes(microsec_bytes),
            }))
        }
        _ => None,
    }
}

/// Convert a raw read_fixed_value result to the correct Value variant based on the declared DataType.
/// This handles ambiguous element sizes where multiple types share the same width.
fn convert_to_type(raw: Value, data_type: &DataType) -> Value {
    match (data_type, &raw) {
        (DataType::Double, Value::BigInt(n)) => Value::Double(f64::from_bits(*n as u64)),
        (DataType::Float, Value::Int(n)) => Value::Float(f32::from_bits(*n as u32)),
        (DataType::Float, Value::BigInt(n)) => Value::Float(f32::from_bits(*n as u32)),
        (DataType::Time, Value::BigInt(n)) => {
            let micros = *n as u64;
            let hour = (micros / 3_600_000_000) as u32;
            let rem = micros % 3_600_000_000;
            let minute = (rem / 60_000_000) as u32;
            let rem = rem % 60_000_000;
            let sec = (rem / 1_000_000) as u32;
            let microsec = (rem % 1_000_000) as u32;
            Value::Time(TimeValue {
                hour,
                minute,
                sec,
                microsec,
            })
        }
        _ => raw,
    }
}

// ---------------------------------------------------------------------------
// Column (public wrapper enum)
// ---------------------------------------------------------------------------

/// Internal dispatch between fixed-width and variable-width storage.
#[derive(Debug, Clone)]
enum ColumnInner {
    Fixed(FixedWidthColumn),
    Variable(VariableWidthColumn),
}

/// Column storage that automatically selects fixed-width or variable-width
/// layout based on the `DataType` at construction time.
///
/// # Variant Selection
///
/// | `DataType` | Storage variant |
/// |---|---|
/// | Bool, SmallInt, Int, BigInt, Float, Double, Date, Time, Uuid | `FixedWidthColumn` |
/// | String | `VariableWidthColumn` |
#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub col_id: i32,
    pub data_type: DataType,
    pub nullable: bool,
    inner: ColumnInner,
    encoding: ColumnEncoding,
}

impl Column {
    pub fn new(name: String, col_id: i32, data_type: DataType, nullable: bool) -> Self {
        let inner = if is_variable_length_type(&data_type) {
            ColumnInner::Variable(VariableWidthColumn::new(data_type.clone(), nullable))
        } else {
            ColumnInner::Fixed(FixedWidthColumn::new(data_type.clone(), nullable))
        };

        Self {
            name,
            col_id,
            data_type,
            nullable,
            inner,
            encoding: ColumnEncoding::None,
        }
    }

    fn inner(&self) -> &dyn ColumnStorage {
        match &self.inner {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }

    fn inner_mut(&mut self) -> &mut dyn ColumnStorage {
        match &mut self.inner {
            ColumnInner::Fixed(c) => c,
            ColumnInner::Variable(c) => c,
        }
    }

    // -----------------------------------------------------------------------
    // Core read / write
    // -----------------------------------------------------------------------

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        if self.encoding.is_encoded() {
            self.encoding.set(row_idx, value)?;
            if row_idx >= self.len() {
                self.sync_row_count_from_encoding();
            }
            return Ok(());
        }

        if let Some(v) = value {
            if v.is_null() {
                if !self.nullable {
                    return Err(StorageError::null_value_not_allowed(self.name.clone()));
                }
                self.inner_mut().set(row_idx, None)?;
            } else {
                self.inner_mut().set(row_idx, Some(v))?;
            }
        } else {
            if !self.nullable {
                return Err(StorageError::null_value_not_allowed(self.name.clone()));
            }
            self.inner_mut().set(row_idx, None)?;
        }

        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        if self.encoding.is_encoded() {
            return self.encoding.get(row_idx);
        }
        self.inner().get(row_idx)
    }

    pub fn is_null(&self, row_idx: usize) -> bool {
        self.inner().is_null(row_idx)
    }

    pub fn null_count(&self) -> usize {
        self.inner().null_count()
    }

    pub fn len(&self) -> usize {
        self.inner().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner().is_empty()
    }

    pub fn null_bitmap(&self) -> Option<&BitVec<u8, Lsb0>> {
        self.inner().null_bitmap()
    }

    pub fn memory_usage(&self) -> usize {
        self.inner().memory_usage() + self.encoding.memory_usage()
    }

    pub fn memory_size(&self) -> usize {
        self.memory_usage() + std::mem::size_of::<Self>()
    }

    pub fn used_memory_size(&self) -> usize {
        let non_null_count = self.len() - self.null_count();
        let elem_size = element_size(&self.data_type);
        non_null_count * elem_size + std::mem::size_of::<Self>()
    }

    pub fn clear(&mut self) {
        self.inner_mut().clear();
        self.encoding = ColumnEncoding::None;
    }

    pub fn resize(&mut self, new_count: usize) {
        self.inner_mut().resize(new_count);
    }

    pub fn load_data_from_raw(
        &mut self,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) {
        self.inner_mut()
            .load_data_from_raw(data, offsets, null_bitmap_raw, bitmap_bit_len);
    }

    pub fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        if !self.encoding.is_encoded() {
            return self.inner().get_flush_data();
        }

        let row_count = self.len();
        let mut new_data = Vec::new();
        let mut new_offsets = Vec::new();
        let mut new_bitmap = self.null_bitmap().map(|_| BitVec::with_capacity(row_count));

        let is_var = is_variable_length_type(&self.data_type);

        for i in 0..row_count {
            let value = self.encoding.get(i);
            match value {
                Some(v) => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(false);
                    }
                    if is_var {
                        new_offsets.push(new_data.len() as u64);
                        match &v {
                            Value::String(s) => {
                                let bytes = s.as_bytes();
                                new_data.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                                new_data.extend_from_slice(bytes);
                            }
                            _ => {
                                new_offsets.pop();
                                new_offsets.push(u64::MAX);
                            }
                        }
                    } else {
                        let elem_size = element_size(&self.data_type);
                        let start = new_data.len();
                        new_data.resize(start + elem_size, 0);
                        let _ = write_fixed_value(&mut new_data, start, elem_size, &v);
                    }
                }
                None => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(true);
                    }
                    if is_var {
                        new_offsets.push(u64::MAX);
                    }
                }
            }
        }

        (new_data, new_offsets, new_bitmap)
    }

    /// Extract data for a specific row range [start_row, end_row).
    /// Returns column data in the same format as `get_flush_data()`.
    pub fn get_flush_data_range(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        if !self.encoding.is_encoded() {
            return self.inner().get_flush_data_range(start_row, end_row);
        }

        let row_count = self.len();
        let end = std::cmp::min(end_row, row_count);
        let start = std::cmp::min(start_row, end);

        let mut new_data = Vec::new();
        let mut new_offsets = Vec::new();
        let mut new_bitmap = self
            .null_bitmap()
            .map(|_| BitVec::with_capacity(end - start));

        let is_var = is_variable_length_type(&self.data_type);

        for i in start..end {
            let value = self.encoding.get(i);
            match value {
                Some(v) => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(false);
                    }
                    if is_var {
                        new_offsets.push(new_data.len() as u64);
                        match &v {
                            Value::String(s) => {
                                let bytes = s.as_bytes();
                                new_data.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                                new_data.extend_from_slice(bytes);
                            }
                            _ => {
                                new_offsets.pop();
                                new_offsets.push(u64::MAX);
                            }
                        }
                    } else {
                        let elem_size = element_size(&self.data_type);
                        let offset = new_data.len();
                        new_data.resize(offset + elem_size, 0);
                        let _ = write_fixed_value(&mut new_data, offset, elem_size, &v);
                    }
                }
                None => {
                    if let Some(ref mut bm) = new_bitmap {
                        bm.push(true);
                    }
                    if is_var {
                        new_offsets.push(u64::MAX);
                    }
                }
            }
        }

        (new_data, new_offsets, new_bitmap)
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    pub fn compute_stats(&self) -> ColumnStats {
        let mut stats = ColumnStats::new(self.data_type.clone());
        stats.row_count = self.len();
        stats.null_count = self.null_count();

        let mut distinct_values = std::collections::HashSet::new();
        let mut total_length: usize = 0;
        let mut run_count: usize = 0;
        let mut prev_value: Option<Value> = None;

        for i in 0..self.len() {
            if let Some(value) = self.get(i) {
                if prev_value.as_ref() != Some(&value) {
                    run_count += 1;
                }
                prev_value = Some(value.clone());
                distinct_values.insert(value.clone());
                if matches!(self.data_type, DataType::String) {
                    if let Value::String(s) = &value {
                        total_length += s.len();
                    }
                }
            }
        }

        stats.distinct_count = distinct_values.len();
        stats.run_count = run_count.max(1);
        stats.avg_length = if !self.is_empty() {
            total_length as f64 / self.len() as f64
        } else {
            0.0
        };

        stats
    }

    // -----------------------------------------------------------------------
    // Encoding
    // -----------------------------------------------------------------------

    pub fn encoding_type(&self) -> EncodingType {
        self.encoding.encoding_type()
    }

    fn sync_row_count_from_encoding(&mut self) {
        let encoded_len = self.encoding.len();
        self.inner_mut().resize(encoded_len);
    }

    pub fn apply_fsst_encoding(&mut self, max_symbols: usize) -> StorageResult<()> {
        if self.data_type != DataType::String {
            return Err(StorageError::not_supported(
                "FSST encoding only supports String type".to_string(),
            ));
        }

        let mut strings: Vec<Option<String>> = Vec::with_capacity(self.len());
        for i in 0..self.len() {
            if self.is_null(i) {
                strings.push(None);
            } else {
                match self.get(i) {
                    Some(Value::String(s)) => strings.push(Some(s)),
                    _ => strings.push(None),
                }
            }
        }

        let string_refs: Vec<Option<&str>> = strings.iter().map(|s| s.as_deref()).collect();
        let non_null: Vec<&str> = string_refs.iter().filter_map(|s| *s).collect();

        if non_null.is_empty() {
            return Ok(());
        }

        let encoder = FsstEncoder::train(&non_null, max_symbols);

        let mut encoded_data = Vec::with_capacity(self.len());
        let mut null_bitmap = NullBitmap::with_capacity(self.len());

        for s in &string_refs {
            match s {
                Some(val) => {
                    encoded_data.push(encoder.encode(val));
                    null_bitmap.push(false);
                }
                None => {
                    encoded_data.push(Vec::new());
                    null_bitmap.push(true);
                }
            }
        }

        let fsst_col = FsstColumn {
            encoder,
            encoded_data,
            null_bitmap,
        };

        self.encoding = ColumnEncoding::Fsst(fsst_col);

        Ok(())
    }

    pub fn apply_dictionary_encoding(&mut self) -> StorageResult<()> {
        if self.data_type != DataType::String {
            return Err(StorageError::not_supported(
                "Dictionary encoding only supports String type".to_string(),
            ));
        }

        use crate::storage::encoding::DictionaryColumn;

        let mut dict_col = DictionaryColumn::new();
        for i in 0..self.len() {
            let value = self.get(i);
            dict_col.set(i, value.as_ref())?;
        }

        self.encoding = ColumnEncoding::Dictionary(dict_col);

        Ok(())
    }

    pub fn apply_rle_encoding(&mut self) -> StorageResult<()> {
        use crate::storage::encoding::{RleBoolColumn, RleIntColumn};

        match self.data_type {
            DataType::Bool => {
                let mut rle_col = RleBoolColumn::new();
                for i in 0..self.len() {
                    let value = self.get(i);
                    rle_col.append(value.as_ref())?;
                }
                self.encoding = ColumnEncoding::RleBool(rle_col);
            }
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                let mut rle_col = RleIntColumn::new();
                for i in 0..self.len() {
                    let value = self.get(i);
                    rle_col.append(value.as_ref())?;
                }
                self.encoding = ColumnEncoding::RleInt(rle_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "RLE encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }

    pub fn apply_bitpacking_encoding(&mut self) -> StorageResult<()> {
        use crate::storage::encoding::BitPackedIntColumn;

        match self.data_type {
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                let mut values: Vec<Option<Value>> = Vec::with_capacity(self.len());
                for i in 0..self.len() {
                    values.push(self.get(i));
                }
                let bp_col = BitPackedIntColumn::analyze(&values, self.data_type.clone())?;
                self.encoding = ColumnEncoding::BitPacked(bp_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "BitPacking encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }

    pub fn apply_alp_encoding(&mut self) -> StorageResult<()> {
        use crate::storage::encoding::AlpColumn;

        match self.data_type {
            DataType::Float | DataType::Double => {
                let mut values: Vec<Option<Value>> = Vec::with_capacity(self.len());
                for i in 0..self.len() {
                    values.push(self.get(i));
                }
                let alp_col = AlpColumn::analyze_values(&values, self.data_type.clone())?;
                self.encoding = ColumnEncoding::Alp(alp_col);
            }
            _ => {
                return Err(StorageError::not_supported(format!(
                    "ALP encoding not supported for {:?}",
                    self.data_type
                )));
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ColumnStore
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ColumnStore {
    columns: Vec<Column>,
    name_to_index: std::collections::HashMap<String, usize>,
}

impl ColumnStore {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            name_to_index: std::collections::HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            columns: Vec::with_capacity(capacity),
            name_to_index: std::collections::HashMap::with_capacity(capacity),
        }
    }

    pub fn add_column(&mut self, name: String, data_type: DataType, nullable: bool) -> i32 {
        let col_id = self.columns.len() as i32;
        let column = Column::new(name.clone(), col_id, data_type, nullable);
        self.name_to_index.insert(name, self.columns.len());
        self.columns.push(column);
        col_id
    }

    pub fn get_column(&self, name: &str) -> Option<&Column> {
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.columns.get(idx))
    }

    pub fn get_column_mut(&mut self, name: &str) -> Option<&mut Column> {
        self.name_to_index
            .get(name)
            .and_then(|&idx| self.columns.get_mut(idx))
    }

    pub fn get_column_by_id(&self, col_id: i32) -> Option<&Column> {
        self.columns.get(col_id as usize)
    }

    pub fn get_column_by_id_mut(&mut self, col_id: i32) -> Option<&mut Column> {
        self.columns.get_mut(col_id as usize)
    }

    pub fn set(&mut self, row_idx: usize, values: &[(String, Value)]) -> StorageResult<()> {
        for (name, value) in values {
            if let Some(col) = self.get_column_mut(name) {
                col.set(row_idx, Some(value))?;
            }
        }
        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Vec<(String, Option<Value>)> {
        self.columns
            .iter()
            .map(|col| (col.name.clone(), col.get(row_idx)))
            .collect()
    }

    pub fn set_property(
        &mut self,
        row_idx: usize,
        col_name: &str,
        value: Option<&Value>,
    ) -> StorageResult<()> {
        let col = self
            .get_column_mut(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;
        col.set(row_idx, value)
    }

    pub fn remove_column(&mut self, name: &str) -> StorageResult<()> {
        let index = self
            .name_to_index
            .get(name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;

        self.columns.remove(index);

        self.name_to_index.clear();
        for (idx, column) in self.columns.iter_mut().enumerate() {
            column.col_id = idx as i32;
            self.name_to_index.insert(column.name.clone(), idx);
        }

        Ok(())
    }

    pub fn rename_column(&mut self, old_name: &str, new_name: String) -> StorageResult<()> {
        if self.name_to_index.contains_key(&new_name) {
            return Err(StorageError::column_already_exists(new_name));
        }

        let index = self
            .name_to_index
            .get(old_name)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;

        if let Some(column) = self.columns.get_mut(index) {
            column.name = new_name;
        }

        self.name_to_index.clear();
        for (idx, column) in self.columns.iter().enumerate() {
            self.name_to_index.insert(column.name.clone(), idx);
        }

        Ok(())
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn row_count(&self) -> usize {
        self.columns.first().map(|c| c.len()).unwrap_or(0)
    }

    pub fn clear(&mut self) {
        for col in &mut self.columns {
            col.clear();
        }
    }

    pub fn resize(&mut self, new_count: usize) {
        for col in &mut self.columns {
            col.resize(new_count);
        }
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub fn load_column_from_raw(
        &mut self,
        name: &str,
        data: Vec<u8>,
        offsets: Vec<u64>,
        null_bitmap_raw: Option<Vec<u8>>,
        bitmap_bit_len: usize,
    ) -> StorageResult<()> {
        if let Some(col) = self.get_column_mut(name) {
            col.load_data_from_raw(data, offsets, null_bitmap_raw, bitmap_bit_len);
            Ok(())
        } else {
            Err(StorageError::column_not_found(name.to_string()))
        }
    }

    pub fn apply_encoding_to_column(
        &mut self,
        col_name: &str,
        encoding_type: EncodingType,
    ) -> StorageResult<()> {
        let col = self
            .get_column_mut(col_name)
            .ok_or_else(|| StorageError::column_not_found(col_name.to_string()))?;

        if col.is_empty() {
            return Ok(());
        }

        match encoding_type {
            EncodingType::Fsst => {
                if col.data_type != DataType::String {
                    return Err(StorageError::not_supported(
                        "FSST encoding only supports String type".to_string(),
                    ));
                }
                col.apply_fsst_encoding(1024)?;
            }
            EncodingType::Dictionary => {
                col.apply_dictionary_encoding()?;
            }
            EncodingType::Rle => {
                col.apply_rle_encoding()?;
            }
            EncodingType::BitPacking => {
                col.apply_bitpacking_encoding()?;
            }
            EncodingType::Alp => {
                col.apply_alp_encoding()?;
            }
            EncodingType::None => {}
        }

        Ok(())
    }

    pub fn auto_apply_encodings(&mut self, config: Option<CompressionConfig>) -> StorageResult<()> {
        let selector = match config {
            Some(c) => CompressionSelector::with_config(c),
            None => CompressionSelector::new(),
        };

        for col in &mut self.columns {
            if col.is_empty() || col.encoding.is_encoded() {
                continue;
            }

            let stats = col.compute_stats();
            let encoding = selector.select(&stats);

            match encoding {
                EncodingType::Fsst => {
                    if col.data_type == DataType::String {
                        col.apply_fsst_encoding(1024)?;
                    }
                }
                EncodingType::Dictionary => {
                    col.apply_dictionary_encoding()?;
                }
                EncodingType::Rle => {
                    col.apply_rle_encoding()?;
                }
                EncodingType::BitPacking => {
                    col.apply_bitpacking_encoding()?;
                }
                EncodingType::Alp => {
                    col.apply_alp_encoding()?;
                }
                EncodingType::None => {}
            }
        }

        Ok(())
    }

    pub fn memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();

        for col in &self.columns {
            total += col.memory_size();
        }

        total += self.name_to_index.len()
            * (std::mem::size_of::<String>() + std::mem::size_of::<usize>());

        total
    }

    pub fn used_memory_size(&self) -> usize {
        let mut total = std::mem::size_of::<Self>();

        for col in &self.columns {
            total += col.used_memory_size();
        }

        total
    }
}

impl Default for ColumnStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_basic() {
        let mut col = Column::new("age".to_string(), 0, DataType::Int, true);

        col.set(0, Some(&Value::Int(25))).unwrap();
        col.set(1, Some(&Value::Int(30))).unwrap();
        col.set(2, None).unwrap();

        assert_eq!(col.get(0), Some(Value::Int(25)));
        assert_eq!(col.get(1), Some(Value::Int(30)));
        assert!(col.is_null(2));
        assert_eq!(col.len(), 3);
    }

    #[test]
    fn test_column_string() {
        let mut col = Column::new("name".to_string(), 0, DataType::String, false);

        col.set(0, Some(&Value::String("Alice".to_string())))
            .unwrap();
        col.set(1, Some(&Value::String("Bob".to_string()))).unwrap();

        assert_eq!(col.get(0), Some(Value::String("Alice".to_string())));
        assert_eq!(col.get(1), Some(Value::String("Bob".to_string())));
        assert_eq!(col.len(), 2);
    }

    #[test]
    fn test_column_store() {
        let mut store = ColumnStore::new();

        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();

        store
            .set(
                1,
                &[
                    ("name".to_string(), Value::String("Bob".to_string())),
                    ("age".to_string(), Value::Int(25)),
                ],
            )
            .unwrap();

        assert_eq!(
            store.get_column("age").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );
        assert_eq!(
            store.get_column("name").and_then(|col| col.get(1)),
            Some(Value::String("Bob".to_string()))
        );
    }

    #[test]
    fn test_column_store_remove_and_rename() {
        let mut store = ColumnStore::new();

        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, true);

        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                ],
            )
            .unwrap();

        store
            .rename_column("age", "years".to_string())
            .expect("rename should succeed");
        assert!(store.get_column("age").is_none());
        assert_eq!(
            store.get_column("years").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );

        store.remove_column("name").expect("remove should succeed");
        assert!(store.get_column("name").is_none());
        assert_eq!(store.column_count(), 1);
        assert_eq!(
            store.get_column("years").and_then(|col| col.get(0)),
            Some(Value::Int(30))
        );
    }

    #[test]
    fn test_fixed_width_multiple_types() {
        let mut col = Column::new("mixed".to_string(), 0, DataType::BigInt, false);
        col.set(0, Some(&Value::BigInt(100))).unwrap();
        col.set(1, Some(&Value::BigInt(200))).unwrap();
        assert_eq!(col.get(0), Some(Value::BigInt(100)));
        assert_eq!(col.get(1), Some(Value::BigInt(200)));
        assert_eq!(col.len(), 2);

        let mut col2 = Column::new("flag".to_string(), 1, DataType::Bool, true);
        col2.set(0, Some(&Value::Bool(true))).unwrap();
        col2.set(1, Some(&Value::Bool(false))).unwrap();
        col2.set(2, None).unwrap();
        assert_eq!(col2.get(0), Some(Value::Bool(true)));
        assert_eq!(col2.get(1), Some(Value::Bool(false)));
        assert!(col2.is_null(2));
    }

    #[test]
    fn test_flush_and_reload_fixed() {
        let mut col = Column::new("val".to_string(), 0, DataType::Int, true);
        col.set(0, Some(&Value::Int(10))).unwrap();
        col.set(1, Some(&Value::Int(20))).unwrap();
        col.set(2, None).unwrap();

        let (data, offsets, bitmap) = col.get_flush_data();
        assert!(offsets.is_empty());

        let mut restored = Column::new("val".to_string(), 0, DataType::Int, true);
        restored.load_data_from_raw(data, Vec::new(), bitmap.map(|b| b.into_vec()), col.len());

        assert_eq!(restored.get(0), Some(Value::Int(10)));
        assert_eq!(restored.get(1), Some(Value::Int(20)));
        assert!(restored.is_null(2));
        assert_eq!(restored.len(), 3);
    }

    #[test]
    fn test_flush_and_reload_variable() {
        let mut col = Column::new("name".to_string(), 0, DataType::String, true);
        col.set(0, Some(&Value::String("Hello".to_string())))
            .unwrap();
        col.set(1, Some(&Value::String("World".to_string())))
            .unwrap();
        col.set(2, None).unwrap();

        let (data, offsets, bitmap) = col.get_flush_data();
        assert!(!offsets.is_empty());

        let mut restored = Column::new("name".to_string(), 0, DataType::String, true);
        restored.load_data_from_raw(data, offsets, bitmap.map(|b| b.into_vec()), 3);

        assert_eq!(restored.get(0), Some(Value::String("Hello".to_string())));
        assert_eq!(restored.get(1), Some(Value::String("World".to_string())));
        assert!(restored.is_null(2));
        assert_eq!(restored.len(), 3);
    }

    // ==================== P0 Priority Tests ====================

    /// Test: Verify large property values (>256 bytes) are handled correctly
    #[test]
    fn test_column_set_large_string_property() {
        let mut col = Column::new("description".to_string(), 0, DataType::String, false);

        // Create a string larger than typical storage boundaries
        let large_value = "a".repeat(1000);
        col.set(0, Some(&Value::String(large_value.clone())))
            .unwrap();
        col.set(1, Some(&Value::String("short".to_string())))
            .unwrap();

        assert_eq!(col.get(0), Some(Value::String(large_value)));
        assert_eq!(col.get(1), Some(Value::String("short".to_string())));
        assert_eq!(col.len(), 2);
    }

    /// Test: Verify updating single property doesn't affect others
    #[test]
    fn test_column_store_update_single_property_preserves_others() {
        let mut store = ColumnStore::new();
        store.add_column("name".to_string(), DataType::String, false);
        store.add_column("age".to_string(), DataType::Int, false);
        store.add_column("city".to_string(), DataType::String, false);

        // Insert initial row
        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(30)),
                    ("city".to_string(), Value::String("NYC".to_string())),
                ],
            )
            .unwrap();

        // Update only the age property
        store
            .set(
                0,
                &[
                    ("name".to_string(), Value::String("Alice".to_string())),
                    ("age".to_string(), Value::Int(31)),
                    ("city".to_string(), Value::String("NYC".to_string())),
                ],
            )
            .unwrap();

        // Verify all properties are correct
        assert_eq!(
            store.get_column("name").and_then(|col| col.get(0)),
            Some(Value::String("Alice".to_string()))
        );
        assert_eq!(
            store.get_column("age").and_then(|col| col.get(0)),
            Some(Value::Int(31))
        );
        assert_eq!(
            store.get_column("city").and_then(|col| col.get(0)),
            Some(Value::String("NYC".to_string()))
        );
    }

    /// Test: Verify very large property values can be stored and retrieved
    #[test]
    fn test_column_large_string_roundtrip() {
        let mut col = Column::new("data".to_string(), 0, DataType::String, false);

        // Test different sizes around potential boundaries
        let sizes = [255, 256, 257, 1000, 10000];
        for (idx, size) in sizes.iter().enumerate() {
            let value = format!("x-{}", "a".repeat(*size));
            col.set(idx, Some(&Value::String(value.clone())))
                .unwrap();
            assert_eq!(col.get(idx), Some(Value::String(value)), "Failed at size {}", size);
        }
    }

    /// Test: Verify string column with mixed null and non-null values
    #[test]
    fn test_column_string_with_nulls() {
        let mut col = Column::new("text".to_string(), 0, DataType::String, true);

        col.set(0, Some(&Value::String("hello".to_string())))
            .unwrap();
        col.set(1, None).unwrap();
        col.set(2, Some(&Value::String("world".to_string())))
            .unwrap();
        col.set(3, None).unwrap();

        assert_eq!(col.get(0), Some(Value::String("hello".to_string())));
        assert!(col.is_null(1));
        assert_eq!(col.get(2), Some(Value::String("world".to_string())));
        assert!(col.is_null(3));
        assert_eq!(col.null_count(), 2);
    }

    /// Test: Verify integer column type conversions and boundaries
    #[test]
    fn test_column_integer_types_boundaries() {
        let mut col_small = Column::new("small".to_string(), 0, DataType::SmallInt, false);
        col_small
            .set(0, Some(&Value::SmallInt(i16::MAX)))
            .unwrap();
        col_small
            .set(1, Some(&Value::SmallInt(i16::MIN)))
            .unwrap();
        assert_eq!(col_small.get(0), Some(Value::SmallInt(i16::MAX)));
        assert_eq!(col_small.get(1), Some(Value::SmallInt(i16::MIN)));

        let mut col_big = Column::new("big".to_string(), 0, DataType::BigInt, false);
        col_big.set(0, Some(&Value::BigInt(i64::MAX))).unwrap();
        col_big.set(1, Some(&Value::BigInt(i64::MIN))).unwrap();
        assert_eq!(col_big.get(0), Some(Value::BigInt(i64::MAX)));
        assert_eq!(col_big.get(1), Some(Value::BigInt(i64::MIN)));
    }

    /// Test: Verify float/double precision preservation
    #[test]
    fn test_column_float_precision() {
        let mut col_f = Column::new("float_val".to_string(), 0, DataType::Float, false);
        let f_value = 1.5_f32;
        col_f.set(0, Some(&Value::Float(f_value))).unwrap();
        assert_eq!(col_f.get(0), Some(Value::Float(f_value)));

        let mut col_d = Column::new("double_val".to_string(), 0, DataType::Double, false);
        let d_value = std::f64::consts::PI;
        col_d.set(0, Some(&Value::Double(d_value))).unwrap();
        assert_eq!(col_d.get(0), Some(Value::Double(d_value)));
    }

    /// Test: Verify column resize operation maintains data integrity
    #[test]
    fn test_column_resize_maintains_data() {
        let mut col = Column::new("num".to_string(), 0, DataType::Int, false);
        col.set(0, Some(&Value::Int(10))).unwrap();
        col.set(1, Some(&Value::Int(20))).unwrap();
        col.set(2, Some(&Value::Int(30))).unwrap();

        // Simulate resize operation
        col.resize(5);
        assert_eq!(col.len(), 5);

        // Verify original data is intact
        assert_eq!(col.get(0), Some(Value::Int(10)));
        assert_eq!(col.get(1), Some(Value::Int(20)));
        assert_eq!(col.get(2), Some(Value::Int(30)));
    }

    // ==================== P0 Priority Encoding Tests ====================

    /// Test: Column with repetitive integer values (RLE compression eligible)
    #[test]
    fn test_column_repetitive_integer_values() {
        let mut col = Column::new("status".to_string(), 0, DataType::Int, false);

        // Insert repetitive values that could benefit from RLE
        for i in 0..100 {
            let value = match i % 3 {
                0 => Value::Int(1),
                1 => Value::Int(2),
                _ => Value::Int(3),
            };
            col.set(i, Some(&value)).unwrap();
        }

        // Verify all values are stored correctly
        for i in 0..100 {
            let expected = match i % 3 {
                0 => Value::Int(1),
                1 => Value::Int(2),
                _ => Value::Int(3),
            };
            assert_eq!(col.get(i), Some(expected));
        }
    }

    /// Test: String column with low cardinality (Dictionary compression eligible)
    #[test]
    fn test_column_low_cardinality_strings() {
        let mut col = Column::new("category".to_string(), 0, DataType::String, false);

        let categories = ["A", "B", "C", "A", "B", "C"];

        // Insert low cardinality strings
        for (i, category) in categories.iter().enumerate() {
            col.set(i, Some(&Value::String(category.to_string())))
                .unwrap();
        }

        // Verify all values are stored and retrievable
        for (i, expected_category) in categories.iter().enumerate() {
            let value = col.get(i);
            assert_eq!(value, Some(Value::String(expected_category.to_string())));
        }
    }

    /// Test: Numeric column suitable for bitpacking
    #[test]
    fn test_column_small_range_integers() {
        let mut col = Column::new("priority".to_string(), 0, DataType::Int, false);

        // Insert values with small range [0-15] - good for bitpacking
        for i in 0..256 {
            let value = Value::Int((i % 16) as i32);
            col.set(i, Some(&value)).unwrap();
        }

        // Verify all values are correctly preserved
        for i in 0..256 {
            let expected = Value::Int((i % 16) as i32);
            assert_eq!(col.get(i), Some(expected));
        }
    }

    /// Test: Long string column suitable for FSST compression
    #[test]
    fn test_column_long_strings_compression() {
        let mut col = Column::new("description".to_string(), 0, DataType::String, false);

        let long_strings = [
            "The quick brown fox jumps over the lazy dog",
            "A Rust programming language feature",
            "GraphDB storage compression techniques",
            "The quick brown fox jumps over the lazy dog", // Repetition
            "Efficient data compression algorithms",
        ];

        // Insert long strings
        for (i, s) in long_strings.iter().enumerate() {
            col.set(i, Some(&Value::String(s.to_string())))
                .unwrap();
        }

        // Verify retrieval works correctly
        for (i, expected_str) in long_strings.iter().enumerate() {
            assert_eq!(
                col.get(i),
                Some(Value::String(expected_str.to_string()))
            );
        }
    }

    /// Test: i64 boundary values
    #[test]
    fn test_column_i64_boundaries() {
        let mut col = Column::new("bigint_val".to_string(), 0, DataType::BigInt, false);

        // Test MAX and MIN values
        col.set(0, Some(&Value::BigInt(i64::MAX))).unwrap();
        col.set(1, Some(&Value::BigInt(i64::MIN))).unwrap();
        col.set(2, Some(&Value::BigInt(0))).unwrap();

        assert_eq!(col.get(0), Some(Value::BigInt(i64::MAX)));
        assert_eq!(col.get(1), Some(Value::BigInt(i64::MIN)));
        assert_eq!(col.get(2), Some(Value::BigInt(0)));
    }

    /// Test: Empty string handling
    #[test]
    fn test_column_empty_string() {
        let mut col = Column::new("text".to_string(), 0, DataType::String, false);

        // Test empty string
        col.set(0, Some(&Value::String("".to_string())))
            .unwrap();
        col.set(1, Some(&Value::String("normal".to_string())))
            .unwrap();

        assert_eq!(col.get(0), Some(Value::String("".to_string())));
        assert_eq!(col.get(1), Some(Value::String("normal".to_string())));
    }

    /// Test: Special characters in strings
    #[test]
    fn test_column_special_characters() {
        let mut col = Column::new("special".to_string(), 0, DataType::String, false);

        let special_strings = [
            "\n\t\r",                    // Whitespace
            "\\\"'",                      // Quotes and backslash
            "你好世界🌍",                   // Unicode and emoji
            "\0null",                     // Control character
        ];

        for (idx, s) in special_strings.iter().enumerate() {
            col.set(idx, Some(&Value::String(s.to_string())))
                .unwrap();
            assert_eq!(col.get(idx), Some(Value::String(s.to_string())));
        }
    }

    /// Test: Float special values
    #[test]
    fn test_column_float_special_values() {
        let mut col = Column::new("float_val".to_string(), 0, DataType::Float, false);

        // Test normal, zero, negative
        col.set(0, Some(&Value::Float(0.0))).unwrap();
        col.set(1, Some(&Value::Float(-1.5))).unwrap();
        col.set(2, Some(&Value::Float(f32::MAX))).unwrap();
        col.set(3, Some(&Value::Float(f32::MIN))).unwrap();

        assert_eq!(col.get(0), Some(Value::Float(0.0)));
        assert_eq!(col.get(1), Some(Value::Float(-1.5)));
        assert_eq!(col.get(2), Some(Value::Float(f32::MAX)));
        assert_eq!(col.get(3), Some(Value::Float(f32::MIN)));
    }
}

