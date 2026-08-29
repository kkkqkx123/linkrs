//! Constant Encoding
//!
//! Compresses columns where all values are identical by storing a single
//! value plus row count. Reduces storage from O(n) to O(1).

use std::io::{Read, Write};

use graphdb_core::{StorageError, StorageResult, Value};

#[derive(Debug, Clone)]
pub struct ConstantColumn {
    value: Option<Value>,
    count: usize,
}

impl ConstantColumn {
    pub fn new(value: Option<Value>, count: usize) -> Self {
        Self { value, count }
    }

    pub fn should_use(values: &[Option<Value>]) -> bool {
        if values.is_empty() {
            return false;
        }
        let first = &values[0];
        values.iter().all(|v| v == first)
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        if row_idx < self.count {
            self.value.clone()
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn memory_usage(&self) -> usize {
        let payload = match &self.value {
            Some(Value::String(s)) => s.len(),
            Some(v) => std::mem::size_of_val(v),
            None => 0,
        };
        payload + std::mem::size_of::<Self>()
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let incoming = value.cloned();
        if row_idx == self.count {
            if incoming == self.value {
                self.count += 1;
                Ok(())
            } else {
                Err(StorageError::invalid_operation(format!(
                    "ConstantColumn set at append index {} with differing value {:?} vs {:?}",
                    row_idx, incoming, self.value
                )))
            }
        } else if row_idx < self.count {
            if incoming == self.value {
                Ok(())
            } else {
                Err(StorageError::invalid_operation(format!(
                    "ConstantColumn set at {} with differing value {:?} vs {:?}",
                    row_idx, incoming, self.value
                )))
            }
        } else {
            Err(StorageError::invalid_input(format!(
                "ConstantColumn set index {} out of bounds (len {})",
                row_idx, self.count
            )))
        }
    }

    pub fn append(&mut self, value: Option<&Value>) -> StorageResult<()> {
        self.set(self.count, value)
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer
            .write_all(&(self.count as u32).to_le_bytes())
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        written += 4;
        match &self.value {
            None => {
                writer
                    .write_all(&[0u8])
                    .map_err(|e| StorageError::io_error(e.to_string()))?;
                written += 1;
            }
            Some(v) => {
                writer
                    .write_all(&[1u8])
                    .map_err(|e| StorageError::io_error(e.to_string()))?;
                written += 1;
                let bytes = postcard::to_allocvec(v).map_err(|e| {
                    StorageError::serialize_error(format!("ConstantColumn serialize value: {}", e))
                })?;
                writer
                    .write_all(&(bytes.len() as u32).to_le_bytes())
                    .map_err(|e| StorageError::io_error(e.to_string()))?;
                writer
                    .write_all(&bytes)
                    .map_err(|e| StorageError::io_error(e.to_string()))?;
                written += 4 + bytes.len();
            }
        }
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut count_bytes = [0u8; 4];
        reader
            .read_exact(&mut count_bytes)
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        let count = u32::from_le_bytes(count_bytes) as usize;
        let mut has_bytes = [0u8; 1];
        reader
            .read_exact(&mut has_bytes)
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        let value = if has_bytes[0] == 0 {
            None
        } else {
            let mut len_bytes = [0u8; 4];
            reader
                .read_exact(&mut len_bytes)
                .map_err(|e| StorageError::io_error(e.to_string()))?;
            let len = u32::from_le_bytes(len_bytes) as usize;
            let mut buf = vec![0u8; len];
            reader
                .read_exact(&mut buf)
                .map_err(|e| StorageError::io_error(e.to_string()))?;
            let v: Value = postcard::from_bytes(&buf).map_err(|e| {
                StorageError::deserialize_error(format!("ConstantColumn deserialize value: {}", e))
            })?;
            Some(v)
        };
        Ok(Self { value, count })
    }
}

impl Default for ConstantColumn {
    fn default() -> Self {
        Self::new(None, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

    #[test]
    fn test_constant_should_use() {
        let values = vec![
            Some(Value::Int(42)),
            Some(Value::Int(42)),
            Some(Value::Int(42)),
        ];
        assert!(ConstantColumn::should_use(&values));
        let values2 = vec![Some(Value::Int(42)), Some(Value::Int(43))];
        assert!(!ConstantColumn::should_use(&values2));
        let empty: Vec<Option<Value>> = vec![];
        assert!(!ConstantColumn::should_use(&empty));
        let nulls = vec![None, None, None];
        assert!(ConstantColumn::should_use(&nulls));
        let mixed = vec![Some(Value::Int(42)), None];
        assert!(!ConstantColumn::should_use(&mixed));
    }

    #[test]
    fn test_constant_get_and_len() {
        let col = ConstantColumn::new(Some(Value::Int(7)), 5);
        assert_eq!(col.len(), 5);
        assert_eq!(col.get(0), Some(Value::Int(7)));
        assert_eq!(col.get(4), Some(Value::Int(7)));
        assert_eq!(col.get(5), None);
    }

    #[test]
    fn test_constant_set_append() {
        let mut col = ConstantColumn::new(Some(Value::Int(1)), 2);
        assert!(col.set(0, Some(&Value::Int(1))).is_ok());
        assert!(col.set(0, Some(&Value::Int(2))).is_err());
        assert!(col.set(2, Some(&Value::Int(1))).is_ok());
        assert_eq!(col.len(), 3);
        assert!(col.set(3, Some(&Value::Int(2))).is_err());
    }

    #[test]
    fn test_constant_serialize_roundtrip() {
        let col = ConstantColumn::new(Some(Value::string("hello")), 100);
        let mut buf = Vec::new();
        col.serialize_meta(&mut buf).unwrap();
        let restored = ConstantColumn::deserialize_meta(&mut &buf[..]).unwrap();
        assert_eq!(restored.len(), 100);
        assert_eq!(restored.get(0), Some(Value::string("hello")));
        assert_eq!(restored.get(99), Some(Value::string("hello")));

        let null_col = ConstantColumn::new(None, 50);
        let mut buf2 = Vec::new();
        null_col.serialize_meta(&mut buf2).unwrap();
        let restored2 = ConstantColumn::deserialize_meta(&mut &buf2[..]).unwrap();
        assert_eq!(restored2.len(), 50);
        assert_eq!(restored2.get(0), None);
    }
}
