//! Encoding Selection Strategy
//!
//! Analyzes column data characteristics to choose the optimal encoding.
//! Thresholds are configurable via `EncodingThresholds`.

use crate::encoding::{ConstantColumn, EncodingType};
use graphdb_core::{DataType, Value};

/// Configurable thresholds for encoding selection.
#[derive(Debug, Clone)]
pub struct EncodingThresholds {
    /// Minimum number of rows required for string encoding analysis.
    pub string_min_rows: usize,
    /// Minimum average string length to consider FSST encoding.
    pub avg_length_threshold: usize,
    /// Cardinality ratio (distinct / total) below which Dictionary is preferred.
    pub cardinality_ratio_threshold: f64,
    /// Ratio of new data to existing data that triggers FSST rebuild.
    pub fsst_rebuild_threshold: f64,
    /// Compression ratio above which re-encoding is recommended.
    /// When the average compressed_size/raw_size exceeds this threshold,
    /// the column should be re-evaluated with a different encoding.
    pub reencode_threshold: f64,
    /// Maximum number of symbols for FSST encoding.
    pub fsst_max_symbols: usize,
    /// ALP exception-rate ceiling above which floats fall back to raw.
    pub alp_exception_threshold: f64,
    /// Maximum dictionary entries per chunk.
    pub dict_max_entries_per_chunk: usize,
    /// Chunk update count above which a chunk is considered hot.
    pub hot_update_threshold: u64,
}

impl Default for EncodingThresholds {
    fn default() -> Self {
        Self {
            string_min_rows: 50,
            avg_length_threshold: 16,
            cardinality_ratio_threshold: 0.5,
            fsst_rebuild_threshold: 0.2,
            reencode_threshold: 0.8,
            fsst_max_symbols: 255,
            alp_exception_threshold: 0.25,
            dict_max_entries_per_chunk: 65536,
            hot_update_threshold: 1000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataTypeFamily {
    Integer,
    Float,
    Bool,
    String,
    Other,
}

pub fn data_type_family(data_type: &DataType) -> DataTypeFamily {
    match data_type {
        DataType::SmallInt | DataType::Int | DataType::BigInt => DataTypeFamily::Integer,
        DataType::Float | DataType::Double => DataTypeFamily::Float,
        DataType::Bool => DataTypeFamily::Bool,
        DataType::String => DataTypeFamily::String,
        _ => DataTypeFamily::Other,
    }
}

#[derive(Debug, Clone, Default)]
struct EncodingFeedback {
    observations: Vec<FeedbackObservation>,
}

#[derive(Debug, Clone, Copy)]
struct FeedbackObservation {
    encoding_type: EncodingType,
    family: DataTypeFamily,
    compression_ratio: f64,
}

impl EncodingFeedback {
    const MAX_OBSERVATIONS: usize = 100;
    const REEVALUATE_AFTER: usize = 20;

    fn record(
        &mut self,
        encoding_type: EncodingType,
        family: DataTypeFamily,
        compression_ratio: f64,
    ) {
        if self.observations.len() >= Self::MAX_OBSERVATIONS {
            self.observations.remove(0);
        }
        self.observations.push(FeedbackObservation {
            encoding_type,
            family,
            compression_ratio,
        });
    }

    fn average_ratio(
        &self,
        encoding_type: EncodingType,
        family: Option<DataTypeFamily>,
    ) -> Option<f64> {
        let ratios: Vec<f64> = self
            .observations
            .iter()
            .filter(|o| o.encoding_type == encoding_type && family.is_none_or(|f| o.family == f))
            .map(|o| o.compression_ratio)
            .collect();
        if ratios.is_empty() {
            return None;
        }
        Some(ratios.iter().sum::<f64>() / ratios.len() as f64)
    }

    fn should_reevaluate(
        &self,
        encoding_type: EncodingType,
        family: Option<DataTypeFamily>,
    ) -> bool {
        let count = self
            .observations
            .iter()
            .filter(|o| o.encoding_type == encoding_type && family.is_none_or(|f| o.family == f))
            .count();
        count >= Self::REEVALUATE_AFTER
    }
}

/// Analyzes data characteristics and selects the optimal encoding.
#[derive(Debug, Clone)]
pub struct EncodingSelector {
    thresholds: EncodingThresholds,
    feedback: EncodingFeedback,
}

impl EncodingSelector {
    pub fn new(thresholds: EncodingThresholds) -> Self {
        Self {
            thresholds,
            feedback: EncodingFeedback::default(),
        }
    }

    pub fn thresholds(&self) -> &EncodingThresholds {
        &self.thresholds
    }

    pub fn record_compression_result_for(
        &mut self,
        encoding_type: EncodingType,
        family: DataTypeFamily,
        compression_ratio: f64,
    ) {
        self.feedback
            .record(encoding_type, family, compression_ratio);
    }

    pub fn should_reencode_for(&self, encoding_type: EncodingType, family: DataTypeFamily) -> bool {
        if !self.feedback.should_reevaluate(encoding_type, Some(family)) {
            return false;
        }
        if let Some(avg_ratio) = self.feedback.average_ratio(encoding_type, Some(family)) {
            avg_ratio > self.thresholds.reencode_threshold
        } else {
            false
        }
    }

    /// Select encoding for integer columns.
    pub fn select_for_integers(&self, values: &[Option<Value>]) -> EncodingType {
        let non_null: Vec<i64> = values
            .iter()
            .filter_map(|v| match v {
                Some(Value::SmallInt(v)) => Some(*v as i64),
                Some(Value::Int(v)) => Some(*v as i64),
                Some(Value::BigInt(v)) => Some(*v),
                _ => None,
            })
            .collect();

        if non_null.len() < self.thresholds.string_min_rows {
            return EncodingType::BitPacking;
        }

        let runs = count_runs(&non_null);
        let run_ratio = runs as f64 / non_null.len() as f64;

        if run_ratio < 0.1 {
            EncodingType::Rle
        } else {
            EncodingType::BitPacking
        }
    }

    /// Select encoding for string columns.
    pub fn select_for_strings(&self, values: &[Option<Value>]) -> EncodingType {
        let non_null: Vec<&str> = values
            .iter()
            .filter_map(|v| match v {
                Some(Value::String(s)) => Some(s.as_str()),
                _ => None,
            })
            .collect();

        if non_null.len() < self.thresholds.string_min_rows {
            return EncodingType::Dictionary;
        }

        let total_len: usize = non_null.iter().map(|s| s.len()).sum();
        let avg_len = total_len / non_null.len();

        let distinct: std::collections::HashSet<&str> = non_null.iter().copied().collect();
        let cardinality_ratio = distinct.len() as f64 / non_null.len() as f64;
        // A dictionary larger than the per-chunk entry cap is never worth
        // building; fall through to the FSST/raw decision instead.
        let dict_fits = distinct.len() <= self.thresholds.dict_max_entries_per_chunk;

        if dict_fits && cardinality_ratio <= self.thresholds.cardinality_ratio_threshold {
            return EncodingType::Dictionary;
        }

        if avg_len >= self.thresholds.avg_length_threshold {
            // Only use FSST when cardinality exceeds the rebuild threshold,
            // avoiding unnecessary FSST model maintenance on near-constant data.
            if cardinality_ratio >= self.thresholds.fsst_rebuild_threshold {
                return EncodingType::Fsst;
            }
        }

        if dict_fits && cardinality_ratio < 0.8 {
            return EncodingType::Dictionary;
        }

        EncodingType::Fsst
    }

    /// Select encoding for floating-point columns. Falls back to raw when
    /// the ALP exception rate exceeds the configured ceiling.
    pub fn select_for_floats(&self, values: &[Option<Value>]) -> EncodingType {
        let analyzed = values
            .first()
            .and_then(|v| v.as_ref())
            .map(|v| v.data_type())
            .map(|data_type| crate::encoding::AlpColumn::analyze_values(values, data_type));
        match analyzed {
            Some(Ok(col)) if col.exception_rate() <= self.thresholds.alp_exception_threshold => {
                EncodingType::Alp
            }
            _ => EncodingType::None,
        }
    }

    /// Select encoding for boolean columns.
    pub fn select_for_booleans(&self, _values: &[Option<Value>]) -> EncodingType {
        EncodingType::Rle
    }

    /// Select encoding based on data type and values.
    pub fn select_for_column(
        &self,
        data_type: &DataType,
        values: &[Option<Value>],
    ) -> EncodingType {
        if ConstantColumn::should_use(values) {
            return EncodingType::Constant;
        }
        match data_type {
            DataType::Bool => self.select_for_booleans(values),
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                self.select_for_integers(values)
            }
            DataType::Float | DataType::Double => self.select_for_floats(values),
            DataType::String => self.select_for_strings(values),
            _ => EncodingType::None,
        }
    }

    /// Chunk-aware encoding selection.
    ///
    /// Takes a single chunk slice (local min/max/distribution) instead of
    /// column-level stats so each chunk independently picks the encoding
    /// that best fits its local data distribution.
    pub fn select_for_chunk(
        &self,
        data_type: &DataType,
        chunk_values: &[Option<Value>],
    ) -> EncodingType {
        self.select_for_column(data_type, chunk_values)
    }

    /// Profile-driven chunk selection without materializing value vectors.
    pub fn select_for_chunk_profile(
        &self,
        profile: &crate::encoding::ChunkProfile,
    ) -> EncodingType {
        if profile.num_values == 0 {
            return EncodingType::Constant;
        }
        // Constant chunks are O(1) regardless of size.
        if let (Some(min), Some(max)) = (profile.min.clone(), profile.max.clone()) {
            if min == max {
                return EncodingType::Constant;
            }
        }
        // Small chunks keep simple heuristics to avoid fitting elaborate
        // encodings to noise.
        let small = profile.num_values < self.thresholds.string_min_rows;
        match &profile.data_type {
            DataType::String if small => return EncodingType::Dictionary,
            DataType::SmallInt | DataType::Int | DataType::BigInt if small => {
                return EncodingType::BitPacking
            }
            _ => {}
        }
        match &profile.data_type {
            DataType::Bool => EncodingType::Rle,
            DataType::SmallInt | DataType::Int | DataType::BigInt => {
                if profile.hot_update && profile.run_ratio.unwrap_or(1.0) >= 0.1 {
                    return EncodingType::None;
                }
                match profile.bit_width {
                    Some(64) => EncodingType::None,
                    _ => {
                        if profile.run_ratio.unwrap_or(1.0) < 0.1 {
                            EncodingType::Rle
                        } else {
                            EncodingType::BitPacking
                        }
                    }
                }
            }
            DataType::Float | DataType::Double => EncodingType::Alp,
            DataType::String => {
                let total = profile.num_values.max(1);
                let distinct = profile.distinct.unwrap_or(total);
                let ratio = distinct as f64 / total as f64;
                let dict_fits = distinct <= self.thresholds.dict_max_entries_per_chunk;
                if dict_fits && ratio <= self.thresholds.cardinality_ratio_threshold {
                    return EncodingType::Dictionary;
                }
                let avg_len = profile.total_str_len.unwrap_or(0) / total;
                if avg_len >= self.thresholds.avg_length_threshold
                    && ratio >= self.thresholds.fsst_rebuild_threshold
                {
                    return EncodingType::Fsst;
                }
                if dict_fits && ratio < 0.8 {
                    return EncodingType::Dictionary;
                }
                EncodingType::Fsst
            }
            _ => EncodingType::None,
        }
    }
}

impl Default for EncodingSelector {
    fn default() -> Self {
        Self::new(EncodingThresholds::default())
    }
}

fn count_runs(values: &[i64]) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut runs = 1;
    for window in values.windows(2) {
        if window[0] != window[1] {
            runs += 1;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_rle_for_low_cardinality_integers() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100).map(|_| Some(Value::Int(42))).collect();
        assert_eq!(selector.select_for_integers(&values), EncodingType::Rle);
    }

    #[test]
    fn test_select_bitpacking_for_high_cardinality_integers() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100).map(|i| Some(Value::Int(i))).collect();
        assert_eq!(
            selector.select_for_integers(&values),
            EncodingType::BitPacking
        );
    }

    #[test]
    fn test_select_dictionary_for_low_cardinality_strings() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| Some(Value::string(format!("val_{}", i % 5))))
            .collect();
        assert_eq!(
            selector.select_for_strings(&values),
            EncodingType::Dictionary
        );
    }

    #[test]
    fn test_select_fsst_for_high_cardinality_long_strings() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| {
                Some(Value::string(format!(
                    "https://example.com/very_long_path_parameter_{}",
                    i
                )))
            })
            .collect();
        assert_eq!(selector.select_for_strings(&values), EncodingType::Fsst);
    }

    #[test]
    fn test_fallback_dictionary_for_mid_cardinality_short_strings() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| Some(Value::string(format!("s{}", i % 60))))
            .collect();
        assert_eq!(
            selector.select_for_strings(&values),
            EncodingType::Dictionary
        );
    }

    #[test]
    fn test_select_alp_for_floats() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100).map(|i| Some(Value::Double(i as f64))).collect();
        assert_eq!(selector.select_for_floats(&values), EncodingType::Alp);
    }

    #[test]
    fn test_select_raw_when_alp_exceptions_exceed_threshold() {
        let selector = EncodingSelector::default();
        // Fractional multiples defeat ALP exponent sharing: the exception
        // rate exceeds the 0.25 ceiling, so selection falls back to raw.
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| Some(Value::Double(i as f64 * 0.1)))
            .collect();
        assert_eq!(selector.select_for_floats(&values), EncodingType::None);
    }

    #[test]
    fn test_select_rle_for_booleans() {
        let selector = EncodingSelector::default();
        let values: Vec<Option<Value>> = (0..100).map(|i| Some(Value::Bool(i % 2 == 0))).collect();
        assert_eq!(selector.select_for_booleans(&values), EncodingType::Rle);
    }

    #[test]
    fn test_configurable_thresholds() {
        let thresholds = EncodingThresholds {
            string_min_rows: 10,
            avg_length_threshold: 8,
            cardinality_ratio_threshold: 0.3,
            fsst_rebuild_threshold: 0.5,
            reencode_threshold: 0.9,
            fsst_max_symbols: 128,
            ..Default::default()
        };
        let selector = EncodingSelector::new(thresholds);
        assert_eq!(selector.thresholds().string_min_rows, 10);
        assert_eq!(selector.thresholds().avg_length_threshold, 8);
        assert!((selector.thresholds().cardinality_ratio_threshold - 0.3).abs() < f64::EPSILON);
        assert!((selector.thresholds().fsst_rebuild_threshold - 0.5).abs() < f64::EPSILON);
        assert!((selector.thresholds().reencode_threshold - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_feedback_triggers_reencode() {
        let mut selector = EncodingSelector::default();
        for _ in 0..EncodingFeedback::REEVALUATE_AFTER {
            selector.record_compression_result_for(
                EncodingType::Dictionary,
                DataTypeFamily::String,
                0.95,
            );
        }
        assert!(selector.should_reencode_for(EncodingType::Dictionary, DataTypeFamily::String));
    }

    #[test]
    fn test_no_reencode_with_few_observations() {
        let mut selector = EncodingSelector::default();
        for _ in 0..5 {
            selector.record_compression_result_for(
                EncodingType::Dictionary,
                DataTypeFamily::String,
                0.95,
            );
        }
        assert!(!selector.should_reencode_for(EncodingType::Dictionary, DataTypeFamily::String));
    }

    #[test]
    fn test_no_reencode_with_good_ratio() {
        let mut selector = EncodingSelector::default();
        for _ in 0..EncodingFeedback::REEVALUATE_AFTER {
            selector.record_compression_result_for(
                EncodingType::Dictionary,
                DataTypeFamily::String,
                0.5,
            );
        }
        assert!(!selector.should_reencode_for(EncodingType::Dictionary, DataTypeFamily::String));
    }

    #[test]
    fn test_reencode_for_is_family_scoped() {
        let mut selector = EncodingSelector::default();
        for _ in 0..EncodingFeedback::REEVALUATE_AFTER {
            selector.record_compression_result_for(
                EncodingType::Dictionary,
                DataTypeFamily::String,
                0.95,
            );
        }
        assert!(selector.should_reencode_for(EncodingType::Dictionary, DataTypeFamily::String));
        assert!(!selector.should_reencode_for(EncodingType::Dictionary, DataTypeFamily::Integer));
    }

    #[test]
    fn test_select_for_column_dispatch() {
        let selector = EncodingSelector::default();
        let int_values: Vec<Option<Value>> = (0..100).map(|i| Some(Value::Int(i))).collect();
        assert_eq!(
            selector.select_for_column(&DataType::Int, &int_values),
            EncodingType::BitPacking
        );

        let bool_values: Vec<Option<Value>> =
            (0..100).map(|i| Some(Value::Bool(i % 2 == 0))).collect();
        assert_eq!(
            selector.select_for_column(&DataType::Bool, &bool_values),
            EncodingType::Rle
        );

        assert_eq!(
            selector.select_for_column(&DataType::VectorDense(0), &[]),
            EncodingType::None
        );
    }
}
