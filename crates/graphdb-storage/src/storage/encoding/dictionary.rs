//! Dictionary Encoding
//!
//! Compresses low-cardinality string columns by storing unique values
//! in a dictionary and using indices to reference them.

use std::collections::HashMap;
use std::io::{Read, Write};
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

    pub fn sorted_entries(&self) -> Vec<(u32, &str)> {
        let mut entries: Vec<(u32, &str)> = self
            .values
            .iter()
            .enumerate()
            .map(|(idx, s)| (idx as u32, s.as_ref()))
            .collect();
        entries.sort_by(|a, b| a.1.cmp(b.1));
        entries
    }

    pub fn memory_usage(&self) -> usize {
        let values_size: usize = self.values.iter().map(|s| s.len()).sum();
        let overhead = self.values.len() * std::mem::size_of::<Arc<str>>();
        let map_overhead =
            self.index_map.len() * (std::mem::size_of::<Arc<str>>() + std::mem::size_of::<u32>());
        values_size + overhead + map_overhead
    }

    pub fn serialize(&self, writer: &mut impl Write) -> std::io::Result<usize> {
        let mut written = 0usize;
        let entries = self.sorted_entries();
        let count = entries.len() as u32;
        writer.write_all(&count.to_le_bytes())?;
        written += 4;
        for (_, value) in entries {
            let len = value.len() as u32;
            writer.write_all(&len.to_le_bytes())?;
            writer.write_all(value.as_bytes())?;
            written += 4 + value.len();
        }
        Ok(written)
    }

    fn sorted_with_remap(&self) -> (Self, Vec<u32>) {
        let entries = self.sorted_entries();
        let mut sorted = Self::new();
        let mut remap = vec![0u32; self.values.len()];
        for (new_index, (old_index, value)) in entries.into_iter().enumerate() {
            let inserted = sorted.insert(value);
            debug_assert_eq!(inserted, new_index as u32);
            remap[old_index as usize] = inserted;
        }
        (sorted, remap)
    }

    pub fn deserialize(reader: &mut impl Read) -> std::io::Result<Self> {
        let mut count_bytes = [0u8; 4];
        reader.read_exact(&mut count_bytes)?;
        let count = u32::from_le_bytes(count_bytes) as usize;
        let mut dict = Self::new();
        for _ in 0..count {
            let mut len_bytes = [0u8; 4];
            reader.read_exact(&mut len_bytes)?;
            let len = u32::from_le_bytes(len_bytes) as usize;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            let value = String::from_utf8(buf)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            dict.insert(&value);
        }
        Ok(dict)
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
            .map(|s| Value::string(s))
    }

    pub fn len(&self) -> usize {
        self.encoder.len()
    }

    pub fn memory_usage(&self) -> usize {
        self.encoder.memory_usage()
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        let (sorted_dictionary, remap) = self.encoder.dictionary.sorted_with_remap();
        written += sorted_dictionary.serialize(writer).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn serialize dict: {}", e))
        })?;
        let idx_count = self.encoder.indices.len() as u32;
        writer.write_all(&idx_count.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn serialize idx count: {}", e))
        })?;
        written += 4;
        for &idx in &self.encoder.indices {
            let remapped = remap.get(idx as usize).copied().ok_or_else(|| {
                StorageError::serialize_error(format!(
                    "dictionary index {} is outside dictionary",
                    idx
                ))
            })?;
            writer.write_all(&remapped.to_le_bytes()).map_err(|e| {
                StorageError::io_error(format!("DictionaryColumn serialize idx: {}", e))
            })?;
            written += 4;
        }
        let bm_len = self.encoder.null_bitmap.len() as u32;
        writer.write_all(&bm_len.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn serialize bm_len: {}", e))
        })?;
        written += 4;
        for &word in self.encoder.null_bitmap.as_bits() {
            writer.write_all(&word.to_le_bytes()).map_err(|e| {
                StorageError::io_error(format!("DictionaryColumn serialize bm word: {}", e))
            })?;
            written += 8;
        }
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let dictionary = StringDictionary::deserialize(reader).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn deserialize dict: {}", e))
        })?;
        let mut idx_count_bytes = [0u8; 4];
        reader.read_exact(&mut idx_count_bytes).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn deserialize idx count: {}", e))
        })?;
        let idx_count = u32::from_le_bytes(idx_count_bytes) as usize;
        let mut indices = Vec::with_capacity(idx_count);
        for _ in 0..idx_count {
            let mut idx_bytes = [0u8; 4];
            reader.read_exact(&mut idx_bytes).map_err(|e| {
                StorageError::io_error(format!("DictionaryColumn deserialize idx: {}", e))
            })?;
            indices.push(u32::from_le_bytes(idx_bytes));
        }
        let mut bm_len_bytes = [0u8; 4];
        reader.read_exact(&mut bm_len_bytes).map_err(|e| {
            StorageError::io_error(format!("DictionaryColumn deserialize bm_len: {}", e))
        })?;
        let bm_len = u32::from_le_bytes(bm_len_bytes) as usize;
        let words = bm_len.div_ceil(64);
        let mut data = Vec::with_capacity(words);
        for _ in 0..words {
            let mut word_bytes = [0u8; 8];
            reader.read_exact(&mut word_bytes).map_err(|e| {
                StorageError::io_error(format!("DictionaryColumn deserialize bm word: {}", e))
            })?;
            data.push(u64::from_le_bytes(word_bytes));
        }
        let null_bitmap = NullBitmap::from_raw(data, bm_len);
        Ok(Self {
            encoder: DictionaryEncoder {
                dictionary,
                indices,
                null_bitmap,
            },
        })
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

        col.set(0, Some(&Value::string("a"))).unwrap();
        col.set(1, Some(&Value::string("b"))).unwrap();
        col.set(2, None).unwrap();
        col.set(3, Some(&Value::string("a"))).unwrap();

        assert_eq!(col.get(0), Some(Value::string("a")));
        assert_eq!(col.get(1), Some(Value::string("b")));
        assert!(col.get(2).is_none());
        assert_eq!(col.get(3), Some(Value::string("a")));
    }

    #[test]
    fn test_sorted_entries_deterministic() {
        let mut dict1 = StringDictionary::new();
        dict1.insert("cherry");
        dict1.insert("apple");
        dict1.insert("banana");

        let mut dict2 = StringDictionary::new();
        dict2.insert("banana");
        dict2.insert("cherry");
        dict2.insert("apple");

        let entries1 = dict1.sorted_entries();
        let entries2 = dict2.sorted_entries();

        let strings1: Vec<&str> = entries1.iter().map(|(_, s)| *s).collect();
        let strings2: Vec<&str> = entries2.iter().map(|(_, s)| *s).collect();

        assert_eq!(strings1, vec!["apple", "banana", "cherry"]);
        assert_eq!(strings2, vec!["apple", "banana", "cherry"]);
        assert_eq!(strings1, strings2);
    }
}
