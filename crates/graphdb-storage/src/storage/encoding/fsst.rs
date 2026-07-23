//! FSST (Fast Static Symbol Table) String Compression
//!
//! A fast string compression technique using a static symbol table.
//! Effective for long strings and high-cardinality scenarios where
//! dictionary encoding is less effective.
//!
//! # Algorithm
//!
//! 1. Analyze input strings to find frequent byte sequences (2-8 bytes)
//! 2. Build a symbol table mapping frequent sequences to single-byte codes
//! 3. Encode strings using the symbol table
//! 4. Decoding is a simple table lookup - very fast
//!
//! # Performance Optimizations
//!
//! - Training uses sampling to limit memory usage
//! - Encoding uses array-based lookup to avoid heap allocations
//! - Only extracts ngrams of length 2-8 (single bytes don't benefit from encoding)

use std::collections::HashMap;
use std::io::{Read, Write};

use crate::utils::NullBitmap;

const MAX_SYMBOL_LEN: usize = 8;
const MIN_SYMBOL_LEN: usize = 2;
const SYMBOL_TABLE_SIZE: usize = 255;
const MAX_TRAINING_SAMPLES: usize = 10000;
const MAX_NGRAMS_PER_STRING: usize = 1000;
const DEFAULT_REBUILD_THRESHOLD: f64 = 0.2;
#[derive(Debug, Clone)]
pub struct FsstSymbolTable {
    code_to_symbol: Vec<Vec<u8>>,
    byte_to_code: HashMap<Vec<u8>, u8>,
}

impl FsstSymbolTable {
    pub fn new() -> Self {
        Self {
            code_to_symbol: vec![Vec::new(); SYMBOL_TABLE_SIZE + 1],
            byte_to_code: HashMap::new(),
        }
    }

    pub fn insert(&mut self, bytes: Vec<u8>, code: u8) {
        if code == 0 {
            return;
        }
        self.code_to_symbol[code as usize] = bytes.clone();
        self.byte_to_code.insert(bytes, code);
    }

    pub fn get_by_code(&self, code: u8) -> Option<&Vec<u8>> {
        let symbol = &self.code_to_symbol[code as usize];
        if symbol.is_empty() {
            None
        } else {
            Some(symbol)
        }
    }

    pub fn get_by_bytes(&self, bytes: &[u8]) -> Option<u8> {
        self.byte_to_code.get(bytes).copied()
    }

    pub fn len(&self) -> usize {
        self.byte_to_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.byte_to_code.is_empty()
    }

    pub fn memory_usage(&self) -> usize {
        self.code_to_symbol.iter().map(|v| v.len()).sum::<usize>()
            + self.byte_to_code.keys().map(|k| k.len()).sum::<usize>()
            + std::mem::size_of::<Self>()
    }

    pub fn serialize(&self, writer: &mut impl Write) -> std::io::Result<usize> {
        let mut written = 0usize;
        let entries: Vec<(u8, &Vec<u8>)> = (0..=SYMBOL_TABLE_SIZE as u8)
            .filter_map(|code| {
                let sym = &self.code_to_symbol[code as usize];
                if !sym.is_empty() {
                    Some((code, sym))
                } else {
                    None
                }
            })
            .collect();
        let count = entries.len() as u16;
        writer.write_all(&count.to_le_bytes())?;
        written += 2;
        for (code, sym) in entries {
            writer.write_all(&[code])?;
            written += 1;
            let len = sym.len() as u8;
            writer.write_all(&[len])?;
            written += 1;
            writer.write_all(sym)?;
            written += sym.len();
        }
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl Read) -> std::io::Result<Self> {
        let mut count_bytes = [0u8; 2];
        reader.read_exact(&mut count_bytes)?;
        let count = u16::from_le_bytes(count_bytes) as usize;
        let mut table = Self::new();
        for _ in 0..count {
            let mut code_buf = [0u8; 1];
            reader.read_exact(&mut code_buf)?;
            let code = code_buf[0];
            let mut len_buf = [0u8; 1];
            reader.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut sym = vec![0u8; len];
            reader.read_exact(&mut sym)?;
            table.code_to_symbol[code as usize] = sym.clone();
            table.byte_to_code.insert(sym, code);
        }
        Ok(table)
    }
}

impl Default for FsstSymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct FsstEncoder {
    table: FsstSymbolTable,
}

impl FsstEncoder {
    pub fn new() -> Self {
        Self::with_table(FsstSymbolTable::new())
    }

    pub fn train(strings: &[&str], max_symbols: usize) -> Self {
        if strings.is_empty() || max_symbols == 0 {
            return Self::new();
        }
        let mut encoder = Self::new();
        encoder.build_symbol_table(strings, max_symbols);
        encoder
    }

    fn build_symbol_table(&mut self, strings: &[&str], max_symbols: usize) {
        let sampled: Vec<&str> = if strings.len() > MAX_TRAINING_SAMPLES {
            let step = strings.len().div_ceil(MAX_TRAINING_SAMPLES);
            strings.iter().step_by(step).copied().collect()
        } else {
            strings.to_vec()
        };
        let mut ngram_freq: HashMap<Vec<u8>, usize> = HashMap::new();
        for s in sampled {
            let bytes = s.as_bytes();
            if bytes.len() < MIN_SYMBOL_LEN {
                continue;
            }
            let mut ngram_count = 0;
            for len in MIN_SYMBOL_LEN..=MAX_SYMBOL_LEN.min(bytes.len()) {
                for i in 0..=bytes.len() - len {
                    if ngram_count >= MAX_NGRAMS_PER_STRING {
                        break;
                    }
                    *ngram_freq.entry(bytes[i..i + len].to_vec()).or_insert(0) += 1;
                    ngram_count += 1;
                }
                if ngram_count >= MAX_NGRAMS_PER_STRING {
                    break;
                }
            }
        }
        let mut ngrams: Vec<(Vec<u8>, usize)> = ngram_freq.into_iter().collect();
        ngrams.sort_by(|a, b| {
            let score_a = a.1 * a.0.len();
            let score_b = b.1 * b.0.len();
            score_b.cmp(&score_a).then_with(|| a.0.cmp(&b.0))
        });
        let max_symbols = max_symbols.min(SYMBOL_TABLE_SIZE);
        for (idx, (ngram, _)) in ngrams.into_iter().enumerate() {
            if idx >= max_symbols.saturating_sub(1) {
                break;
            }
            self.table.insert(ngram, (idx + 1) as u8);
        }
    }

    pub fn encode(&self, s: &str) -> Vec<u8> {
        self.encode_bytes(s.as_bytes())
    }

    pub fn encode_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        if bytes.is_empty() {
            return Vec::new();
        }
        if self.symbol_count() == 0 {
            return bytes.to_vec();
        }

        let mut result = Vec::with_capacity(bytes.len());
        let mut i = 0;

        while i < bytes.len() {
            let remaining = bytes.len() - i;
            let max_len = MAX_SYMBOL_LEN.min(remaining);

            let mut found = false;
            for len in (MIN_SYMBOL_LEN..=max_len).rev() {
                if let Some(code) = self.table.get_by_bytes(&bytes[i..i + len]) {
                    result.push(code);
                    i += len;
                    found = true;
                    break;
                }
            }

            if !found {
                result.push(bytes[i]);
                i += 1;
            }
        }

        result
    }

    pub fn decode(&self, encoded: &[u8]) -> Vec<u8> {
        if self.table.is_empty() {
            return encoded.to_vec();
        }

        let mut result = Vec::with_capacity(encoded.len() * 2);

        for &code in encoded {
            if let Some(symbol) = self.table.get_by_code(code) {
                result.extend_from_slice(symbol);
            } else {
                result.push(code);
            }
        }

        result
    }

    pub fn decode_to_string(&self, encoded: &[u8]) -> Option<String> {
        let bytes = self.decode(encoded);
        String::from_utf8(bytes).ok()
    }

    pub fn table(&self) -> &FsstSymbolTable {
        &self.table
    }

    pub fn symbol_count(&self) -> usize {
        self.table.len()
    }

    pub fn with_table(table: FsstSymbolTable) -> Self {
        Self { table }
    }

    pub fn serialize(&self, writer: &mut impl Write) -> std::io::Result<usize> {
        self.table.serialize(writer)
    }

    pub fn deserialize(reader: &mut impl Read) -> std::io::Result<Self> {
        let table = FsstSymbolTable::deserialize(reader)?;
        Ok(Self { table })
    }
}

impl Default for FsstEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct FsstColumn {
    pub encoder: FsstEncoder,
    pub encoded_data: Vec<Vec<u8>>,
    pub null_bitmap: NullBitmap,
    pub(crate) updates_since_rebuild: usize,
}

impl FsstColumn {
    pub fn new() -> Self {
        Self {
            encoder: FsstEncoder::new(),
            encoded_data: Vec::new(),
            null_bitmap: NullBitmap::new(),
            updates_since_rebuild: 0,
        }
    }

    pub fn get(&self, row_idx: usize) -> Option<String> {
        self.get_bytes(row_idx)
            .and_then(|b| String::from_utf8(b).ok())
    }

    pub fn get_bytes(&self, row_idx: usize) -> Option<Vec<u8>> {
        if row_idx >= self.encoded_data.len() || self.null_bitmap.is_null(row_idx) {
            return None;
        }
        Some(self.encoder.decode(&self.encoded_data[row_idx]))
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&str>) -> crate::core::StorageResult<()> {
        self.set_bytes(row_idx, value.map(|s| s.as_bytes()))
    }

    pub fn set_bytes(
        &mut self,
        row_idx: usize,
        value: Option<&[u8]>,
    ) -> crate::core::StorageResult<()> {
        if row_idx >= self.encoded_data.len() {
            return Err(crate::core::StorageError::invalid_offset(row_idx as u32));
        }

        match value {
            Some(bytes) => {
                self.encoded_data[row_idx] = self.encoder.encode_bytes(bytes);
                self.null_bitmap.set(row_idx, false);
            }
            None => {
                self.encoded_data[row_idx].clear();
                self.null_bitmap.set(row_idx, true);
            }
        }
        self.updates_since_rebuild = self.updates_since_rebuild.saturating_add(1);
        self.rebuild_if_needed(DEFAULT_REBUILD_THRESHOLD)?;
        Ok(())
    }

    pub fn append(&mut self, value: Option<&str>) -> crate::core::StorageResult<()> {
        self.append_bytes(value.map(|s| s.as_bytes()))
    }

    pub fn append_bytes(&mut self, value: Option<&[u8]>) -> crate::core::StorageResult<()> {
        match value {
            Some(bytes) => {
                self.encoded_data.push(self.encoder.encode_bytes(bytes));
                self.null_bitmap.push(false);
            }
            None => {
                self.encoded_data.push(Vec::new());
                self.null_bitmap.push(true);
            }
        }
        self.updates_since_rebuild = self.updates_since_rebuild.saturating_add(1);
        self.rebuild_if_needed(DEFAULT_REBUILD_THRESHOLD)
    }

    pub fn len(&self) -> usize {
        self.encoded_data.len()
    }

    pub fn rebuild(&mut self, new_strings: &[String]) -> crate::core::StorageResult<()> {
        let new_bytes: Vec<&[u8]> = new_strings.iter().map(|s| s.as_bytes()).collect();
        self.rebuild_bytes(&new_bytes)
    }

    pub fn rebuild_bytes(&mut self, new_bytes: &[&[u8]]) -> crate::core::StorageResult<()> {
        let mut existing: Vec<(usize, Vec<u8>)> = Vec::with_capacity(self.encoded_data.len());
        for (idx, encoded) in self.encoded_data.iter().enumerate() {
            if self.null_bitmap.is_null(idx) {
                continue;
            }
            existing.push((idx, self.encoder.decode(encoded)));
        }

        let mut training: Vec<&[u8]> = existing.iter().map(|(_, v)| v.as_slice()).collect();
        training.extend_from_slice(new_bytes);
        if training.is_empty() {
            self.updates_since_rebuild = 0;
            return Ok(());
        }

        let refs: Vec<&str> = training
            .iter()
            .map(|b| std::str::from_utf8(b).unwrap_or(""))
            .collect();
        let new_encoder = FsstEncoder::train(&refs, SYMBOL_TABLE_SIZE);
        for (idx, value) in existing {
            self.encoded_data[idx] = new_encoder.encode_bytes(&value);
        }
        self.encoder = new_encoder;
        self.updates_since_rebuild = 0;
        Ok(())
    }

    pub fn rebuild_if_needed(&mut self, threshold: f64) -> crate::core::StorageResult<()> {
        let threshold = threshold.clamp(0.0, 1.0);
        let existing_rows = self
            .encoded_data
            .len()
            .saturating_sub(self.updates_since_rebuild);
        let required_updates = ((existing_rows as f64) * threshold).ceil() as usize;
        if self.updates_since_rebuild > 0 && self.updates_since_rebuild >= required_updates.max(1) {
            self.rebuild(&[])?;
        }
        Ok(())
    }

    pub fn memory_usage(&self) -> usize {
        let data_size: usize = self.encoded_data.iter().map(|v| v.len()).sum();
        let null_size = self.null_bitmap.memory_usage();
        let table_size = self.encoder.table().memory_usage();

        data_size + null_size + table_size
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> crate::core::StorageResult<usize> {
        let mut written = 0usize;
        written += self.encoder.serialize(writer).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn serialize encoder: {}", e))
        })?;
        let data_count = self.encoded_data.len() as u32;
        writer.write_all(&data_count.to_le_bytes()).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn serialize count: {}", e))
        })?;
        written += 4;
        for item in &self.encoded_data {
            let len = u32::try_from(item.len()).map_err(|_| {
                crate::core::StorageError::serialize_error(
                    "FSST encoded item exceeds u32 length".to_string(),
                )
            })?;
            writer.write_all(&len.to_le_bytes()).map_err(|e| {
                crate::core::StorageError::io_error(format!("FsstColumn serialize item len: {}", e))
            })?;
            writer.write_all(item).map_err(|e| {
                crate::core::StorageError::io_error(format!("FsstColumn serialize item: {}", e))
            })?;
            written += 4 + item.len();
        }
        let bm_len = self.null_bitmap.len() as u32;
        writer.write_all(&bm_len.to_le_bytes()).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn serialize bm_len: {}", e))
        })?;
        written += 4;
        for &word in self.null_bitmap.as_bits() {
            writer.write_all(&word.to_le_bytes()).map_err(|e| {
                crate::core::StorageError::io_error(format!("FsstColumn serialize bm word: {}", e))
            })?;
            written += 8;
        }
        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> crate::core::StorageResult<Self> {
        let encoder = FsstEncoder::deserialize(reader).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn deserialize encoder: {}", e))
        })?;
        let mut count_bytes = [0u8; 4];
        reader.read_exact(&mut count_bytes).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn deserialize count: {}", e))
        })?;
        let data_count = u32::from_le_bytes(count_bytes) as usize;
        let mut encoded_data = Vec::with_capacity(data_count);
        for _ in 0..data_count {
            let mut len_bytes = [0u8; 4];
            reader.read_exact(&mut len_bytes).map_err(|e| {
                crate::core::StorageError::io_error(format!(
                    "FsstColumn deserialize item len: {}",
                    e
                ))
            })?;
            let len = u32::from_le_bytes(len_bytes) as usize;
            let mut item = vec![0u8; len];
            reader.read_exact(&mut item).map_err(|e| {
                crate::core::StorageError::io_error(format!("FsstColumn deserialize item: {}", e))
            })?;
            encoded_data.push(item);
        }
        let mut bm_len_bytes = [0u8; 4];
        reader.read_exact(&mut bm_len_bytes).map_err(|e| {
            crate::core::StorageError::io_error(format!("FsstColumn deserialize bm_len: {}", e))
        })?;
        let bm_len = u32::from_le_bytes(bm_len_bytes) as usize;
        let words = bm_len.div_ceil(64);
        let mut data = Vec::with_capacity(words);
        for _ in 0..words {
            let mut word_bytes = [0u8; 8];
            reader.read_exact(&mut word_bytes).map_err(|e| {
                crate::core::StorageError::io_error(format!(
                    "FsstColumn deserialize bm word: {}",
                    e
                ))
            })?;
            data.push(u64::from_le_bytes(word_bytes));
        }
        let null_bitmap = NullBitmap::from_raw(data, bm_len);
        Ok(Self {
            encoder,
            encoded_data,
            null_bitmap,
            updates_since_rebuild: 0,
        })
    }
}

impl Default for FsstColumn {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::NullBitmap;

    fn build_fsst_column(strings: &[Option<&str>], max_symbols: usize) -> FsstColumn {
        let non_null: Vec<&str> = strings.iter().filter_map(|s| *s).collect();
        let encoder = if non_null.is_empty() {
            FsstEncoder::new()
        } else {
            FsstEncoder::train(&non_null, max_symbols)
        };

        let mut column = FsstColumn {
            encoder,
            encoded_data: Vec::with_capacity(strings.len()),
            null_bitmap: NullBitmap::with_capacity(strings.len()),
            updates_since_rebuild: 0,
        };

        for value in strings {
            match value {
                Some(s) => {
                    column.encoded_data.push(column.encoder.encode(s));
                    column.null_bitmap.push(false);
                }
                None => {
                    column.encoded_data.push(Vec::new());
                    column.null_bitmap.push(true);
                }
            }
        }

        column
    }

    fn select_fsst(values: &[&str]) -> bool {
        values.len() >= 64
            && values.iter().map(|value| value.len()).sum::<usize>() / values.len() >= 16
    }

    #[test]
    fn test_fsst_symbol_table() {
        let mut table = FsstSymbolTable::new();

        table.insert(b"hello".to_vec(), 1);
        table.insert(b"world".to_vec(), 2);

        assert_eq!(table.len(), 2);
        assert_eq!(table.get_by_code(1), Some(&b"hello".to_vec()));
        assert_eq!(table.get_by_bytes(b"world"), Some(2));
    }

    #[test]
    fn test_fsst_encoder_basic() {
        let strings = vec!["hello world", "hello rust", "hello code"];
        let encoder = FsstEncoder::train(&strings, 100);

        let encoded = encoder.encode("hello world");
        let decoded = encoder.decode_to_string(&encoded);

        assert_eq!(decoded, Some("hello world".to_string()));
    }

    #[test]
    fn test_fsst_encoder_compression() {
        let strings: Vec<&str> = (0..100)
            .map(|i| {
                if i % 3 == 0 {
                    "prefix_common_data_suffix"
                } else if i % 3 == 1 {
                    "prefix_other_data_suffix"
                } else {
                    "prefix_extra_data_suffix"
                }
            })
            .collect();

        let encoder = FsstEncoder::train(&strings, 200);

        let original_len: usize = strings.iter().map(|s| s.len()).sum();
        let compressed_len: usize = strings.iter().map(|s| encoder.encode(s).len()).sum();

        assert!(compressed_len < original_len);
    }

    #[test]
    fn test_fsst_column() {
        let strings = vec![
            Some("hello world"),
            None,
            Some("hello rust"),
            Some("hello code"),
        ];

        let column = build_fsst_column(&strings, 100);

        assert_eq!(column.len(), 4);
        assert_eq!(column.get(0), Some("hello world".to_string()));
        assert!(column.null_bitmap.is_null(1));
        assert_eq!(column.get(2), Some("hello rust".to_string()));
    }

    #[test]
    fn test_fsst_column_set() {
        let strings = vec![Some("hello world")];
        let mut column = build_fsst_column(&strings, 100);

        column.set(0, Some("hello rust")).unwrap();
        assert_eq!(column.get(0), Some("hello rust".to_string()));

        column.set(0, None).unwrap();
        assert!(column.null_bitmap.is_null(0));
    }

    #[test]
    fn test_fsst_column_set_out_of_bounds() {
        let strings = vec![Some("hello world")];
        let mut column = build_fsst_column(&strings, 100);

        let result = column.set(5, Some("test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_fsst_decode_zero_byte() {
        let encoder = FsstEncoder::new();
        let input: Vec<u8> = vec![0x00, 0x01, 0x00, 0x02];
        let decoded = encoder.decode(&input);
        assert_eq!(decoded, input);
    }

    #[test]
    fn test_fsst_encode_decode_with_zero_bytes() {
        let strings = vec!["ab", "cd"];
        let encoder = FsstEncoder::train(&strings, 100);

        let input = "a\x00b";
        let encoded = encoder.encode(input);
        let decoded = encoder.decode(&encoded);
        assert_eq!(decoded, input.as_bytes());
    }

    #[test]
    fn test_select_fsst() {
        let short_strings: Vec<String> = (0..100).map(|i| format!("s{}", i)).collect();
        let short_refs: Vec<&str> = short_strings.iter().map(|s| s.as_str()).collect();
        assert!(!select_fsst(&short_refs));

        let long_strings: Vec<String> = (0..100)
            .map(|i| format!("very_long_string_with_common_prefix_{}", i))
            .collect();
        let long_refs: Vec<&str> = long_strings.iter().map(|s| s.as_str()).collect();
        assert!(select_fsst(&long_refs));
    }

    #[test]
    fn test_fsst_roundtrip() {
        let strings: Vec<&str> = vec![
            "https://example.com/page/1",
            "https://example.com/page/2",
            "https://example.com/page/3",
            "https://example.com/page/4",
            "https://example.com/page/5",
        ];

        let encoder = FsstEncoder::train(&strings, 200);

        for s in &strings {
            let encoded = encoder.encode(s);
            let decoded = encoder.decode_to_string(&encoded);
            assert_eq!(decoded, Some(s.to_string()));
        }
    }

    #[test]
    fn test_empty_strings() {
        let strings: Vec<&str> = vec![];
        let encoder = FsstEncoder::train(&strings, 100);
        assert_eq!(encoder.symbol_count(), 0);

        let encoded = encoder.encode("");
        assert!(encoded.is_empty());
    }

    #[test]
    fn test_encode() {
        let strings = vec!["hello world", "hello rust"];
        let encoder = FsstEncoder::train(&strings, 100);

        let _ = encoder.encode("hello world");
        let _ = encoder.encode("hello rust");
    }

    #[test]
    fn test_append_with_stats() {
        let strings = vec![Some("hello world")];
        let mut column = build_fsst_column(&strings, 100);

        column
            .encoded_data
            .push(column.encoder.encode("hello rust"));
        column.null_bitmap.push(false);
        column
            .encoded_data
            .push(column.encoder.encode("hello code"));
        column.null_bitmap.push(false);

        let mut original_size = 0usize;
        let mut compressed_size = 0usize;
        for (idx, data) in column.encoded_data.iter().enumerate() {
            if !column.null_bitmap.is_null(idx) {
                original_size += column.encoder.decode(data).len();
                compressed_size += data.len();
            }
        }

        assert!(original_size >= compressed_size);
    }

    #[test]
    fn test_large_training_set() {
        let strings: Vec<String> = (0..20000)
            .map(|i| format!("long_string_with_prefix_{}", i))
            .collect();
        let refs: Vec<&str> = strings.iter().map(|s| s.as_str()).collect();

        let encoder = FsstEncoder::train(&refs, 100);
        assert!(encoder.symbol_count() > 0);
    }
}
