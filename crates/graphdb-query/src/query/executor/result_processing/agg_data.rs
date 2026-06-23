//! Aggregated Data Status Module
//!
//! Refer to the AggData design of nebula-graph to manage the intermediate states and final results of the aggregation functions.

use crate::core::value::{NullType, Value};
use std::collections::HashSet;

/// Aggregated data status
///
/// Corresponding to the AggData of nebula-graph, it stores the intermediate states as well as the final results of the aggregation calculations.
#[derive(Debug, Clone)]
pub struct AggData {
    /// Counting (used for functions such as COUNT, AVG, STD, etc.)
    cnt: Value,
    /// Cumulative sum (used for functions such as SUM, AVG, etc.)
    sum: Value,
    /// Average value (used for AVG)
    avg: Value,
    /// Variance (used for STD)
    deviation: Value,
    /// Final result
    result: Value,
    /// Unique set (used for COLLECT_SET, COUNT DISTINCT, etc.)
    uniques: Option<HashSet<Value>>,
    /// Vector sum (used for VEC_SUM)
    vec_sum: Value,
    /// Vector average (used for VEC_AVG)
    vec_avg: Value,
}

impl AggData {
    /// Create a new aggregated data status.
    pub fn new() -> Self {
        Self {
            cnt: Value::Null(NullType::NaN),
            sum: Value::Null(NullType::NaN),
            avg: Value::Null(NullType::NaN),
            deviation: Value::Null(NullType::NaN),
            result: Value::Null(NullType::NaN),
            uniques: None,
            vec_sum: Value::Null(NullType::NaN),
            vec_avg: Value::Null(NullType::NaN),
        }
    }

    /// Create an aggregated data status with a deduplication function.
    pub fn with_uniques() -> Self {
        Self {
            cnt: Value::Null(NullType::NaN),
            sum: Value::Null(NullType::NaN),
            avg: Value::Null(NullType::NaN),
            deviation: Value::Null(NullType::NaN),
            result: Value::Null(NullType::NaN),
            uniques: Some(HashSet::new()),
            vec_sum: Value::Null(NullType::NaN),
            vec_avg: Value::Null(NullType::NaN),
        }
    }

    /// Obtain the count.
    pub fn cnt(&self) -> &Value {
        &self.cnt
    }

    /// Obtain a variable count
    pub fn cnt_mut(&mut self) -> &mut Value {
        &mut self.cnt
    }

    /// Set the count
    pub fn set_cnt(&mut self, cnt: Value) {
        self.cnt = cnt;
    }

    /// Obtain the cumulative sum.
    pub fn sum(&self) -> &Value {
        &self.sum
    }

    /// Obtain the variable cumulative sum.
    pub fn sum_mut(&mut self) -> &mut Value {
        &mut self.sum
    }

    /// Set the cumulative sum.
    pub fn set_sum(&mut self, sum: Value) {
        self.sum = sum;
    }

    /// Calculate the average value.
    pub fn avg(&self) -> &Value {
        &self.avg
    }

    /// Obtaining the variable average value
    pub fn avg_mut(&mut self) -> &mut Value {
        &mut self.avg
    }

    /// Set the average value
    pub fn set_avg(&mut self, avg: Value) {
        self.avg = avg;
    }

    /// Calculating the variance
    pub fn deviation(&self) -> &Value {
        &self.deviation
    }

    /// Obtaining variable variance
    pub fn deviation_mut(&mut self) -> &mut Value {
        &mut self.deviation
    }

    /// Setting the variance
    pub fn set_deviation(&mut self, deviation: Value) {
        self.deviation = deviation;
    }

    /// Obtain the final result.
    pub fn result(&self) -> &Value {
        &self.result
    }

    /// Obtain a variable final result.
    pub fn result_mut(&mut self) -> &mut Value {
        &mut self.result
    }

    /// Set the final result
    pub fn set_result(&mut self, result: Value) {
        self.result = result;
    }

    /// Obtain a set with no duplicates
    pub fn uniques(&self) -> Option<&HashSet<Value>> {
        self.uniques.as_ref()
    }

    /// Obtain a variable, deduplicated set
    pub fn uniques_mut(&mut self) -> Option<&mut HashSet<Value>> {
        self.uniques.as_mut()
    }

    /// Setting up a deduplication set
    pub fn set_uniques(&mut self, uniques: HashSet<Value>) {
        self.uniques = Some(uniques);
    }

    /// Obtain the vector sum.
    pub fn vec_sum(&self) -> &Value {
        &self.vec_sum
    }

    /// Obtain the variable vector sum.
    pub fn vec_sum_mut(&mut self) -> &mut Value {
        &mut self.vec_sum
    }

    /// Set the vector sum.
    pub fn set_vec_sum(&mut self, vec_sum: Value) {
        self.vec_sum = vec_sum;
    }

    /// Obtain the vector average.
    pub fn vec_avg(&self) -> &Value {
        &self.vec_avg
    }

    /// Obtain the variable vector average.
    pub fn vec_avg_mut(&mut self) -> &mut Value {
        &mut self.vec_avg
    }

    /// Set the vector average.
    pub fn set_vec_avg(&mut self, vec_avg: Value) {
        self.vec_avg = vec_avg;
    }

    /// Check whether it is BadNull.
    pub fn is_bad_null(&self) -> bool {
        self.result.is_bad_null()
    }

    /// Reset the status
    pub fn reset(&mut self) {
        self.cnt = Value::Null(NullType::NaN);
        self.sum = Value::Null(NullType::NaN);
        self.avg = Value::Null(NullType::NaN);
        self.deviation = Value::Null(NullType::NaN);
        self.result = Value::Null(NullType::NaN);
        if let Some(ref mut uniques) = self.uniques {
            uniques.clear();
        }
        self.vec_sum = Value::Null(NullType::NaN);
        self.vec_avg = Value::Null(NullType::NaN);
    }

    /// Obtain variable references to all fields (for use inside aggregate functions)
    ///
    /// Return a variable reference to (result, cnt, sum, avg, deviation)
    pub fn get_all_mut(&mut self) -> (&mut Value, &mut Value, &mut Value, &mut Value, &mut Value) {
        (
            &mut self.result,
            &mut self.cnt,
            &mut self.sum,
            &mut self.avg,
            &mut self.deviation,
        )
    }
}

impl Default for AggData {
    fn default() -> Self {
        Self::new()
    }
}
