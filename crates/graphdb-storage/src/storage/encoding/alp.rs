//! ALP (Adaptive Lossless floating-Point) Compression
//!
//! Lossless compression for floating-point numbers by converting them
//! to integers through multiplication by a power of 10, then applying
//! BitPacking.
//!
//! # Algorithm
//!
//! 1. Analyze float values to find optimal exponent k
//! 2. Multiply each value by 10^k to convert to integer
//! 3. Apply BitPacking on the integers
//! 4. Decompression reverses the process

use super::bitpacking::BitPackedColumn;
use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::utils::NullBitmap;

#[derive(Debug, Clone)]
pub struct AlpEncoder {
    factor: f64,
    bit_packed: BitPackedColumn,
}

impl AlpEncoder {
    pub fn new() -> Self {
        Self {
            factor: 0.0,
            bit_packed: BitPackedColumn::new(),
        }
    }

    pub fn analyze(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self {
                factor: 1.0,
                bit_packed: BitPackedColumn::new(),
            };
        }

        let best_exponent = Self::find_optimal_exponent(values);

        let factor = 10f64.powi(best_exponent as i32);
        let int_values: Vec<i64> = values
            .iter()
            .map(|&v| (v * factor).round() as i64)
            .collect();

        let bit_packed = BitPackedColumn::analyze(&int_values);

        Self {
            factor: 10f64.powi(best_exponent as i32),
            bit_packed,
        }
    }

    fn find_optimal_exponent(values: &[f64]) -> i8 {
        let mut best_exponent: i8 = 0;
        let mut best_bit_width = 64;

        for exp in -10..=10 {
            let factor = 10f64.powi(exp as i32);
            let mut valid = true;
            let mut int_values = Vec::with_capacity(values.len());

            for &v in values {
                let scaled = v * factor;
                if scaled.is_finite() && scaled.abs() < i64::MAX as f64 {
                    let int_val = scaled.round() as i64;
                    if (int_val as f64 / factor - v).abs() < 1e-9 {
                        int_values.push(int_val);
                    } else {
                        valid = false;
                        break;
                    }
                } else {
                    valid = false;
                    break;
                }
            }

            if valid && !int_values.is_empty() {
                let min_val = *int_values.iter().min().unwrap_or(&0);
                let max_val = *int_values.iter().max().unwrap_or(&0);
                let range = (max_val - min_val) as u64;

                let bit_width = if range == 0 {
                    1
                } else {
                    (64 - range.leading_zeros()) as u8
                };

                if bit_width < best_bit_width {
                    best_bit_width = bit_width;
                    best_exponent = exp;
                }
            }
        }

        best_exponent
    }

    pub fn compress(&self, value: f64) -> i64 {
        (value * self.factor).round() as i64
    }

    pub fn decompress(&self, value: i64) -> f64 {
        value as f64 / self.factor
    }

    pub fn memory_usage(&self) -> usize {
        self.bit_packed.memory_usage() + std::mem::size_of::<Self>()
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
}

impl AlpColumn {
    pub fn new() -> Self {
        Self {
            encoder: AlpEncoder::new(),
            row_count: 0,
            null_bitmap: NullBitmap::new(),
        }
    }

    pub fn analyze_f64(values: &[Option<f64>]) -> Self {
        let non_null: Vec<f64> = values.iter().filter_map(|v| *v).collect();

        if non_null.is_empty() {
            return Self {
                row_count: values.len(),
                null_bitmap: Self::build_bitmap(values),
                ..Default::default()
            };
        }

        let encoder = AlpEncoder::analyze(&non_null);

        let int_values: Vec<Option<i64>> = values
            .iter()
            .map(|v| v.map(|val| encoder.compress(val)))
            .collect();

        let bit_packed = BitPackedColumn::analyze_nullable(&int_values);

        Self {
            encoder: AlpEncoder {
                factor: encoder.factor,
                bit_packed,
            },
            row_count: values.len(),
            null_bitmap: Self::build_bitmap(values),
        }
    }

    fn build_bitmap(values: &[Option<f64>]) -> NullBitmap {
        let mut bitmap = NullBitmap::with_capacity(values.len());
        for v in values {
            bitmap.push(v.is_none());
        }
        bitmap
    }

    pub fn analyze_f32(values: &[Option<f32>]) -> Self {
        let f64_values: Vec<Option<f64>> = values.iter().map(|v| v.map(|x| x as f64)).collect();
        Self::analyze_f64(&f64_values)
    }

    pub fn analyze_values(values: &[Option<Value>], data_type: DataType) -> StorageResult<Self> {
        match data_type {
            DataType::Float => {
                let floats: Vec<Option<f32>> = values
                    .iter()
                    .map(|v| {
                        v.as_ref().and_then(|val| {
                            if let Value::Float(f) = val {
                                Some(*f)
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                Ok(Self::analyze_f32(&floats))
            }
            DataType::Double => {
                let doubles: Vec<Option<f64>> = values
                    .iter()
                    .map(|v| {
                        v.as_ref().and_then(|val| {
                            if let Value::Double(d) = val {
                                Some(*d)
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                Ok(Self::analyze_f64(&doubles))
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

        let int_val = self.encoder.bit_packed.get(row_idx)?;
        Some(self.encoder.decompress(int_val))
    }

    pub fn get_value(&self, row_idx: usize) -> Option<Value> {
        self.get(row_idx).map(Value::Double)
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
                self.encoder.bit_packed.set(row_idx, Some(int_val))?;
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
}

impl Default for AlpColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select_alp(values: &[f64]) -> bool {
        if values.len() < 64 {
            return false;
        }
        values.iter().any(|value| value.fract() != 0.0)
    }

    #[test]
    fn test_alp_encoder_basic() {
        let values = vec![1.5, 2.5, 3.5, 4.5, 5.5];
        let encoder = AlpEncoder::analyze(&values);

        for &v in &values {
            let compressed = encoder.compress(v);
            let decompressed = encoder.decompress(compressed);
            assert!((decompressed - v).abs() < 1e-9);
        }
    }

    #[test]
    fn test_alp_encoder_compression() {
        let values: Vec<f64> = (0..1000).map(|i| i as f64 * 0.01).collect();
        let encoder = AlpEncoder::analyze(&values);

        let original_size = values.len() * 8;
        let compressed_size = encoder.memory_usage();

        assert!(compressed_size < original_size);
    }

    #[test]
    fn test_alp_column_f64() {
        let values = vec![Some(1.5), None, Some(3.5), Some(5.5)];

        let column = AlpColumn::analyze_f64(&values);

        assert_eq!(column.len(), 4);
        assert!((column.get(0).unwrap() - 1.5).abs() < 1e-9);
        assert!(column.get(1).is_none());
        assert!((column.get(2).unwrap() - 3.5).abs() < 1e-9);
    }

    #[test]
    fn test_alp_column_set() {
        let values = vec![Some(1.5), Some(2.5)];
        let mut column = AlpColumn::analyze_f64(&values);

        let original = column.get(0).unwrap();
        assert!((original - 1.5).abs() < 1e-9);

        column.set(0, Some(2.0)).unwrap();
        let updated = column.get(0).unwrap();
        assert!(
            (updated - 2.0).abs() < 1e-9,
            "Expected 2.0, got {}",
            updated
        );

        column.set(1, None).unwrap();
        assert!(column.get(1).is_none());
    }

    #[test]
    fn test_alp_column_values() {
        let values = vec![Some(Value::Double(1.5)), None, Some(Value::Double(3.5))];

        let column = AlpColumn::analyze_values(&values, DataType::Double).unwrap();

        assert_eq!(column.get_value(0), Some(Value::Double(1.5)));
        assert!(column.get(1).is_none());
        assert_eq!(column.get_value(2), Some(Value::Double(3.5)));
    }

    #[test]
    fn test_select_alp() {
        let integers: Vec<f64> = (0..1000).map(|i| i as f64).collect();
        assert!(!select_alp(&integers));

        let decimals: Vec<f64> = (0..1000).map(|i| i as f64 * 0.01).collect();
        assert!(select_alp(&decimals));
    }

    #[test]
    fn test_alp_roundtrip_precision() {
        let values = vec![1.234567, 2.345678, 3.456789, 4.567890, 5.678901];
        let encoder = AlpEncoder::analyze(&values);

        for &v in &values {
            let compressed = encoder.compress(v);
            let decompressed = encoder.decompress(compressed);
            assert!(
                (decompressed - v).abs() < 1e-6,
                "Roundtrip failed: {} -> {} -> {}",
                v,
                compressed,
                decompressed
            );
        }
    }

    #[test]
    fn test_alp_negative_values() {
        let values = vec![-1.5, -2.5, 0.0, 1.5, 2.5];
        let encoder = AlpEncoder::analyze(&values);

        for &v in &values {
            let compressed = encoder.compress(v);
            let decompressed = encoder.decompress(compressed);
            assert!((decompressed - v).abs() < 1e-9);
        }
    }
}
