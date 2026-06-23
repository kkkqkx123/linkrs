//! Implementation of aggregate functions

use crate::core::types::operators::AggregateFunction;
use crate::core::value::list::List;
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;
use serde::{Deserialize, Serialize};

/// Create an AggregateFunction from a string name
pub fn aggregate_from_str(func_name: &str) -> Result<AggregateFunction, ExpressionError> {
    match func_name.to_uppercase().as_str() {
        "COUNT" => Ok(AggregateFunction::Count(None)),
        "COUNT_DISTINCT" => Ok(AggregateFunction::Distinct("".to_string())),
        "SUM" => Ok(AggregateFunction::Sum("".to_string())),
        "AVG" => Ok(AggregateFunction::Avg("".to_string())),
        "MIN" => Ok(AggregateFunction::Min("".to_string())),
        "MAX" => Ok(AggregateFunction::Max("".to_string())),
        "COLLECT" => Ok(AggregateFunction::Collect("".to_string())),
        "DISTINCT" => Ok(AggregateFunction::Distinct("".to_string())),
        "PERCENTILE" => Ok(AggregateFunction::Percentile("".to_string(), 50.0)),
        _ => Err(ExpressionError::function_error(format!(
            "Unknown aggregate function: {}",
            func_name
        ))),
    }
}

/// Creating aggregate functions from strings and parameters
pub fn aggregate_from_str_with_args(
    func_name: &str,
    args: &[String],
) -> Result<AggregateFunction, ExpressionError> {
    match func_name.to_uppercase().as_str() {
        "COUNT" => {
            if args.is_empty() {
                Ok(AggregateFunction::Count(None))
            } else {
                Ok(AggregateFunction::Count(Some(args[0].clone())))
            }
        }
        "SUM" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "SUM function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Sum(args[0].clone()))
        }
        "AVG" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "AVG function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Avg(args[0].clone()))
        }
        "MIN" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "MIN function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Min(args[0].clone()))
        }
        "MAX" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "MAX function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Max(args[0].clone()))
        }
        "COLLECT" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "COLLECT function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Collect(args[0].clone()))
        }
        "DISTINCT" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "DISTINCT function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::Distinct(args[0].clone()))
        }
        "PERCENTILE" => {
            if args.len() < 2 {
                return Err(ExpressionError::function_error(
                    "PERCENTILE function requires a field name and percentile value".to_string(),
                ));
            }
            let percentile = args[1].parse::<f64>().map_err(|_| {
                ExpressionError::function_error("Invalid percentile value".to_string())
            })?;
            Ok(AggregateFunction::Percentile(args[0].clone(), percentile))
        }
        "VEC_SUM" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "VEC_SUM function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::VecSum(args[0].clone()))
        }
        "VEC_AVG" => {
            if args.is_empty() {
                return Err(ExpressionError::function_error(
                    "VEC_AVG function requires a field name".to_string(),
                ));
            }
            Ok(AggregateFunction::VecAvg(args[0].clone()))
        }
        _ => Err(ExpressionError::function_error(format!(
            "Unknown aggregate function: {}",
            func_name
        ))),
    }
}

/// Aggregate expressions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateExpression {
    pub function: AggregateFunction,
    pub argument: Box<Expression>,
    pub distinct: bool,
}

impl AggregateExpression {
    pub fn new(function: AggregateFunction, argument: Expression, distinct: bool) -> Self {
        Self {
            function,
            argument: Box::new(argument),
            distinct,
        }
    }

    /// Calculate the value of the aggregate expression.
    pub fn evaluate<C: crate::query::executor::expression::ExpressionContext>(
        &self,
        context: &mut C,
        state: &mut AggregateState,
    ) -> Result<Value, ExpressionError> {
        let arg_value =
            crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator::evaluate(
                &self.argument,
                context,
            )
            .map_err(|e| ExpressionError::function_error(e.to_string()))?;

        // Update the aggregation status.
        state.update(&self.function, &arg_value, self.distinct);

        // Return the aggregated results of the current state.
        match &self.function {
            AggregateFunction::Count(_) => Ok(Value::BigInt(state.count)),
            AggregateFunction::Sum(_) => Ok(state.sum.clone()),
            AggregateFunction::Min(_) => Ok(state
                .min
                .clone()
                .unwrap_or(Value::Null(crate::core::value::NullType::Null))),
            AggregateFunction::Max(_) => Ok(state
                .max
                .clone()
                .unwrap_or(Value::Null(crate::core::value::NullType::Null))),
            AggregateFunction::Avg(_) => {
                if state.count > 0 {
                    match &state.sum {
                        Value::SmallInt(i) => Ok(Value::Double(*i as f64 / state.count as f64)),
                        Value::Int(i) => Ok(Value::Double(*i as f64 / state.count as f64)),
                        Value::BigInt(i) => Ok(Value::Double(*i as f64 / state.count as f64)),
                        Value::Float(f) => Ok(Value::Double(*f as f64 / state.count as f64)),
                        Value::Double(f) => Ok(Value::Double(*f / state.count as f64)),
                        _ => Ok(Value::Double(0.0)),
                    }
                } else {
                    Ok(Value::Double(0.0))
                }
            }
            AggregateFunction::Collect(_) => Ok(Value::list(List::from(state.values.clone()))),
            AggregateFunction::CollectSet(_) => Ok(Value::set(
                state
                    .values
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>(),
            )),
            AggregateFunction::Distinct(_) => Ok(Value::set(
                state
                    .values
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>(),
            )),
            AggregateFunction::Percentile(_, _) => state.calculate_percentile(50.0),
            AggregateFunction::Std(_) => state.calculate_std(),
            AggregateFunction::BitAnd(_) => state.calculate_bit_and(),
            AggregateFunction::BitOr(_) => state.calculate_bit_or(),
            AggregateFunction::GroupConcat(_, _) => state.calculate_group_concat(),
            AggregateFunction::VecSum(_) => Ok(state.vec_sum.clone()),
            AggregateFunction::VecAvg(_) => {
                if state.count > 0 {
                    Ok(state.vec_avg.clone())
                } else {
                    Ok(Value::Null(crate::core::value::NullType::NaN))
                }
            }
        }
    }
}

/// Aggregation status, used to accumulate the intermediate results of aggregate functions.
#[derive(Debug, Clone)]
pub struct AggregateState {
    pub count: i64,
    pub sum: Value,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub values: Vec<Value>,
    pub distinct_values: std::collections::HashSet<String>,
    pub percentile_values: Vec<f64>,
    pub std_values: Vec<f64>,
    pub bit_and_value: Option<i64>,
    pub bit_or_value: Option<i64>,
    pub group_concat_values: Vec<Value>,
    /// Vector sum for VEC_SUM
    pub vec_sum: Value,
    /// Vector average for VEC_AVG
    pub vec_avg: Value,
}

impl Default for AggregateState {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateState {
    pub fn new() -> Self {
        Self {
            count: 0,
            sum: Value::Int(0),
            min: None,
            max: None,
            values: Vec::new(),
            distinct_values: std::collections::HashSet::new(),
            percentile_values: Vec::new(),
            std_values: Vec::new(),
            bit_and_value: None,
            bit_or_value: None,
            group_concat_values: Vec::new(),
            vec_sum: Value::Null(crate::core::value::NullType::NaN),
            vec_avg: Value::Null(crate::core::value::NullType::NaN),
        }
    }

    pub fn reset(&mut self) {
        self.count = 0;
        self.sum = Value::Int(0);
        self.min = None;
        self.max = None;
        self.values.clear();
        self.distinct_values.clear();
        self.percentile_values.clear();
        self.std_values.clear();
        self.bit_and_value = None;
        self.bit_or_value = None;
        self.group_concat_values.clear();
        self.vec_sum = Value::Null(crate::core::value::NullType::NaN);
        self.vec_avg = Value::Null(crate::core::value::NullType::NaN);
    }

    /// Update the aggregation status.
    pub fn update(&mut self, function: &AggregateFunction, value: &Value, distinct: bool) {
        let value_str = format!("{}", value);

        // If `distinct` is enabled, check whether it already exists.
        if distinct && self.distinct_values.contains(&value_str) {
            return;
        }

        // The recorded values are used for deduplication (i.e., to remove duplicate entries).
        if distinct {
            self.distinct_values.insert(value_str);
        }

        self.count += 1;
        self.values.push(value.clone());

        // Special processing is performed depending on the type of aggregate function.
        match function {
            AggregateFunction::Percentile(_, _) => {
                // Special handling of the PERCENTILE function: Collecting numerical values
                match value {
                    Value::SmallInt(v) => self.percentile_values.push(*v as f64),
                    Value::Int(v) => self.percentile_values.push(*v as f64),
                    Value::BigInt(v) => self.percentile_values.push(*v as f64),
                    Value::Float(v) => self.percentile_values.push(*v as f64),
                    Value::Double(v) => self.percentile_values.push(*v),
                    _ => {}
                }
            }
            AggregateFunction::Std(_) => {
                // Special handling of the STD function: Collecting numerical values
                match value {
                    Value::SmallInt(v) => self.std_values.push(*v as f64),
                    Value::Int(v) => self.std_values.push(*v as f64),
                    Value::BigInt(v) => self.std_values.push(*v as f64),
                    Value::Float(v) => self.std_values.push(*v as f64),
                    Value::Double(v) => self.std_values.push(*v),
                    _ => {}
                }
            }
            AggregateFunction::BitAnd(_) => {
                // Special handling of the BIT_AND function
                if let Value::BigInt(v) = value {
                    if let Some(current) = self.bit_and_value {
                        self.bit_and_value = Some(current & v);
                    } else {
                        self.bit_and_value = Some(*v);
                    }
                }
            }
            AggregateFunction::BitOr(_) => {
                // Special handling of the BIT_OR function
                if let Value::BigInt(v) = value {
                    if let Some(current) = self.bit_or_value {
                        self.bit_or_value = Some(current | v);
                    } else {
                        self.bit_or_value = Some(*v);
                    }
                }
            }
            AggregateFunction::GroupConcat(_, _) => {
                // Special handling of the GROUP_CONCAT function
                self.group_concat_values.push(value.clone());
            }
            AggregateFunction::VecSum(_) => {
                // Special handling for VEC_SUM function
                if matches!(value, Value::Vector(_)) {
                    if self.vec_sum.is_null() {
                        self.vec_sum = value.clone();
                    } else if let (Value::Vector(sum_vec), Value::Vector(input_vec)) =
                        (&mut self.vec_sum, value)
                    {
                        let sum_data = sum_vec.to_dense();
                        let input_data = input_vec.to_dense();

                        if sum_data.len() == input_data.len() {
                            let new_data: Vec<f32> = sum_data
                                .iter()
                                .zip(input_data.iter())
                                .map(|(&a, &b)| a + b)
                                .collect();
                            self.vec_sum = Value::vector(new_data);
                        }
                    }
                }
            }
            AggregateFunction::VecAvg(_) => {
                // Special handling for VEC_AVG function
                if matches!(value, Value::Vector(_)) {
                    if self.vec_avg.is_null() {
                        self.vec_avg = value.clone();
                    } else if let (Value::Vector(avg_vec), Value::Vector(input_vec)) =
                        (&mut self.vec_avg, value)
                    {
                        let avg_data = avg_vec.to_dense();
                        let input_data = input_vec.to_dense();

                        if avg_data.len() == input_data.len() {
                            // Incremental average calculation
                            let new_avg: Vec<f32> = avg_data
                                .iter()
                                .zip(input_data.iter())
                                .map(|(&avg, &input)| avg + (input - avg) / self.count as f32)
                                .collect();
                            self.vec_avg = Value::vector(new_avg);
                        }
                    }
                }
            }
            _ => {
                // General handling of other aggregate functions
                // Update the minimum value
                if self.min.as_ref().is_none_or(|min_val| value < min_val) {
                    self.min = Some(value.clone());
                }

                // Update the maximum value
                if self.max.as_ref().is_none_or(|max_val| value > max_val) {
                    self.max = Some(value.clone());
                }

                // Update Total
                match (&mut self.sum, value) {
                    (Value::SmallInt(ref mut sum_int), Value::SmallInt(val_int)) => {
                        *sum_int += *val_int;
                    }
                    (Value::Int(ref mut sum_int), Value::Int(val_int)) => {
                        *sum_int += *val_int;
                    }
                    (Value::BigInt(ref mut sum_int), Value::BigInt(val_int)) => {
                        *sum_int += *val_int;
                    }
                    (Value::Float(ref mut sum_float), Value::Float(val_float)) => {
                        *sum_float += *val_float;
                    }
                    (Value::Double(ref mut sum_float), Value::Double(val_float)) => {
                        *sum_float += *val_float;
                    }
                    (Value::SmallInt(ref mut sum_int), Value::Int(val_int)) => {
                        self.sum = Value::Int(*sum_int as i32 + *val_int);
                    }
                    (Value::Int(ref mut sum_int), Value::SmallInt(val_int)) => {
                        *sum_int += *val_int as i32;
                    }
                    (Value::Int(ref mut sum_int), Value::BigInt(val_int)) => {
                        self.sum = Value::BigInt(*sum_int as i64 + *val_int);
                    }
                    (Value::BigInt(ref mut sum_int), Value::Int(val_int)) => {
                        *sum_int += *val_int as i64;
                    }
                    (Value::Float(ref mut sum_float), Value::Double(val_float)) => {
                        self.sum = Value::Double(*sum_float as f64 + *val_float);
                    }
                    (Value::Double(ref mut sum_float), Value::Float(val_float)) => {
                        *sum_float += *val_float as f64;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Calculating percentiles
    pub fn calculate_percentile(&self, percentile: f64) -> Result<Value, ExpressionError> {
        if self.percentile_values.is_empty() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        if !(0.0..=100.0).contains(&percentile) {
            return Err(ExpressionError::function_error(
                "Percentile must be between 0 and 100".to_string(),
            ));
        }

        let mut sorted_values = self.percentile_values.clone();
        sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let index = (percentile / 100.0) * (sorted_values.len() - 1) as f64;
        let lower_index = index.floor() as usize;
        let upper_index = index.ceil() as usize;

        if lower_index == upper_index {
            Ok(Value::Double(sorted_values[lower_index]))
        } else {
            let lower_value = sorted_values[lower_index];
            let upper_value = sorted_values[upper_index];
            let weight = index - lower_index as f64;
            let interpolated = lower_value + weight * (upper_value - lower_value);
            Ok(Value::Double(interpolated))
        }
    }

    /// Calculate the standard deviation
    pub fn calculate_std(&self) -> Result<Value, ExpressionError> {
        if self.std_values.is_empty() {
            return Ok(Value::Null(crate::core::value::NullType::Null));
        }

        let n = self.std_values.len() as f64;
        let mean: f64 = self.std_values.iter().sum::<f64>() / n;
        let variance: f64 = self
            .std_values
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / n;
        let std_dev = variance.sqrt();

        Ok(Value::Double(std_dev))
    }

    /// Performing a bitwise AND operation
    pub fn calculate_bit_and(&self) -> Result<Value, ExpressionError> {
        if let Some(value) = self.bit_and_value {
            Ok(Value::BigInt(value))
        } else {
            Ok(Value::Null(crate::core::value::NullType::Null))
        }
    }

    /// Performing a bitwise OR operation
    pub fn calculate_bit_or(&self) -> Result<Value, ExpressionError> {
        if let Some(value) = self.bit_or_value {
            Ok(Value::BigInt(value))
        } else {
            Ok(Value::Null(crate::core::value::NullType::Null))
        }
    }

    /// Computing group joins
    pub fn calculate_group_concat(&self) -> Result<Value, ExpressionError> {
        if self.group_concat_values.is_empty() {
            return Ok(Value::String(String::new()));
        }

        let result: Vec<String> = self
            .group_concat_values
            .iter()
            .map(|v| format!("{}", v))
            .collect();
        Ok(Value::String(result.join(",")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unified_aggregate_function() {
        // The test involves creating objects from strings.
        let func = aggregate_from_str("COUNT").expect("from_str should succeed");
        assert!(matches!(func, AggregateFunction::Count(_)));

        let func = aggregate_from_str("SUM").expect("from_str should succeed");
        assert!(matches!(func, AggregateFunction::Sum(_)));

        let sum_func = aggregate_from_str_with_args("SUM", &["field".to_string()])
            .expect("from_str_with_args should succeed");
        assert!(sum_func.is_numeric());
        assert!(!sum_func.is_collection());

        let collect_func = aggregate_from_str_with_args("COLLECT", &["field".to_string()])
            .expect("from_str_with_args should succeed");
        assert!(!collect_func.is_numeric());
        assert!(collect_func.is_collection());
    }
}
