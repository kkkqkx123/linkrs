//! Histogram Statistics Module
//!
//! Use an equi-depth histogram to record the distribution of attribute values.
//! Each histogram contains a fixed number of bins, and each bin records the same number of tuples.

use crate::core::value::Value;
use std::time::Instant;

/// Histogram bins
#[derive(Debug, Clone)]
pub struct HistogramBucket {
    /// Upper bound of the bucket (inclusive)
    pub upper_bound: Value,
    /// Number of tuples in the bucket
    pub count: u64,
    /// Number of Different Values (NDV)
    pub distinct_values: u64,
}

impl HistogramBucket {
    /// Create new histogram buckets.
    pub fn new(upper_bound: Value, count: u64, distinct_values: u64) -> Self {
        Self {
            upper_bound,
            count,
            distinct_values: distinct_values.max(1),
        }
    }
}

/// Range condition types
#[derive(Debug, Clone)]
pub enum RangeCondition {
    /// less than
    Lt(Value),
    /// Less than or equal to
    Le(Value),
    /// greater than
    Gt(Value),
    /// greater than or equal to
    Ge(Value),
    /// Range [low, high)
    Range { low: Value, high: Value },
}

/// Isobathic histogram
#[derive(Debug, Clone)]
pub struct Histogram {
    /// Bucket list (sorted by upper bound)
    buckets: Vec<HistogramBucket>,
    /// Proportion of null values
    null_fraction: f64,
    /// Total number of different values
    total_distinct_values: u64,
    /// Total number of records
    total_count: u64,
    /// Last update time
    last_updated: Instant,
}

impl Histogram {
    /// Create an empty histogram.
    pub fn empty() -> Self {
        Self {
            buckets: Vec::new(),
            null_fraction: 0.0,
            total_distinct_values: 0,
            total_count: 0,
            last_updated: Instant::now(),
        }
    }

    /// Constructing an isobathic histogram from the sampled data
    ///
    /// # Parameters
    /// “samples”: A list of sampled values
    /// `num_buckets`: The number of buckets
    /// `total_count`: The total number of records (used to calculate the proportion of null values).
    ///
    /// # Description
    /// An isochore plot ensures that each bin contains approximately the same number of samples.
    pub fn from_samples(samples: Vec<Value>, num_buckets: usize, total_count: u64) -> Self {
        if samples.is_empty() || num_buckets == 0 {
            return Self::empty();
        }

        let mut sorted_samples = samples;
        // Separate null values from non-null values
        let null_count = sorted_samples.iter().filter(|v| v.is_null()).count();
        sorted_samples.retain(|v| !v.is_null());

        if sorted_samples.is_empty() {
            return Self {
                buckets: Vec::new(),
                null_fraction: null_count as f64 / total_count.max(1) as f64,
                total_distinct_values: 0,
                total_count,
                last_updated: Instant::now(),
            };
        }

        // Sort by value
        sorted_samples.sort_by(compare_values);

        // Calculating the number of different values
        let distinct_values = calculate_distinct_values(&sorted_samples);

        // Constructing an isohyetic histogram
        let non_null_count = sorted_samples.len();
        let bucket_size = non_null_count / num_buckets;
        let remainder = non_null_count % num_buckets;

        let mut buckets = Vec::with_capacity(num_buckets);
        let mut start_idx = 0;

        for i in 0..num_buckets {
            let current_bucket_size = bucket_size + if i < remainder { 1 } else { 0 };
            if current_bucket_size == 0 {
                continue;
            }

            let end_idx = (start_idx + current_bucket_size).min(sorted_samples.len());
            let bucket_samples = &sorted_samples[start_idx..end_idx];

            if let Some(upper_bound) = bucket_samples.last().cloned() {
                let count = bucket_samples.len() as u64;
                let bucket_distinct = calculate_distinct_values(bucket_samples);
                buckets.push(HistogramBucket::new(upper_bound, count, bucket_distinct));
            }

            start_idx = end_idx;
        }

        Self {
            buckets,
            null_fraction: null_count as f64 / total_count.max(1) as f64,
            total_distinct_values: distinct_values,
            total_count,
            last_updated: Instant::now(),
        }
    }

    /// Estimated equivalence query selectivity
    ///
    /// # Description
    /// Find the bucket that contains that value, using the assumption of a uniform distribution within the bucket: 1 / the number of unique values (NDV) in the bucket.
    pub fn estimate_equality_selectivity(&self, value: &Value) -> f64 {
        if value.is_null() {
            return self.null_fraction;
        }

        if self.buckets.is_empty() {
            return 0.1; // Default selection option
        }

        // Find the corresponding bucket.
        match self.find_bucket(value) {
            Some(bucket_idx) => {
                let bucket = &self.buckets[bucket_idx];
                // The assumption of uniform distribution within the barrel
                let bucket_selectivity = 1.0 / bucket.distinct_values.max(1) as f64;
                // Consider the proportion of the barrel in the overall structure.
                let bucket_ratio = bucket.count as f64 / self.total_count.max(1) as f64;
                bucket_selectivity * bucket_ratio
            }
            None => {
                // The value exceeds the range of the histogram; therefore, the minimum selective estimation method is used.
                1.0 / self.total_distinct_values.max(1) as f64
            }
        }
    }

    /// Selective query within the estimated range
    pub fn estimate_range_selectivity(&self, range: &RangeCondition) -> f64 {
        match range {
            RangeCondition::Lt(value) | RangeCondition::Le(value) => {
                self.estimate_less_than_selectivity(value)
            }
            RangeCondition::Gt(value) | RangeCondition::Ge(value) => {
                1.0 - self.estimate_less_than_selectivity(value)
            }
            RangeCondition::Range { low, high } => {
                let high_selectivity = self.estimate_less_than_selectivity(high);
                let low_selectivity = self.estimate_less_than_selectivity(low);
                (high_selectivity - low_selectivity).max(0.0)
            }
        }
    }

    /// Estimated selectivity that is less than the specified condition
    fn estimate_less_than_selectivity(&self, value: &Value) -> f64 {
        if self.buckets.is_empty() {
            return 0.333; // Default range selection
        }

        let mut selectivity = 0.0;
        let mut found = false;

        for bucket in &self.buckets {
            if compare_values(value, &bucket.upper_bound) != std::cmp::Ordering::Greater {
                // The value is within the current bucket range.
                // Assuming the contents of the barrel are evenly distributed, estimate the proportion of items that are smaller than the given value.
                let bucket_ratio = bucket.count as f64 / self.total_count.max(1) as f64;
                selectivity += bucket_ratio * 0.5; // Simplified estimate: Take half of the bucket.
                found = true;
                break;
            } else {
                // The entire bucket contains less than the value specified.
                selectivity += bucket.count as f64 / self.total_count.max(1) as f64;
            }
        }

        if !found {
            // The value is greater than the upper limit of all buckets.
            selectivity = 1.0 - self.null_fraction;
        }

        selectivity.min(1.0 - self.null_fraction)
    }

    /// Find the bucket index where the value is located.
    fn find_bucket(&self, value: &Value) -> Option<usize> {
        for (idx, bucket) in self.buckets.iter().enumerate() {
            if compare_values(value, &bucket.upper_bound) != std::cmp::Ordering::Greater {
                return Some(idx);
            }
        }
        None
    }

    /// Obtain the number of buckets
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Obtain the proportion of null values
    pub fn null_fraction(&self) -> f64 {
        self.null_fraction
    }

    /// Get the total number of records.
    pub fn total_count(&self) -> u64 {
        self.total_count
    }

    /// Obtaining different numbers of values
    pub fn distinct_values(&self) -> u64 {
        self.total_distinct_values
    }

    /// Get the last update time.
    pub fn last_updated(&self) -> Instant {
        self.last_updated
    }
}

/// Compare two values
fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (Value::SmallInt(a), Value::SmallInt(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Double(a), Value::Double(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        // Different types are sorted by their type (using the type ID as the sorting criterion).
        _ => {
            let type_a = std::mem::discriminant(a);
            let type_b = std::mem::discriminant(b);
            // Using address comparison as a stable basis for sorting.
            let ptr_a = &type_a as *const _ as usize;
            let ptr_b = &type_b as *const _ as usize;
            ptr_a.cmp(&ptr_b)
        }
    }
}

/// Calculating the number of different values
fn calculate_distinct_values(values: &[Value]) -> u64 {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    for value in values {
        // Use a string representation of the value as the hash key.
        let key = format!("{:?}", value);
        seen.insert(key);
    }
    seen.len() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_histogram_empty() {
        let hist = Histogram::empty();
        assert_eq!(hist.bucket_count(), 0);
        assert_eq!(hist.null_fraction(), 0.0);
    }

    #[test]
    fn test_histogram_from_samples() {
        let samples: Vec<Value> = (1..=100).map(Value::Int).collect();
        let hist = Histogram::from_samples(samples, 10, 100);

        assert_eq!(hist.bucket_count(), 10);
        assert_eq!(hist.total_count(), 100);
    }

    #[test]
    fn test_estimate_equality_selectivity() {
        let samples: Vec<Value> = (1..=100).map(Value::Int).collect();
        let hist = Histogram::from_samples(samples, 10, 100);

        // For uniformly distributed data, the selectivity should be close to 1/100 = 0.01
        let selectivity = hist.estimate_equality_selectivity(&Value::Int(50));
        assert!(selectivity > 0.0 && selectivity < 0.1);
    }

    #[test]
    fn test_estimate_range_selectivity() {
        let samples: Vec<Value> = (1..=100).map(Value::Int).collect();
        let hist = Histogram::from_samples(samples, 10, 100);

        // Estimates of selectivity less than 50 should be closer to 0.5
        let range = RangeCondition::Lt(Value::Int(50));
        let selectivity = hist.estimate_range_selectivity(&range);
        assert!(selectivity > 0.3 && selectivity < 0.7);
    }

    #[test]
    fn test_null_handling() {
        let mut samples: Vec<Value> = (1..=90).map(Value::Int).collect();
        // Add 10 empty values.
        for _ in 0..10 {
            samples.push(Value::Null(crate::core::value::NullType::Null));
        }

        let hist = Histogram::from_samples(samples, 10, 100);
        assert!((hist.null_fraction() - 0.1).abs() < 0.01);

        let null_selectivity =
            hist.estimate_equality_selectivity(&Value::Null(crate::core::value::NullType::Null));
        assert!((null_selectivity - 0.1).abs() < 0.01);
    }
}
