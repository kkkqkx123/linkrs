//! BitPacking Encoding
//!
//! Compresses integer columns by storing values using minimal bits.
//! Effective for columns with small value ranges.

use std::io::{Read, Write};

use bitvec::prelude::*;

use crate::core::{DataType, StorageError, StorageResult, Value};

#[derive(Debug, Clone)]
pub struct BitPackedColumn {
    data: BitVec<u8, Lsb0>,
    bit_width: u8,
    min_value: i64,
    row_count: usize,
    null_bitmap: Option<BitVec<u8, Lsb0>>,
}

impl BitPackedColumn {
    pub fn new() -> Self {
        Self {
            data: BitVec::new(),
            bit_width: 64,
            min_value: 0,
            row_count: 0,
            null_bitmap: None,
        }
    }

    pub fn analyze(values: &[i64]) -> Self {
        if values.is_empty() {
            return Self::new();
        }

        let min_val = *values.iter().min().unwrap_or(&0);
        let max_val = *values.iter().max().unwrap_or(&0);

        let range = (max_val - min_val) as u64;
        let bit_width = Self::calculate_bit_width(range);

        let mut column = Self {
            data: BitVec::with_capacity(values.len() * bit_width as usize),
            bit_width,
            min_value: min_val,
            row_count: 0,
            null_bitmap: None,
        };

        for &val in values {
            column.append_value(val);
        }

        column
    }

    pub fn analyze_nullable(values: &[Option<i64>]) -> Self {
        let non_null: Vec<i64> = values.iter().filter_map(|v| *v).collect();

        if non_null.is_empty() {
            return Self {
                data: BitVec::new(),
                bit_width: 1,
                min_value: 0,
                row_count: values.len(),
                null_bitmap: Some(BitVec::repeat(false, values.len())),
            };
        }

        let min_val = *non_null.iter().min().unwrap_or(&0);
        let max_val = *non_null.iter().max().unwrap_or(&0);
        let range = (max_val - min_val) as u64;
        let bit_width = Self::calculate_bit_width(range).max(1);

        let mut column = Self {
            data: BitVec::with_capacity(values.len() * bit_width as usize),
            bit_width,
            min_value: min_val,
            row_count: 0,
            null_bitmap: Some(BitVec::with_capacity(values.len())),
        };

        for val in values {
            column.append_optional(*val);
        }

        column
    }

    fn calculate_bit_width(range: u64) -> u8 {
        if range == 0 {
            return 1;
        }
        (64 - range.leading_zeros()) as u8
    }

    pub fn append_value(&mut self, value: i64) {
        let adjusted = (value - self.min_value) as u64;
        self.append_bits(adjusted);
        self.row_count += 1;
    }

    pub fn append_optional(&mut self, value: Option<i64>) {
        if let Some(ref mut bitmap) = self.null_bitmap {
            match value {
                Some(v) => {
                    bitmap.push(false);
                    self.append_value(v);
                }
                None => {
                    bitmap.push(true);
                    self.append_bits(0);
                    self.row_count += 1;
                }
            }
        } else {
            if let Some(v) = value {
                self.append_value(v);
            }
        }
    }

    fn append_bits(&mut self, value: u64) {
        for i in 0..self.bit_width {
            let bit = (value >> i) & 1;
            self.data.push(bit == 1);
        }
    }

    pub fn get(&self, row_idx: usize) -> Option<i64> {
        if row_idx >= self.row_count {
            return None;
        }

        if self.is_null(row_idx) {
            return None;
        }

        let bit_offset = row_idx * self.bit_width as usize;
        let mut value: u64 = 0;

        for i in 0..self.bit_width as usize {
            let bit_idx = bit_offset + i;
            if bit_idx >= self.data.len() {
                break;
            }
            if self.data[bit_idx] {
                value |= 1u64 << i;
            }
        }

        Some(value as i64 + self.min_value)
    }

    pub fn is_null(&self, row_idx: usize) -> bool {
        self.null_bitmap
            .as_ref()
            .map(|b| row_idx < b.len() && b[row_idx])
            .unwrap_or(false)
    }

    pub fn set(&mut self, row_idx: usize, value: Option<i64>) -> StorageResult<()> {
        if row_idx >= self.row_count {
            return Err(StorageError::invalid_input(format!(
                "Index {} out of bounds (len: {})",
                row_idx, self.row_count
            )));
        }

        match value {
            Some(v) => {
                let adjusted = (v - self.min_value) as u64;
                let bit_offset = row_idx * self.bit_width as usize;

                for i in 0..self.bit_width as usize {
                    let bit = (adjusted >> i) & 1;
                    let bit_idx = bit_offset + i;
                    if bit_idx < self.data.len() {
                        self.data.set(bit_idx, bit == 1);
                    }
                }

                if let Some(ref mut bitmap) = self.null_bitmap {
                    if row_idx < bitmap.len() {
                        bitmap.set(row_idx, false);
                    }
                }
            }
            None => {
                if let Some(ref mut bitmap) = self.null_bitmap {
                    if row_idx < bitmap.len() {
                        bitmap.set(row_idx, true);
                    }
                } else {
                    return Err(StorageError::null_value_not_allowed(
                        "bitpacked_column".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.row_count
    }

    pub fn memory_usage(&self) -> usize {
        let data_size = self.data.len().div_ceil(8);
        let null_size = self
            .null_bitmap
            .as_ref()
            .map(|b| b.len().div_ceil(8))
            .unwrap_or(0);
        data_size + null_size + std::mem::size_of::<Self>()
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&self.bit_width.to_le_bytes())?;
        writer.write_all(&self.min_value.to_le_bytes())?;
        writer.write_all(&(self.row_count as u32).to_le_bytes())?;
        written += 12;
        let has_bitmap = self.null_bitmap.is_some() as u8;
        writer.write_all(&[has_bitmap])?;
        written += 1;
        if let Some(ref bitmap) = self.null_bitmap {
            let bitmap_bytes = bitmap.as_raw_slice();
            writer.write_all(&(bitmap_bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bitmap_bytes)?;
            written += 4 + bitmap_bytes.len();
        }
        let data_bytes = self.data.as_raw_slice();
        writer.write_all(&(data_bytes.len() as u32).to_le_bytes())?;
        writer.write_all(data_bytes)?;
        written += 4 + data_bytes.len();
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut bit_width_buf = [0u8; 1];
        reader.read_exact(&mut bit_width_buf)?;
        let bit_width = bit_width_buf[0];
        let mut min_value_bytes = [0u8; 8];
        reader.read_exact(&mut min_value_bytes)?;
        let min_value = i64::from_le_bytes(min_value_bytes);
        let mut row_count_bytes = [0u8; 4];
        reader.read_exact(&mut row_count_bytes)?;
        let row_count = u32::from_le_bytes(row_count_bytes) as usize;
        let mut has_bitmap_buf = [0u8; 1];
        reader.read_exact(&mut has_bitmap_buf)?;
        let has_bitmap = has_bitmap_buf[0] != 0;
        let null_bitmap = if has_bitmap {
            let mut bitmap_len_bytes = [0u8; 4];
            reader.read_exact(&mut bitmap_len_bytes)?;
            let bitmap_len = u32::from_le_bytes(bitmap_len_bytes) as usize;
            let mut bitmap_bytes = vec![0u8; bitmap_len];
            reader.read_exact(&mut bitmap_bytes)?;
            Some(BitVec::<u8, Lsb0>::from_slice(&bitmap_bytes))
        } else {
            None
        };
        let mut data_len_bytes = [0u8; 4];
        reader.read_exact(&mut data_len_bytes)?;
        let data_len = u32::from_le_bytes(data_len_bytes) as usize;
        let mut data_bytes = vec![0u8; data_len];
        reader.read_exact(&mut data_bytes)?;
        let data = BitVec::<u8, Lsb0>::from_vec(data_bytes);
        Ok(Self {
            data,
            bit_width,
            min_value,
            row_count,
            null_bitmap,
        })
    }
}

impl Default for BitPackedColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct BitPackedIntColumn {
    packed: BitPackedColumn,
    data_type: DataType,
}

impl BitPackedIntColumn {
    pub fn analyze(values: &[Option<Value>], data_type: DataType) -> StorageResult<Self> {
        let int_values: Vec<Option<i64>> = values
            .iter()
            .map(|v| {
                v.as_ref().and_then(|val| match val {
                    Value::SmallInt(i) => Some(*i as i64),
                    Value::Int(i) => Some(*i as i64),
                    Value::BigInt(i) => Some(*i),
                    _ => None,
                })
            })
            .collect();

        let packed = BitPackedColumn::analyze_nullable(&int_values);

        Ok(Self { packed, data_type })
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        let raw = self.packed.get(row_idx)?;
        match self.data_type {
            DataType::SmallInt => Some(Value::SmallInt(raw as i16)),
            DataType::Int => Some(Value::Int(raw as i32)),
            DataType::BigInt => Some(Value::BigInt(raw)),
            _ => None,
        }
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let int_val = value.and_then(|v| match v {
            Value::SmallInt(i) => Some(*i as i64),
            Value::Int(i) => Some(*i as i64),
            Value::BigInt(i) => Some(*i),
            _ => None,
        });

        self.packed.set(row_idx, int_val)
    }

    pub fn len(&self) -> usize {
        self.packed.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.packed.memory_usage()
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        let type_tag = match self.data_type {
            DataType::SmallInt => 1u8,
            DataType::Int => 2u8,
            DataType::BigInt => 3u8,
            _ => 0u8,
        };
        writer.write_all(&[type_tag])?;
        written += 1;
        written += self.packed.serialize_meta(writer)?;
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut type_tag_buf = [0u8; 1];
        reader.read_exact(&mut type_tag_buf)?;
        let data_type = match type_tag_buf[0] {
            1 => DataType::SmallInt,
            2 => DataType::Int,
            3 => DataType::BigInt,
            _ => DataType::BigInt,
        };
        let packed = BitPackedColumn::deserialize_meta(reader)?;
        Ok(Self { packed, data_type })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_bitpacking(values: &[i64]) -> bool {
        if values.is_empty() {
            return false;
        }
        let min = values.iter().min().copied().unwrap_or_default();
        let max = values.iter().max().copied().unwrap_or_default();
        (max - min) < (1 << 20)
    }

    #[test]
    fn test_bitpacking_basic() {
        let values = vec![10, 20, 30, 40, 50];
        let column = BitPackedColumn::analyze(&values);

        assert_eq!(column.len(), 5);
        assert_eq!(column.get(0), Some(10));
        assert_eq!(column.get(4), Some(50));
    }

    #[test]
    fn test_bitpacking_small_range() {
        let values: Vec<i64> = (0..100).collect();
        let column = BitPackedColumn::analyze(&values);

        let original_size = 100 * 8;
        let compressed_size = column.memory_usage();
        assert!(compressed_size < original_size);
    }

    #[test]
    fn test_bitpacking_nullable() {
        let values = vec![Some(10), None, Some(30), None, Some(50)];
        let column = BitPackedColumn::analyze_nullable(&values);

        assert_eq!(column.len(), 5);
        assert_eq!(column.get(0), Some(10));
        assert!(column.is_null(1));
        assert_eq!(column.get(2), Some(30));
        assert!(column.is_null(3));
    }

    #[test]
    fn test_bitpacking_set() {
        let values = vec![10, 20, 30];
        let mut column = BitPackedColumn::analyze(&values);

        column.set(1, Some(25)).unwrap();
        assert_eq!(column.get(1), Some(25));
    }

    #[test]
    fn test_bitpacking_memory_usage() {
        let values: Vec<i64> = (0..1000).collect();
        let column = BitPackedColumn::analyze(&values);

        let original_size = 1000 * 8;
        let compressed_size = column.memory_usage();

        assert!(compressed_size < original_size);
    }

    #[test]
    fn test_select_bitpacking() {
        let small_range: Vec<i64> = (0..1000).collect();
        assert!(select_bitpacking(&small_range));

        let large_range: Vec<i64> = (0..10_000_000_000i64).step_by(1_000_000_000).collect();
        assert!(!select_bitpacking(&large_range));
    }

    #[test]
    fn test_bitpacked_int_column() {
        let values = vec![Some(Value::Int(10)), None, Some(Value::Int(30))];

        let column = BitPackedIntColumn::analyze(&values, DataType::Int).unwrap();

        assert_eq!(column.get(0), Some(Value::Int(10)));
        assert!(column.get(1).is_none());
        assert_eq!(column.get(2), Some(Value::Int(30)));
    }
}
