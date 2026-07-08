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

use crate::utils::NullBitmap;

const MAX_SYMBOL_LEN: usize = 8;
const MIN_SYMBOL_LEN: usize = 2;
const SYMBOL_TABLE_SIZE: usize = 255;
const MAX_TRAINING_SAMPLES: usize = 10000;
const MAX_NGRAMS_PER_STRING: usize = 1000;

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
        if strings.is_empty() {
            return Self::new();
        }

        let mut encoder = Self::new();
        encoder.build_symbol_table(strings, max_symbols);
        encoder
    }

    fn build_symbol_table(&mut self, strings: &[&str], max_symbols: usize) {
        let sampled: Vec<&str> = if strings.len() > MAX_TRAINING_SAMPLES {
            let step = strings.len() / MAX_TRAINING_SAMPLES;
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
                    let ngram: Vec<u8> = bytes[i..i + len].to_vec();
                    *ngram_freq.entry(ngram).or_insert(0) += 1;
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
            score_b.cmp(&score_a)
        });

        for (code, (ngram, _freq)) in (1_u8..).zip(ngrams) {
            if code as usize >= max_symbols.min(SYMBOL_TABLE_SIZE) {
                break;
            }
            self.table.insert(ngram, code);
        }
    }

    pub fn encode(&self, s: &str) -> Vec<u8> {
        let bytes = s.as_bytes();
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
}

impl FsstColumn {
    pub fn new() -> Self {
        Self {
            encoder: FsstEncoder::new(),
            encoded_data: Vec::new(),
            null_bitmap: NullBitmap::new(),
        }
    }

    pub fn get(&self, row_idx: usize) -> Option<String> {
        if row_idx >= self.encoded_data.len() || self.null_bitmap.is_null(row_idx) {
            return None;
        }

        self.encoder.decode_to_string(&self.encoded_data[row_idx])
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&str>) -> crate::core::StorageResult<()> {
        if row_idx >= self.encoded_data.len() {
            return Err(crate::core::StorageError::invalid_offset(row_idx as u32));
        }

        match value {
            Some(s) => {
                self.encoded_data[row_idx] = self.encoder.encode(s);
                self.null_bitmap.set(row_idx, false);
            }
            None => {
                self.encoded_data[row_idx].clear();
                self.null_bitmap.set(row_idx, true);
            }
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.encoded_data.len()
    }

    pub fn memory_usage(&self) -> usize {
        let data_size: usize = self.encoded_data.iter().map(|v| v.len()).sum();
        let null_size = self.null_bitmap.memory_usage();
        let table_size = self.encoder.table().memory_usage();

        data_size + null_size + table_size
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
