//! Compression Strategy Selector
//!
//! Automatically selects the optimal compression algorithm based on
//! data characteristics and access patterns.
//!
//! # Strategy
//!
//! 1. Analyze data statistics (cardinality, range, patterns)
//! 2. Consider access patterns (hot/cold data)
//! 3. Select encoding that balances compression ratio and query speed

use crate::core::{DataType, Value};

use super::EncodingType;

#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub row_count: usize,
    pub null_count: usize,
    pub distinct_count: usize,
    pub min_value: Option<Value>,
    pub max_value: Option<Value>,
    pub avg_length: f64,
    pub run_count: usize,
    pub data_type: DataType,
}

impl Default for ColumnStats {
    fn default() -> Self {
        Self {
            row_count: 0,
            null_count: 0,
            distinct_count: 0,
            min_value: None,
            max_value: None,
            avg_length: 0.0,
            run_count: 0,
            data_type: DataType::String,
        }
    }
}

impl ColumnStats {
    pub fn new(data_type: DataType) -> Self {
        Self {
            data_type,
            ..Default::default()
        }
    }

    pub fn cardinality_ratio(&self) -> f64 {
        if self.row_count == 0 {
            return 0.0;
        }
        self.distinct_count as f64 / self.row_count as f64
    }

    pub fn run_ratio(&self) -> f64 {
        if self.row_count == 0 {
            return 1.0;
        }
        self.run_count as f64 / self.row_count as f64
    }

    pub fn value_range(&self) -> Option<u64> {
        match (&self.min_value, &self.max_value) {
            (Some(Value::SmallInt(min)), Some(Value::SmallInt(max))) => Some((*max - *min) as u64),
            (Some(Value::Int(min)), Some(Value::Int(max))) => Some((*max - *min) as u64),
            (Some(Value::BigInt(min)), Some(Value::BigInt(max))) => Some((*max - *min) as u64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub min_rows_for_compression: usize,
    pub max_dictionary_size: usize,
    pub string_min_rows: usize,
    pub int_min_rows: usize,
    pub float_min_rows: usize,
    pub skip_high_cardinality_short_strings: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            min_rows_for_compression: 100,
            max_dictionary_size: 10000,
            string_min_rows: 50,
            int_min_rows: 20,
            float_min_rows: 100,
            skip_high_cardinality_short_strings: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompressionSelector {
    config: CompressionConfig,
}

impl CompressionSelector {
    pub fn new() -> Self {
        Self {
            config: CompressionConfig::default(),
        }
    }

    pub fn with_config(config: CompressionConfig) -> Self {
        Self { config }
    }

    pub fn select(&self, stats: &ColumnStats) -> EncodingType {
        match stats.data_type {
            DataType::String => self.select_string_encoding(stats),
            DataType::Int | DataType::SmallInt | DataType::BigInt => {
                self.select_int_encoding(stats)
            }
            DataType::Float | DataType::Double => self.select_float_encoding(stats),
            DataType::Bool => self.select_bool_encoding(stats),
            _ => EncodingType::None,
        }
    }

    fn select_string_encoding(&self, stats: &ColumnStats) -> EncodingType {
        if stats.row_count < self.config.string_min_rows {
            return EncodingType::None;
        }

        let cardinality_ratio = stats.cardinality_ratio();

        if cardinality_ratio < 0.5 && stats.distinct_count < self.config.max_dictionary_size {
            let estimated_dict_size =
                stats.distinct_count * stats.avg_length as usize + stats.row_count * 4;
            let estimated_raw_size = stats.row_count * stats.avg_length as usize;

            if estimated_dict_size < estimated_raw_size {
                return EncodingType::Dictionary;
            }
        }

        if stats.avg_length >= 20.0 && cardinality_ratio > 0.5 {
            return EncodingType::Fsst;
        }

        if self.config.skip_high_cardinality_short_strings
            && cardinality_ratio > 0.8
            && stats.avg_length < 20.0
        {
            return EncodingType::None;
        }

        EncodingType::None
    }

    fn select_int_encoding(&self, stats: &ColumnStats) -> EncodingType {
        if stats.row_count < self.config.int_min_rows {
            return EncodingType::None;
        }

        let run_ratio = stats.run_ratio();

        if run_ratio < 0.3 {
            return EncodingType::Rle;
        }

        if let Some(range) = stats.value_range() {
            let bit_width = if range == 0 {
                1
            } else {
                64 - range.leading_zeros() as u8
            };

            if bit_width < 32 {
                return EncodingType::BitPacking;
            }
        }

        EncodingType::None
    }

    fn select_float_encoding(&self, stats: &ColumnStats) -> EncodingType {
        if stats.row_count < self.config.float_min_rows {
            return EncodingType::None;
        }

        EncodingType::Alp
    }

    fn select_bool_encoding(&self, stats: &ColumnStats) -> EncodingType {
        if stats.run_ratio() < 0.5 {
            return EncodingType::Rle;
        }
        EncodingType::None
    }
}

impl Default for CompressionSelector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_column_stats() {
        let stats = ColumnStats {
            row_count: 1000,
            null_count: 100,
            distinct_count: 50,
            data_type: DataType::String,
            ..Default::default()
        };

        assert!((stats.cardinality_ratio() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn test_compression_selector_string_dictionary() {
        let stats = ColumnStats {
            row_count: 1000,
            distinct_count: 50,
            avg_length: 20.0,
            data_type: DataType::String,
            ..Default::default()
        };

        let selector = CompressionSelector::new();
        let encoding = selector.select(&stats);

        assert_eq!(encoding, EncodingType::Dictionary);
    }

    #[test]
    fn test_compression_selector_string_fsst() {
        let stats = ColumnStats {
            row_count: 1000,
            distinct_count: 800,
            avg_length: 50.0,
            data_type: DataType::String,
            ..Default::default()
        };

        let selector = CompressionSelector::new();
        let encoding = selector.select(&stats);

        assert_eq!(encoding, EncodingType::Fsst);
    }

    #[test]
    fn test_compression_selector_int_rle() {
        let stats = ColumnStats {
            row_count: 1000,
            run_count: 100,
            data_type: DataType::Int,
            ..Default::default()
        };

        let selector = CompressionSelector::new();
        let encoding = selector.select(&stats);

        assert_eq!(encoding, EncodingType::Rle);
    }

    #[test]
    fn test_compression_selector_int_bitpacking() {
        let stats = ColumnStats {
            row_count: 1000,
            run_count: 800,
            min_value: Some(Value::Int(0)),
            max_value: Some(Value::Int(100)),
            data_type: DataType::Int,
            ..Default::default()
        };

        let selector = CompressionSelector::new();
        let encoding = selector.select(&stats);

        assert_eq!(encoding, EncodingType::BitPacking);
    }

    #[test]
    fn test_compression_selector_float_alp() {
        let stats = ColumnStats {
            row_count: 1000,
            data_type: DataType::Double,
            ..Default::default()
        };

        let selector = CompressionSelector::new();
        let encoding = selector.select(&stats);

        assert_eq!(encoding, EncodingType::Alp);
    }
}
