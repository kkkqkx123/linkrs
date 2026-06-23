//! Dictionary Encoding
//!
//! Compresses low-cardinality string columns by storing unique values
//! in a dictionary and using indices to reference them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::{StorageError, StorageResult, Value};
use crate::utils::NullBitmap;

#[derive(Debug, Clone)]
pub struct StringDictionary {
    values: Vec<Arc<str>>,
    index_map: HashMap<Arc<str>, u32>,
}

impl StringDictionary {
    pub fn new() -> Self {
        Self {
            values: Vec::new(),
            index_map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, value: &str) -> u32 {
        if let Some(&idx) = self.index_map.get(value) {
            return idx;
        }

        let idx = self.values.len() as u32;
        let arc_value: Arc<str> = Arc::from(value);
        self.index_map.insert(arc_value.clone(), idx);
        self.values.push(arc_value);
        idx
    }

    pub fn get(&self, index: u32) -> Option<&str> {
        self.values.get(index as usize).map(|s| s.as_ref())
    }

    pub fn memory_usage(&self) -> usize {
        let values_size: usize = self.values.iter().map(|s| s.len()).sum();
        let overhead = self.values.len() * std::mem::size_of::<Arc<str>>();
        let map_overhead =
            self.index_map.len() * (std::mem::size_of::<Arc<str>>() + std::mem::size_of::<u32>());
        values_size + overhead + map_overhead
    }
}

impl Default for StringDictionary {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct DictionaryEncoder {
    dictionary: StringDictionary,
    indices: Vec<u32>,
    null_bitmap: NullBitmap,
}

impl DictionaryEncoder {
    pub fn new() -> Self {
        Self {
            dictionary: StringDictionary::new(),
            indices: Vec::new(),
            null_bitmap: NullBitmap::new(),
        }
    }

    pub fn encode(&mut self, value: Option<&str>) {
        match value {
            Some(s) => {
                let idx = self.dictionary.insert(s);
                self.indices.push(idx);
                self.null_bitmap.push(false);
            }
            None => {
                self.indices.push(0);
                self.null_bitmap.push(true);
            }
        }
    }

    pub fn decode(&self, row_idx: usize) -> Option<&str> {
        if row_idx >= self.indices.len() {
            return None;
        }
        if row_idx < self.null_bitmap.len() && self.null_bitmap.is_null(row_idx) {
            return None;
        }
        self.dictionary.get(self.indices[row_idx])
    }

    pub fn len(&self) -> usize {
        self.indices.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.dictionary.memory_usage()
            + self.indices.len() * std::mem::size_of::<u32>()
            + self.null_bitmap.memory_usage()
    }
}

impl Default for DictionaryEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct DictionaryColumn {
    encoder: DictionaryEncoder,
}

impl DictionaryColumn {
    pub fn new() -> Self {
        Self {
            encoder: DictionaryEncoder::new(),
        }
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> StorageResult<()> {
        while self.encoder.len() <= row_idx {
            self.encoder.encode(None);
        }

        match value {
            Some(Value::String(s)) => {
                let idx = self.encoder.dictionary.insert(s);
                if row_idx < self.encoder.indices.len() {
                    self.encoder.indices[row_idx] = idx;
                    self.encoder.null_bitmap.set(row_idx, false);
                } else {
                    self.encoder.indices.push(idx);
                    self.encoder.null_bitmap.push(false);
                }
            }
            Some(v) => {
                return Err(StorageError::type_mismatch(
                    crate::core::DataType::String,
                    v.data_type(),
                ));
            }
            None => {
                if row_idx < self.encoder.indices.len() {
                    self.encoder.indices[row_idx] = 0;
                    self.encoder.null_bitmap.set(row_idx, true);
                } else {
                    self.encoder.indices.push(0);
                    self.encoder.null_bitmap.push(true);
                }
            }
        }

        Ok(())
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        self.encoder
            .decode(row_idx)
            .map(|s| Value::String(s.to_string()))
    }

    pub fn len(&self) -> usize {
        self.encoder.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.encoder.memory_usage()
    }
}

impl Default for DictionaryColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictionary_basic() {
        let mut dict = StringDictionary::new();

        let idx1 = dict.insert("apple");
        let idx2 = dict.insert("banana");
        let idx3 = dict.insert("apple");

        assert_eq!(idx1, 0);
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 0);

        assert_eq!(dict.get(0), Some("apple"));
        assert_eq!(dict.get(1), Some("banana"));
    }

    #[test]
    fn test_encoder_basic() {
        let mut encoder = DictionaryEncoder::new();

        encoder.encode(Some("hello"));
        encoder.encode(Some("world"));
        encoder.encode(None);
        encoder.encode(Some("hello"));

        assert_eq!(encoder.decode(0), Some("hello"));
        assert_eq!(encoder.decode(1), Some("world"));
        assert_eq!(encoder.decode(2), None);
        assert_eq!(encoder.decode(3), Some("hello"));
    }

    #[test]
    fn test_dictionary_column() {
        let mut col = DictionaryColumn::new();

        col.set(0, Some(&Value::String("a".to_string()))).unwrap();
        col.set(1, Some(&Value::String("b".to_string()))).unwrap();
        col.set(2, None).unwrap();
        col.set(3, Some(&Value::String("a".to_string()))).unwrap();

        assert_eq!(col.get(0), Some(Value::String("a".to_string())));
        assert_eq!(col.get(1), Some(Value::String("b".to_string())));
        assert!(col.get(2).is_none());
        assert_eq!(col.get(3), Some(Value::String("a".to_string())));
    }
}
