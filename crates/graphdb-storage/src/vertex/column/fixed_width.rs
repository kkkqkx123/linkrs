use graphdb_core::value::{DateValue, DateTimeValue, TimeValue};
use graphdb_core::{DataType, StorageError, StorageResult, Value};

use super::{ensure_bitmap_len, ColumnStorage};
use bitvec::prelude::*;

/// Column storage for fixed-width (primitive) types.
///
/// Values are stored in a flat `Vec<u8>` with direct offset calculation:
/// `offset = row_idx * element_size`.
/// This provides O(1) random access without any branching on type.
#[derive(Debug, Clone)]
pub struct FixedWidthColumn {
    pub(super) data: Vec<u8>,
    pub(super) data_type: DataType,
    pub(super) element_size: usize,
    pub(super) null_bitmap: Option<BitVec<u8, Lsb0>>,
    pub(super) row_count: usize,
    /// O(1) count of null rows, maintained incrementally on set/resize/clear
    /// so `used_memory_size` does not rescan the bitmap (which is O(n)).
    pub(super) null_count: usize,
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
            null_count: 0,
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
        let was_null = self
            .null_bitmap
            .as_ref()
            .map(|b| row_idx < b.len() && b[row_idx])
            .unwrap_or(false);

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

        if self.null_bitmap.is_some() {
            match value {
                Some(v) if !v.is_null() => {
                    if was_null {
                        self.null_count = self.null_count.saturating_sub(1);
                    }
                }
                _ => {
                    if !was_null {
                        self.null_count += 1;
                    }
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

    fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional * self.element_size.max(1));
        if let Some(ref mut bitmap) = self.null_bitmap {
            bitmap.reserve(additional);
        }
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
        self.null_count = 0;
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
        if let Some(bitmap) = &self.null_bitmap {
            if new_count >= old_count {
                self.null_count += new_count - old_count;
            } else {
                // Shrink: recompute (rare; happens during load/compaction).
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
        self.null_count = self
            .null_bitmap
            .as_ref()
            .map(|b| b.count_ones())
            .unwrap_or(0);
        self.row_count = self.data.len() / elem_size;
    }

    fn get_flush_data(&self) -> (Vec<u8>, Vec<u64>, Option<BitVec<u8, Lsb0>>) {
        (self.data.clone(), Vec::new(), self.null_bitmap.clone())
    }
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
        DataType::DateTime => 28,
        DataType::Uuid => 16,
        _ => 0,
    }
}

pub(crate) fn write_fixed_value(
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
            offset,
            required_size,
            data.len(),
            element_size
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

pub(crate) fn read_fixed_value(data: &[u8], offset: usize, element_size: usize) -> Option<Value> {
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
pub(crate) fn convert_to_type(raw: Value, data_type: &DataType) -> Value {
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
