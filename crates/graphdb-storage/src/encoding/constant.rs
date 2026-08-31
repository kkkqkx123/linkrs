//! Constant Encoding
//!
//! Compresses columns where all values are identical by storing a single
//! value plus row count. Reduces storage from O(n) to O(1).

use std::collections::HashMap;
use std::io::{Read, Write};

use graphdb_core::{StorageError, StorageResult, Value};

const MAX_CONSTANT_OVERRIDES: usize = 128;

#[derive(Debug, Clone)]
pub struct ConstantColumn {
    value: Option<Value>,
    count: usize,
    overrides: HashMap<usize, Option<Value>>,
}

impl ConstantColumn {
    pub fn new(value: Option<Value>, count: usize) -> Self {
        Self {
            value,
            count,
            overrides: HashMap::new(),
        }
    }

    #[allow(unused)]
    pub fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }

    #[allow(unused)]
    pub fn overrides_len(&self) -> usize {
        self.overrides.len()
    }

    pub fn should_use(values: &[Option<Value>]) -> bool {
        if values.is_empty() {
            return false;
        }
        let first = &values[0];
        values.iter().all(|v| v == first)
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        if row_idx >= self.count {
            return None;
        }
        if let Some(v) = self.overrides.get(&row_idx) {
            return v.clone();
        }
        self.value.clone()
    }

    pub fn len(&self) -> usize {
        self.count
    }

    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn memory_usage(&self) -> usize {
        let payload = match &self.value {
            Some(Value::String(s)) => s.len(),
            Some(v) => std::mem::size_of_val(v),
            None => 0,
        };
        let overrides_bytes: usize = self
            .overrides
            .iter()
            .map(|(k, v)| {
                std::mem::size_of::<usize>()
                    + std::mem::size_of::<Option<Value>>()
                    + *k % 8
                    + v.as_ref()
                        .map(|vv| match vv {
                            Value::String(s) => s.len(),
                            _ => std::mem::size_of_val(vv),
                        })
                        .unwrap_or(0)
            })
            .sum();
        payload + std::mem::size_of::<Self>() + overrides_bytes
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        let incoming = value.cloned();
        if row_idx == self.count {
            if incoming == self.value {
                self.count += 1;
                Ok(())
            } else {
                // Differing append: store as override instead of full
                // decode. This eliminates the O(n) batch-decode cliff for
                // the common "mostly constant but occasional different"
                // workload. Only when overrides grow beyond the threshold
                // do we fall back to raw (return Err).
                self.count += 1;
                let idx = self.count - 1;
                self.overrides.insert(idx, incoming.clone());
                if self.overrides.len() > MAX_CONSTANT_OVERRIDES
                    && self.overrides.len() * 4 > self.count
                {
                    return Err(StorageError::invalid_operation(format!(
                        "ConstantColumn at append {} exceeded override threshold ({} overrides, {} rows)",
                        row_idx,
                        self.overrides.len(),
                        self.count
                    )));
                }
                Ok(())
            }
        } else if row_idx < self.count {
            if incoming == self.value {
                // Reverting an overridden row back to base value.
                self.overrides.remove(&row_idx);
                Ok(())
            } else {
                // Store differing value as override.
                let is_new = !self.overrides.contains_key(&row_idx);
                self.overrides.insert(row_idx, incoming.clone());
                if is_new
                    && self.overrides.len() > MAX_CONSTANT_OVERRIDES
                    && self.overrides.len() * 4 > self.count
                {
                    return Err(StorageError::invalid_operation(format!(
                        "ConstantColumn at {} exceeded override threshold ({} overrides, {} rows)",
                        row_idx,
                        self.overrides.len(),
                        self.count
                    )));
                }
                Ok(())
            }
        } else {
            Err(StorageError::invalid_input(format!(
                "ConstantColumn set index {} out of bounds (len {})",
                row_idx, self.count
            )))
        }
    }

    #[allow(unused)]
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
        // Overrides for "mostly constant" workload: store differing rows
        // as a sparse map instead of falling back to raw immediately.
        writer
            .write_all(&(self.overrides.len() as u32).to_le_bytes())
            .map_err(|e| StorageError::io_error(e.to_string()))?;
        written += 4;
        for (idx, val) in &self.overrides {
            writer
                .write_all(&(*idx as u32).to_le_bytes())
                .map_err(|e| StorageError::io_error(e.to_string()))?;
            written += 4;
            match val {
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
                        StorageError::serialize_error(format!(
                            "ConstantColumn serialize override value: {}",
                            e
                        ))
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
        // Try to read overrides; old files without this section will hit EOF,
        // in which case we treat overrides as empty for backward compatibility.
        let mut overrides = HashMap::new();
        let mut ov_count_bytes = [0u8; 4];
        match reader.read_exact(&mut ov_count_bytes) {
            Ok(()) => {
                let ov_count = u32::from_le_bytes(ov_count_bytes) as usize;
                for _ in 0..ov_count {
                    let mut idx_bytes = [0u8; 4];
                    reader
                        .read_exact(&mut idx_bytes)
                        .map_err(|e| StorageError::io_error(e.to_string()))?;
                    let idx = u32::from_le_bytes(idx_bytes) as usize;
                    let mut has_ov = [0u8; 1];
                    reader
                        .read_exact(&mut has_ov)
                        .map_err(|e| StorageError::io_error(e.to_string()))?;
                    let val = if has_ov[0] == 0 {
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
                            StorageError::deserialize_error(format!(
                                "ConstantColumn deserialize override value: {}",
                                e
                            ))
                        })?;
                        Some(v)
                    };
                    overrides.insert(idx, val);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Old file without overrides section.
            }
            Err(e) => return Err(StorageError::io_error(e.to_string())),
        }
        Ok(Self {
            value,
            count,
            overrides,
        })
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
        // Differing value is now stored as sparse override, not an error,
        // avoiding the O(n) batch-decode cliff.
        assert!(col.set(0, Some(&Value::Int(2))).is_ok());
        assert_eq!(col.get(0), Some(Value::Int(2)));
        assert_eq!(col.get(1), Some(Value::Int(1)));
        // Reverting override back to base value.
        assert!(col.set(0, Some(&Value::Int(1))).is_ok());
        assert_eq!(col.get(0), Some(Value::Int(1)));
        assert!(col.overrides.is_empty());

        assert!(col.set(2, Some(&Value::Int(1))).is_ok());
        assert_eq!(col.len(), 3);
        assert!(col.set(3, Some(&Value::Int(2))).is_ok());
        assert_eq!(col.get(3), Some(Value::Int(2)));
        assert_eq!(col.overrides_len(), 1);
    }

    #[test]
    fn test_constant_overrides_threshold() {
        let mut col = ConstantColumn::new(Some(Value::Int(1)), 0);
        // Fill with constant value
        for _ in 0..200 {
            col.append(Some(&Value::Int(1))).unwrap();
        }
        // Introduce many distinct overrides beyond threshold: should
        // eventually trigger fallback to raw (Err) when the sparse
        // representation becomes inefficient.
        let mut err = None;
        for i in 0..200 {
            let v = Value::Int(i as i32 + 1000);
            // Alternate between append (new row) and update of existing.
            let res = if i % 2 == 0 {
                col.set(col.len(), Some(&v))
            } else {
                col.set(i % col.len(), Some(&v))
            };
            if res.is_err() {
                err = Some(i);
                break;
            }
        }
        // Threshold logic: with 200 rows, MAX_CONSTANT_OVERRIDES=128 and
        // 25% rule, error should occur only after >64 overrides in a
        // sufficiently small column. Here we expect an error after enough
        // divergent values.
        assert!(err.is_some(), "expected override threshold to trigger");
    }

    #[test]
    fn test_constant_overrides_serialize() {
        let mut col = ConstantColumn::new(Some(Value::Int(1)), 3);
        col.set(1, Some(&Value::Int(2))).unwrap();
        col.set(3, Some(&Value::Int(3))).unwrap();
        let mut buf = Vec::new();
        col.serialize_meta(&mut buf).unwrap();
        let restored = ConstantColumn::deserialize_meta(&mut &buf[..]).unwrap();
        assert_eq!(restored.len(), 4);
        assert_eq!(restored.get(0), Some(Value::Int(1)));
        assert_eq!(restored.get(1), Some(Value::Int(2)));
        assert_eq!(restored.get(2), Some(Value::Int(1)));
        assert_eq!(restored.get(3), Some(Value::Int(3)));
        // Old format without overrides section should still load.
        let mut old_buf = Vec::new();
        // Manually write old format without overrides tail
        old_buf.extend_from_slice(&(5u32.to_le_bytes()));
        old_buf.push(1);
        let bytes = postcard::to_allocvec(&Value::Int(7)).unwrap();
        old_buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        old_buf.extend_from_slice(&bytes);
        // No overrides count appended
        let restored_old = ConstantColumn::deserialize_meta(&mut &old_buf[..]).unwrap();
        assert_eq!(restored_old.len(), 5);
        assert_eq!(restored_old.get(0), Some(Value::Int(7)));
        assert!(restored_old.overrides.is_empty());
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
