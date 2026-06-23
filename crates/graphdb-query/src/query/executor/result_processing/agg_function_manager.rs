//! Aggregation Function Manager Module
//!
//! Refer to the design of AggFunctionManager in nebula-graph.
//! Unified management of built-in aggregate functions, with support for dynamic registration and retrieval.

use super::agg_data::AggData;
use crate::core::error::DBError;
use crate::core::value::list::List;
use crate::core::value::{NullType, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Types of aggregate functions
pub type AggFunction = Arc<dyn Fn(&mut AggData, &Value) -> Result<(), DBError> + Send + Sync>;

/// Aggregate Function Manager
///
/// Manage all aggregate functions and provide a unified interface for retrieval and execution.
#[derive(Clone)]
pub struct AggFunctionManager {
    functions: HashMap<String, AggFunction>,
}

impl AggFunctionManager {
    /// Create a new Aggregate Function Manager and register the built-in functions.
    pub fn new() -> Self {
        let mut manager = Self {
            functions: HashMap::new(),
        };
        manager.register_builtin_functions();
        manager
    }

    /// Obtaining a singleton instance
    pub fn instance() -> Self {
        Self::new()
    }

    /// Registering built-in aggregate functions
    fn register_builtin_functions(&mut self) {
        // COUNT function
        self.functions.insert(
            "COUNT".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = Value::Int(0);
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::Int(n) = res {
                    *res = Value::Int(*n + 1);
                }
                Ok(())
            }),
        );

        // SUM function
        self.functions.insert(
            "SUM".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !val.is_numeric() {
                    *res = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = val.clone();
                } else {
                    match res.add(val) {
                        Ok(new_val) => *res = new_val,
                        Err(_) => *res = Value::Null(NullType::BadType),
                    }
                }
                Ok(())
            }),
        );

        // AVG function
        self.functions.insert(
            "AVG".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                // First, check whether the result is `BadNull`.
                if agg_data.result().is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !val.is_numeric() {
                    *agg_data.result_mut() = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }

                // Initialization
                if agg_data.result().is_null() {
                    *agg_data.result_mut() = Value::Float(0.0);
                    *agg_data.sum_mut() = Value::Float(0.0);
                    *agg_data.cnt_mut() = Value::Float(0.0);
                }

                // Update the sum
                let sum = agg_data.sum_mut();
                match sum.add(val) {
                    Ok(new_sum) => *sum = new_sum,
                    Err(_) => {
                        *agg_data.result_mut() = Value::Null(NullType::BadType);
                        return Ok(());
                    }
                }

                // Update the count
                let cnt = agg_data.cnt_mut();
                if let Value::Float(n) = cnt {
                    *cnt = Value::Float(*n + 1.0);
                }

                // Calculate: avg = sum / count
                let sum = agg_data.sum().clone();
                let cnt = agg_data.cnt().clone();
                if let Value::Float(c) = cnt {
                    if c > 0.0 {
                        match sum.div(&Value::Float(c)) {
                            Ok(avg_val) => *agg_data.result_mut() = avg_val,
                            Err(_) => *agg_data.result_mut() = Value::Null(NullType::DivByZero),
                        }
                    }
                }
                Ok(())
            }),
        );

        // MAX function
        self.functions.insert(
            "MAX".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = val.clone();
                    return Ok(());
                }
                if val > res {
                    *res = val.clone();
                }
                Ok(())
            }),
        );

        // The MIN function
        self.functions.insert(
            "MIN".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = val.clone();
                    return Ok(());
                }
                if val < res {
                    *res = val.clone();
                }
                Ok(())
            }),
        );

        // STD function (Standard Deviation)
        self.functions.insert(
            "STD".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                // First, check whether the result is `BadNull`.
                if agg_data.result().is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !val.is_numeric() {
                    *agg_data.result_mut() = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }

                // Obtain the value
                let val_f64 = match val {
                    Value::SmallInt(v) => *v as f64,
                    Value::Int(v) => *v as f64,
                    Value::BigInt(v) => *v as f64,
                    Value::Float(v) => (*v).into(),
                    Value::Double(v) => *v,
                    _ => return Ok(()),
                };

                // Initialization
                if agg_data.result().is_null() {
                    *agg_data.result_mut() = Value::Double(0.0);
                    *agg_data.cnt_mut() = Value::Double(0.0);
                    *agg_data.avg_mut() = Value::Double(0.0);
                    *agg_data.deviation_mut() = Value::Double(0.0);
                }

                // Get the current value
                let cnt = agg_data.cnt().clone();
                let avg = agg_data.avg().clone();
                let deviation = agg_data.deviation().clone();

                if let (Value::Double(c), Value::Double(a), Value::Double(d)) =
                    (cnt, avg, deviation)
                {
                    let new_cnt = c + 1.0;
                    // The Welford algorithm is used to calculate the standard deviation.
                    let delta = val_f64 - a;
                    let new_avg = a + delta / new_cnt;
                    let delta2 = val_f64 - new_avg;
                    let new_deviation = d + delta * delta2;

                    *agg_data.cnt_mut() = Value::Double(new_cnt);
                    *agg_data.avg_mut() = Value::Double(new_avg);
                    *agg_data.deviation_mut() = Value::Double(new_deviation);

                    if new_cnt >= 2.0 {
                        let variance = new_deviation / (new_cnt - 1.0);
                        *agg_data.result_mut() = Value::Double(variance.sqrt());
                    }
                }
                Ok(())
            }),
        );

        // BIT_AND function
        self.functions.insert(
            "BIT_AND".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !matches!(val, Value::Int(_)) {
                    *res = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::Int(v) = val {
                    if res.is_null() {
                        *res = Value::Int(*v);
                    } else if let Value::Int(r) = res {
                        *res = Value::Int(*r & *v);
                    }
                }
                Ok(())
            }),
        );

        // BIT_OR function
        self.functions.insert(
            "BIT_OR".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !matches!(val, Value::Int(_)) {
                    *res = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::Int(v) = val {
                    if res.is_null() {
                        *res = Value::Int(*v);
                    } else if let Value::Int(r) = res {
                        *res = Value::Int(*r | *v);
                    }
                }
                Ok(())
            }),
        );

        // BIT_XOR function
        self.functions.insert(
            "BIT_XOR".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if !val.is_null() && !val.is_empty() && !matches!(val, Value::Int(_)) {
                    *res = Value::Null(NullType::BadType);
                    return Ok(());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::Int(v) = val {
                    if res.is_null() {
                        *res = Value::Int(*v);
                    } else if let Value::Int(r) = res {
                        *res = Value::Int(*r ^ *v);
                    }
                }
                Ok(())
            }),
        );

        // The COLLECT function
        self.functions.insert(
            "COLLECT".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = Value::list(List::from(Vec::new()));
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::List(ref mut list) = res {
                    list.push(val.clone());
                } else {
                    *res = Value::Null(NullType::BadData);
                }
                Ok(())
            }),
        );

        // The COLLECT_SET function (corresponding to COLLECT_SET in nebula-graph)
        self.functions.insert(
            "COLLECT_SET".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                let res = agg_data.result_mut();
                if res.is_bad_null() {
                    return Ok(());
                }
                if res.is_null() {
                    *res = Value::set(std::collections::HashSet::new());
                }
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                if let Value::Set(ref mut set) = res {
                    set.insert(val.clone());
                } else {
                    *res = Value::Null(NullType::BadData);
                }
                Ok(())
            }),
        );

        // VEC_SUM function - element-wise sum of vectors
        self.functions.insert(
            "VEC_SUM".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                // Skip null or empty values
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                // Check if value is a vector
                if !matches!(val, Value::Vector(_)) {
                    *agg_data.vec_sum_mut() = Value::Null(NullType::BadType);
                    return Ok(());
                }

                // Initialize with first vector
                if agg_data.vec_sum().is_null() {
                    *agg_data.vec_sum_mut() = val.clone();
                } else {
                    // Element-wise addition of vectors
                    let sum_data = agg_data.vec_sum().clone();
                    if let (Value::Vector(sum_vec), Value::Vector(input_vec)) =
                        (sum_data, val.clone())
                    {
                        let sum_data_vec = sum_vec.to_dense();
                        let input_data_vec = input_vec.to_dense();

                        // Check dimension match
                        if sum_data_vec.len() != input_data_vec.len() {
                            *agg_data.vec_sum_mut() = Value::Null(NullType::BadType);
                            return Ok(());
                        }

                        // Element-wise addition
                        let new_data: Vec<f32> = sum_data_vec
                            .iter()
                            .zip(input_data_vec.iter())
                            .map(|(&a, &b)| a + b)
                            .collect();
                        *agg_data.vec_sum_mut() = Value::vector(new_data);
                    } else {
                        *agg_data.vec_sum_mut() = Value::Null(NullType::BadType);
                    }
                }
                Ok(())
            }),
        );

        // VEC_AVG function - element-wise average of vectors
        self.functions.insert(
            "VEC_AVG".to_string(),
            Arc::new(|agg_data: &mut AggData, val: &Value| {
                // Check BadNull
                if agg_data.vec_avg().is_bad_null() {
                    return Ok(());
                }
                // Skip null or empty values
                if val.is_null() || val.is_empty() {
                    return Ok(());
                }
                // Check if value is a vector
                if !matches!(val, Value::Vector(_)) {
                    *agg_data.vec_avg_mut() = Value::Null(NullType::BadType);
                    return Ok(());
                }

                // Get input vector data
                let input_data = if let Value::Vector(v) = val {
                    v.to_dense()
                } else {
                    return Ok(());
                };

                // Initialize
                if agg_data.vec_avg().is_null() {
                    *agg_data.vec_avg_mut() = val.clone();
                    *agg_data.cnt_mut() = Value::Int(1);
                } else {
                    // Update count
                    let cnt = if let Value::Int(c) = agg_data.cnt() {
                        c + 1
                    } else {
                        1
                    };
                    *agg_data.cnt_mut() = Value::Int(cnt);

                    // Element-wise average using Welford-like algorithm
                    let current_avg = agg_data.vec_avg_mut();
                    if let Value::Vector(avg_vec) = current_avg {
                        let avg_data = avg_vec.to_dense();

                        // Check dimension match
                        if avg_data.len() != input_data.len() {
                            *agg_data.vec_avg_mut() = Value::Null(NullType::BadType);
                            return Ok(());
                        }

                        // Update average incrementally
                        let new_avg: Vec<f32> = avg_data
                            .iter()
                            .zip(input_data.iter())
                            .map(|(&avg, &input)| avg + (input - avg) / cnt as f32)
                            .collect();
                        *agg_data.vec_avg_mut() = Value::vector(new_avg);
                    }
                }
                Ok(())
            }),
        );
    }

    /// Obtaining aggregate functions
    pub fn get(&self, name: &str) -> Option<AggFunction> {
        self.functions.get(&name.to_uppercase()).cloned()
    }

    /// Check whether aggregate functions exist.
    pub fn find(&self, name: &str) -> bool {
        self.functions.contains_key(&name.to_uppercase())
    }

    /// Registering custom aggregate functions
    pub fn register(&mut self, name: &str, func: AggFunction) -> Result<(), DBError> {
        let upper_name = name.to_uppercase();
        if self.functions.contains_key(&upper_name) {
            return Err(DBError::query(format!(
                "The aggregate function '{}' already exists.",
                name
            )));
        }
        self.functions.insert(upper_name, func);
        Ok(())
    }
}

impl Default for AggFunctionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_function() {
        let manager = AggFunctionManager::new();
        let count_func = manager
            .get("COUNT")
            .expect("The COUNT function should exist");

        let mut agg_data = AggData::new();

        // Testing for null values: The COUNT function should be initialized to 0.
        count_func(&mut agg_data, &Value::Null(NullType::Null))
            .expect("The COUNT function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(0));

        // Normal test values
        count_func(&mut agg_data, &Value::Int(1)).expect("The COUNT function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(1));

        count_func(&mut agg_data, &Value::Int(2)).expect("The COUNT function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(2));

        // Testing NULL cases is not counted.
        count_func(&mut agg_data, &Value::Null(NullType::Null))
            .expect("The COUNT function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(2));
    }

    #[test]
    fn test_sum_function() {
        let manager = AggFunctionManager::new();
        let sum_func = manager.get("SUM").expect("The SUM function should exist");

        let mut agg_data = AggData::new();

        sum_func(&mut agg_data, &Value::Int(10)).expect("The SUM function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(10));

        sum_func(&mut agg_data, &Value::Int(20)).expect("The SUM function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(30));

        // Test: NULL should not be included.
        sum_func(&mut agg_data, &Value::Null(NullType::Null))
            .expect("The SUM function call should succeed");
        assert_eq!(agg_data.result(), &Value::Int(30));
    }

    #[test]
    fn test_collect_set_function() {
        let manager = AggFunctionManager::new();
        let collect_set_func = manager
            .get("COLLECT_SET")
            .expect("The COLLECT_SET function should exist");

        let mut agg_data = AggData::new();

        collect_set_func(&mut agg_data, &Value::Int(1))
            .expect("COLLECT_SET function call should succeed");
        collect_set_func(&mut agg_data, &Value::Int(2))
            .expect("COLLECT_SET function call should succeed");
        collect_set_func(&mut agg_data, &Value::Int(1))
            .expect("COLLECT_SET function call should succeed");

        if let Value::Set(set) = agg_data.result() {
            assert_eq!(set.len(), 2);
        } else {
            panic!("The result should be of the Set type.");
        }
    }
}
