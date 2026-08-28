//! ALP (Adaptive Lossless floating-Point) Compression
//!
//! Lossless compression for floating-point numbers by converting them
//! to integers through multiplication by a power of 10, then applying
//! BitPacking. Values that cannot be losslessly encoded with the chosen
//! factor are stored in an exception table, guaranteeing bit-exact round-trips.
//!
//! # Algorithm
//!
//! 1. Analyze float values to find optimal exponent k
//! 2. Multiply each value by 10^k to convert to integer
//! 3. Values that round-trip losslessly → BitPacking
//! 4. Values that don't round-trip → exception table
//! 5. Decompression reverses the process, patches from exception table

use std::io::{Read, Write};

use super::bitpacking::BitPackedColumn;
use graphdb_core::NullBitmap;
use graphdb_core::{DataType, StorageError, StorageResult, Value};

const ALP_EPSILON: f64 = f64::EPSILON;

#[derive(Debug, Clone)]
pub struct ExceptionEntry {
    pub row_idx: u32,
    pub original_value: f64,
}

#[derive(Debug, Clone)]
pub struct AlpEncoder {
    factor: f64,
    bit_packed: BitPackedColumn,
    exceptions: Vec<ExceptionEntry>,
}

impl AlpEncoder {
    pub fn new() -> Self {
        Self {
            factor: 0.0,
            bit_packed: BitPackedColumn::new(),
            exceptions: Vec::new(),
        }
    }

    pub fn analyze(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self {
                factor: 1.0,
                bit_packed: BitPackedColumn::new(),
                exceptions: Vec::new(),
            };
        }
        let factor = 10f64.powi(Self::find_optimal_exponent(values) as i32);
        let mut int_values = Vec::with_capacity(values.len());
        let mut exceptions = Vec::new();
        for (idx, &value) in values.iter().enumerate() {
            let scaled = value * factor;
            if scaled.is_finite() && scaled.abs() < i64::MAX as f64 {
                let int_value = scaled.round() as i64;
                if (int_value as f64 / factor - value).abs() < ALP_EPSILON {
                    int_values.push(int_value);
                    continue;
                }
            }
            exceptions.push(ExceptionEntry {
                row_idx: idx as u32,
                original_value: value,
            });
            int_values.push(0);
        }
        Self {
            factor,
            bit_packed: BitPackedColumn::analyze(&int_values),
            exceptions,
        }
    }

    fn find_optimal_exponent(values: &[f64]) -> i8 {
        let mut best_exponent = 0;
        let mut best_bit_width = u8::MAX;
        let mut best_valid_count = 0;
        for exponent in -10..=10 {
            let factor = 10f64.powi(exponent);
            let mut int_values = Vec::with_capacity(values.len());
            let mut valid_count = 0;
            for &value in values {
                let scaled = value * factor;
                if scaled.is_finite() && scaled.abs() < i64::MAX as f64 {
                    let int_value = scaled.round() as i64;
                    if (int_value as f64 / factor - value).abs() < ALP_EPSILON {
                        int_values.push(int_value);
                        valid_count += 1;
                        continue;
                    }
                }
                int_values.push(0);
            }
            if valid_count == 0 {
                continue;
            }
            let min_value = *int_values.iter().min().unwrap_or(&0);
            let max_value = *int_values.iter().max().unwrap_or(&0);
            let range = max_value.saturating_sub(min_value) as u64;
            let bit_width = if range == 0 {
                1
            } else {
                (64 - range.leading_zeros()) as u8
            };
            if valid_count > best_valid_count
                || (valid_count == best_valid_count && bit_width < best_bit_width)
            {
                best_valid_count = valid_count;
                best_bit_width = bit_width;
                best_exponent = exponent as i8;
            }
        }
        best_exponent
    }

    pub fn compress(&self, value: f64) -> i64 {
        (value * self.factor).round() as i64
    }

    pub fn exceptions(&self) -> &[ExceptionEntry] {
        &self.exceptions
    }

    pub fn decompress(&self, value: i64) -> f64 {
        value as f64 / self.factor
    }

    pub fn memory_usage(&self) -> usize {
        self.bit_packed.memory_usage()
            + self.exceptions.capacity() * std::mem::size_of::<ExceptionEntry>()
            + std::mem::size_of::<Self>()
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&self.factor.to_le_bytes())?;
        written += 8;
        written += self.bit_packed.serialize_meta(writer)?;
        let exc_count = self.exceptions.len() as u32;
        writer.write_all(&exc_count.to_le_bytes())?;
        written += 4;
        for exc in &self.exceptions {
            writer.write_all(&exc.row_idx.to_le_bytes())?;
            writer.write_all(&exc.original_value.to_le_bytes())?;
            written += 12;
        }
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut factor_bytes = [0u8; 8];
        reader.read_exact(&mut factor_bytes)?;
        let factor = f64::from_le_bytes(factor_bytes);
        let bit_packed = BitPackedColumn::deserialize_meta(reader)?;
        let mut exc_count_bytes = [0u8; 4];
        reader.read_exact(&mut exc_count_bytes)?;
        let exc_count = u32::from_le_bytes(exc_count_bytes) as usize;
        let mut exceptions = Vec::with_capacity(exc_count);
        for _ in 0..exc_count {
            let mut row_idx_bytes = [0u8; 4];
            reader.read_exact(&mut row_idx_bytes)?;
            let row_idx = u32::from_le_bytes(row_idx_bytes);
            let mut val_bytes = [0u8; 8];
            reader.read_exact(&mut val_bytes)?;
            let original_value = f64::from_le_bytes(val_bytes);
            exceptions.push(ExceptionEntry {
                row_idx,
                original_value,
            });
        }
        Ok(Self {
            factor,
            bit_packed,
            exceptions,
        })
    }
}

impl Default for AlpEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct AlpColumn {
    encoder: AlpEncoder,
    row_count: usize,
    null_bitmap: NullBitmap,
    data_type: DataType,
}

impl AlpColumn {
    pub fn new() -> Self {
        Self {
            encoder: AlpEncoder::new(),
            row_count: 0,
            null_bitmap: NullBitmap::new(),
            data_type: DataType::Double,
        }
    }

    pub fn analyze_f64(values: &[Option<f64>]) -> Self {
        let row_count = values.len();
        let null_bitmap = Self::build_bitmap(values);
        let dense: Vec<f64> = values.iter().map(|value| value.unwrap_or(0.0)).collect();
        Self {
            encoder: AlpEncoder::analyze(&dense),
            row_count,
            null_bitmap,
            data_type: DataType::Double,
        }
    }

    fn build_bitmap(values: &[Option<f64>]) -> NullBitmap {
        let mut bitmap = NullBitmap::with_capacity(values.len());
        for value in values {
            bitmap.push(value.is_none());
        }
        bitmap
    }

    pub fn analyze_f32(values: &[Option<f32>]) -> Self {
        let values: Vec<Option<f64>> = values.iter().map(|value| value.map(f64::from)).collect();
        let mut column = Self::analyze_f64(&values);
        column.data_type = DataType::Float;
        column
    }

    pub fn analyze_values(values: &[Option<Value>], data_type: DataType) -> StorageResult<Self> {
        match data_type {
            DataType::Float => {
                let values = values
                    .iter()
                    .map(|value| match value {
                        Some(Value::Float(value)) => Some(*value),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                Ok(Self::analyze_f32(&values))
            }
            DataType::Double => {
                let values = values
                    .iter()
                    .map(|value| match value {
                        Some(Value::Double(value)) => Some(*value),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                Ok(Self::analyze_f64(&values))
            }
            _ => Err(StorageError::invalid_input(format!(
                "ALP compression not supported for {:?}",
                data_type
            ))),
        }
    }

    pub fn get(&self, row_idx: usize) -> Option<f64> {
        if row_idx >= self.row_count || self.null_bitmap.is_null(row_idx) {
            return None;
        }

        for exc in self.encoder.exceptions() {
            if exc.row_idx as usize == row_idx {
                return Some(exc.original_value);
            }
        }

        let int_val = self.encoder.bit_packed.get(row_idx)?;
        Some(self.encoder.decompress(int_val))
    }

    pub fn get_value(&self, row_idx: usize) -> Option<Value> {
        self.get(row_idx).map(|value| match self.data_type {
            DataType::Float => Value::Float(value as f32),
            _ => Value::Double(value),
        })
    }

    pub fn set(&mut self, row_idx: usize, value: Option<f64>) -> StorageResult<()> {
        if row_idx >= self.row_count {
            return Err(StorageError::invalid_input(format!(
                "Index {} out of bounds (len: {})",
                row_idx, self.row_count
            )));
        }

        match value {
            Some(v) => {
                let int_val = self.encoder.compress(v);
                let scaled = v * self.encoder.factor;
                let is_lossless = scaled.is_finite()
                    && scaled.abs() < i64::MAX as f64
                    && (int_val as f64 / self.encoder.factor - v).abs() < ALP_EPSILON;

                let fits_in_range = is_lossless && self.encoder.bit_packed.fits_value(int_val);

                if fits_in_range {
                    self.encoder.bit_packed.set(row_idx, Some(int_val))?;
                    self.encoder
                        .exceptions
                        .retain(|e| e.row_idx as usize != row_idx);
                } else if is_lossless {
                    self.encoder.bit_packed.set(row_idx, Some(0))?;
                    match self
                        .encoder
                        .exceptions
                        .iter()
                        .position(|e| e.row_idx as usize == row_idx)
                    {
                        Some(pos) => self.encoder.exceptions[pos].original_value = v,
                        None => self.encoder.exceptions.push(ExceptionEntry {
                            row_idx: row_idx as u32,
                            original_value: v,
                        }),
                    }
                } else {
                    self.encoder.bit_packed.set(row_idx, Some(0))?;
                    match self
                        .encoder
                        .exceptions
                        .iter()
                        .position(|e| e.row_idx as usize == row_idx)
                    {
                        Some(pos) => self.encoder.exceptions[pos].original_value = v,
                        None => self.encoder.exceptions.push(ExceptionEntry {
                            row_idx: row_idx as u32,
                            original_value: v,
                        }),
                    }
                }
                self.null_bitmap.set(row_idx, false);
            }
            None => {
                self.null_bitmap.set(row_idx, true);
            }
        }

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.row_count
    }

    pub fn memory_usage(&self) -> usize {
        self.encoder.memory_usage() + self.null_bitmap.memory_usage()
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&[if self.data_type == DataType::Float {
            1
        } else {
            2
        }])?;
        written += 1;
        writer.write_all(&(self.row_count as u32).to_le_bytes())?;
        written += 4;
        written += self.encoder.serialize_meta(writer)?;
        let bm_len = self.null_bitmap.len() as u32;
        writer.write_all(&bm_len.to_le_bytes())?;
        written += 4;
        for &word in self.null_bitmap.as_bits() {
            writer.write_all(&word.to_le_bytes())?;
            written += 8;
        }
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut type_byte = [0u8; 1];
        reader.read_exact(&mut type_byte)?;
        let data_type = match type_byte[0] {
            1 => DataType::Float,
            2 => DataType::Double,
            value => {
                return Err(StorageError::deserialize_error(format!(
                    "invalid ALP data type: {}",
                    value
                )))
            }
        };
        let mut rc_bytes = [0u8; 4];
        reader.read_exact(&mut rc_bytes)?;
        let row_count = u32::from_le_bytes(rc_bytes) as usize;
        let encoder = AlpEncoder::deserialize_meta(reader)?;
        let mut bm_len_bytes = [0u8; 4];
        reader.read_exact(&mut bm_len_bytes)?;
        let bm_len = u32::from_le_bytes(bm_len_bytes) as usize;
        let words = bm_len.div_ceil(64);
        let mut data = Vec::with_capacity(words);
        for _ in 0..words {
            let mut word_bytes = [0u8; 8];
            reader.read_exact(&mut word_bytes)?;
            data.push(u64::from_le_bytes(word_bytes));
        }
        let null_bitmap = NullBitmap::from_raw(data, bm_len);
        Ok(Self {
            encoder,
            row_count,
            null_bitmap,
            data_type,
        })
    }
}

impl Default for AlpColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    fn select_alp(values: &[f64]) -> bool {
        if values.len() < 64 {
            return false;
        }
        values.iter().any(|value| value.fract() != 0.0)
    }

    #[test]
    fn test_select_alp() {
        let integers: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        assert!(!select_alp(&integers));

        let decimals: Vec<f64> = (0..1000).map(|i| i as f64 * 0.01).collect();
        assert!(select_alp(&decimals));
    }
}
