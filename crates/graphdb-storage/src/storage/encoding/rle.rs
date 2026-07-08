//! Run-Length Encoding (RLE)
//!
//! Compresses columns with repeated values by storing (value, count) pairs.

use crate::core::{DataType, StorageError, StorageResult, Value};
use crate::utils::NullBitmap;

#[derive(Debug, Clone, PartialEq)]
pub struct RleRun<T> {
    pub value: T,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct RleEncoder<T> {
    runs: Vec<RleRun<T>>,
    total_count: usize,
}

impl<T: Clone + PartialEq> RleEncoder<T> {
    pub fn new() -> Self {
        Self {
            runs: Vec::new(),
            total_count: 0,
        }
    }

    pub fn encode(&mut self, value: T) {
        self.total_count += 1;

        if let Some(last_run) = self.runs.last_mut() {
            if last_run.value == value {
                last_run.count += 1;
                return;
            }
        }

        self.runs.push(RleRun { value, count: 1 });
    }

    pub fn decode(&self, index: usize) -> Option<&T> {
        if index >= self.total_count {
            return None;
        }

        let mut current_idx = 0;
        for run in &self.runs {
            if index < current_idx + run.count {
                return Some(&run.value);
            }
            current_idx += run.count;
        }

        None
    }

    pub fn len(&self) -> usize {
        self.total_count
    }

    pub fn memory_usage(&self) -> usize {
        self.runs.len() * (std::mem::size_of::<RleRun<T>>())
    }
}

impl<T: Clone + PartialEq + Default> Default for RleEncoder<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RleIntColumn {
    encoder: RleEncoder<i64>,
    null_bitmap: NullBitmap,
}

impl RleIntColumn {
    pub fn new() -> Self {
        Self {
            encoder: RleEncoder::new(),
            null_bitmap: NullBitmap::new(),
        }
    }

    pub fn append(&mut self, value: Option<&Value>) -> StorageResult<()> {
        match value {
            Some(Value::SmallInt(v)) => {
                self.null_bitmap.push(false);
                self.encoder.encode(*v as i64);
            }
            Some(Value::Int(v)) => {
                self.null_bitmap.push(false);
                self.encoder.encode(*v as i64);
            }
            Some(Value::BigInt(v)) => {
                self.null_bitmap.push(false);
                self.encoder.encode(*v);
            }
            Some(v) => {
                return Err(StorageError::type_mismatch(DataType::BigInt, v.data_type()));
            }
            None => {
                self.null_bitmap.push(true);
                self.encoder.encode(0);
            }
        }

        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        if row_idx >= self.encoder.len() {
            return None;
        }
        if self.null_bitmap.is_null(row_idx) {
            return None;
        }
        self.encoder.decode(row_idx).map(|&v| Value::BigInt(v))
    }

    pub fn len(&self) -> usize {
        self.encoder.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.encoder.memory_usage() + self.null_bitmap.memory_usage()
    }
}

impl Default for RleIntColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RleBoolColumn {
    encoder: RleEncoder<bool>,
    null_bitmap: NullBitmap,
}

impl RleBoolColumn {
    pub fn new() -> Self {
        Self {
            encoder: RleEncoder::new(),
            null_bitmap: NullBitmap::new(),
        }
    }

    pub fn append(&mut self, value: Option<&Value>) -> StorageResult<()> {
        match value {
            Some(Value::Bool(v)) => {
                self.null_bitmap.push(false);
                self.encoder.encode(*v);
            }
            Some(v) => {
                return Err(StorageError::type_mismatch(DataType::Bool, v.data_type()));
            }
            None => {
                self.null_bitmap.push(true);
                self.encoder.encode(false);
            }
        }

        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        if row_idx >= self.encoder.len() {
            return None;
        }
        if self.null_bitmap.is_null(row_idx) {
            return None;
        }
        self.encoder.decode(row_idx).map(|&v| Value::Bool(v))
    }

    pub fn len(&self) -> usize {
        self.encoder.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.encoder.memory_usage() + self.null_bitmap.memory_usage()
    }
}

impl Default for RleBoolColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rle_encoder_basic() {
        let mut encoder = RleEncoder::<i64>::new();

        encoder.encode(1);
        encoder.encode(1);
        encoder.encode(1);
        encoder.encode(2);
        encoder.encode(2);
        encoder.encode(3);

        assert_eq!(encoder.len(), 6);
        assert_eq!(encoder.runs.len(), 3);

        assert_eq!(encoder.decode(0), Some(&1));
        assert_eq!(encoder.decode(2), Some(&1));
        assert_eq!(encoder.decode(3), Some(&2));
        assert_eq!(encoder.decode(5), Some(&3));
    }

    #[test]
    fn test_rle_int_column() {
        let mut col = RleIntColumn::new();

        col.append(Some(&Value::Int(1))).unwrap();
        col.append(Some(&Value::Int(1))).unwrap();
        col.append(Some(&Value::Int(1))).unwrap();
        col.append(Some(&Value::Int(2))).unwrap();
        col.append(None).unwrap();

        assert_eq!(col.get(0), Some(Value::BigInt(1)));
        assert_eq!(col.get(3), Some(Value::BigInt(2)));
        assert!(col.null_bitmap.is_null(4));
        assert_eq!(col.encoder.runs.len(), 3);
    }

    #[test]
    fn test_rle_bool_column() {
        let mut col = RleBoolColumn::new();

        col.append(Some(&Value::Bool(true))).unwrap();
        col.append(Some(&Value::Bool(true))).unwrap();
        col.append(Some(&Value::Bool(false))).unwrap();
        col.append(None).unwrap();

        assert_eq!(col.get(0), Some(Value::Bool(true)));
        assert_eq!(col.get(2), Some(Value::Bool(false)));
        assert!(col.null_bitmap.is_null(3));
        assert_eq!(col.encoder.runs.len(), 2);
    }
}
