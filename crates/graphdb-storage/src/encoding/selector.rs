//! Encoding Selection Strategy
//!
//! Analyzes column data characteristics to choose the optimal encoding.
//! Thresholds are configurable via `EncodingThresholds`.

use crate::core::{DataType, Value};
use crate::encoding::EncodingType;

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
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EncodingFeedback {
    observations: Vec<FeedbackObservation>,
}

#[derive(Debug, Clone, Copy)]
struct FeedbackObservation {
    encoding_type: EncodingType,
    compression_ratio: f64,
}

impl EncodingFeedback {
    const MAX_OBSERVATIONS: usize = 100;
    const REEVALUATE_AFTER: usize = 20;

    fn record(&mut self, encoding_type: EncodingType, compression_ratio: f64) {
        if self.observations.len() >= Self::MAX_OBSERVATIONS {
            self.observations.remove(0);
        }
        self.observations.push(FeedbackObservation {
            encoding_type,
            compression_ratio,
        });
    }

    fn average_ratio(&self, encoding_type: EncodingType) -> Option<f64> {
        let ratios: Vec<f64> = self
            .observations
            .iter()
            .filter(|o| o.encoding_type == encoding_type)
            .map(|o| o.compression_ratio)
            .collect();
        if ratios.is_empty() {
            return None;
        }
        Some(ratios.iter().sum::<f64>() / ratios.len() as f64)
    }

    fn should_reevaluate(&self, encoding_type: EncodingType) -> bool {
        let count = self
            .observations
            .iter()
            .filter(|o| o.encoding_type == encoding_type)
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

    pub fn record_compression_result(
        &mut self,
        encoding_type: EncodingType,
        compression_ratio: f64,
    ) {
        self.feedback.record(encoding_type, compression_ratio);
    }

    pub fn should_reencode(&self, encoding_type: EncodingType) -> bool {
        if !self.feedback.should_reevaluate(encoding_type) {
            return false;
        }
        if let Some(avg_ratio) = self.feedback.average_ratio(encoding_type) {
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

        if cardinality_ratio <= self.thresholds.cardinality_ratio_threshold {
            return EncodingType::Dictionary;
        }

        if avg_len >= self.thresholds.avg_length_threshold {
            // Only use FSST when cardinality exceeds the rebuild threshold,
            // avoiding unnecessary FSST model maintenance on near-constant data.
            if cardinality_ratio >= self.thresholds.fsst_rebuild_threshold {
                return EncodingType::Fsst;
            }
        }

        if cardinality_ratio < 0.8 {
            return EncodingType::Dictionary;
        }

        EncodingType::Fsst
    }

    /// Select encoding for floating-point columns.
    pub fn select_for_floats(&self, _values: &[Option<Value>]) -> EncodingType {
        EncodingType::Alp
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
        let values: Vec<Option<Value>> = (0..100)
            .map(|i| Some(Value::Double(i as f64 * 0.1)))
            .collect();
        assert_eq!(selector.select_for_floats(&values), EncodingType::Alp);
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
            selector.record_compression_result(EncodingType::Dictionary, 0.95);
        }
        assert!(selector.should_reencode(EncodingType::Dictionary));
    }

    #[test]
    fn test_no_reencode_with_few_observations() {
        let mut selector = EncodingSelector::default();
        for _ in 0..5 {
            selector.record_compression_result(EncodingType::Dictionary, 0.95);
        }
        assert!(!selector.should_reencode(EncodingType::Dictionary));
    }

    #[test]
    fn test_no_reencode_with_good_ratio() {
        let mut selector = EncodingSelector::default();
        for _ in 0..EncodingFeedback::REEVALUATE_AFTER {
            selector.record_compression_result(EncodingType::Dictionary, 0.5);
        }
        assert!(!selector.should_reencode(EncodingType::Dictionary));
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
