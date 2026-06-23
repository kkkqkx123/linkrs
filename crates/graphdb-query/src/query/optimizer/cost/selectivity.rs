//! Selective Estimator Module
//!
//! Used to estimate the selectivity of the query conditions

use std::sync::Arc;

use crate::core::types::BinaryOperator;
use crate::core::types::Expression;
use crate::core::value::Value;
use crate::query::optimizer::stats::{RangeCondition, StatisticsManager};

/// Selective Estimator
///
/// Provide selective estimates based on statistical information and heuristic rules.
#[derive(Debug)]
pub struct SelectivityEstimator {
    stats_manager: Arc<StatisticsManager>,
}

/// Default selective constants
pub mod defaults {
    /// Default selectivity for equivalent queries (assuming 10 different values)
    pub const EQUALITY: f64 = 0.1;
    /// The default selectivity for range queries is such that approximately one-third of the data is selected.
    pub const RANGE: f64 = 0.333;
    /// The default selectivity for the “less than/greater than” query
    pub const COMPARISON: f64 = 0.333;
    /// The default selectivity of inequality queries
    pub const NOT_EQUAL: f64 = 0.9;
    /// The selectivity of IS NULL queries (which usually rarely return a value of NULL)
    pub const IS_NULL: f64 = 0.05;
    /// The selectivity of the IS NOT NULL query
    pub const IS_NOT_NULL: f64 = 0.95;
    /// The default selectivity of an IN query (assuming 3 values)
    pub const IN_LIST: f64 = 0.3;
    /// The SELECTIVE nature of the EXISTS query
    pub const EXISTS: f64 = 0.5;
    /// The selective penalty of the Boolean AND operation
    pub const AND_CORRELATION: f64 = 0.9;
    /// The selective penalty for the Boolean OR operation
    pub const OR_CORRELATION: f64 = 0.9;
}

impl SelectivityEstimator {
    /// Create a new selective estimator.
    pub fn new(stats_manager: Arc<StatisticsManager>) -> Self {
        Self { stats_manager }
    }

    /// Estimated Equivalence Condition Selectivity
    ///
    /// If there is histogram statistical information, use the histogram for an accurate estimation.
    /// Otherwise, if basic statistical information is available, use 1 divided by the number of distinct values.
    /// Otherwise the default value of 0.1 is used
    pub fn estimate_equality_selectivity(
        &self,
        tag_name: Option<&str>,
        property_name: &str,
        value: Option<&Value>,
    ) -> f64 {
        let stats = self
            .stats_manager
            .get_property_stats(tag_name, property_name);

        match stats {
            Some(s) => {
                // Give priority to using histograms for accurate estimates.
                if s.should_use_histogram() {
                    if let Some(ref histogram) = s.histogram {
                        if let Some(v) = value {
                            return histogram.estimate_equality_selectivity(v);
                        }
                    }
                }

                // Return to basic statistical information
                if s.distinct_values > 0 {
                    (1.0 / s.distinct_values as f64).min(1.0)
                } else {
                    defaults::EQUALITY
                }
            }
            _ => defaults::EQUALITY,
        }
    }

    /// Estimated Equivalence Condition Selectivity (simplified version, without values)
    pub fn estimate_equality_selectivity_simple(
        &self,
        tag_name: Option<&str>,
        property_name: &str,
    ) -> f64 {
        self.estimate_equality_selectivity(tag_name, property_name, None)
    }

    /// Conditional selectivity of the estimation range
    ///
    /// If there is histogram statistical information, the calculations are based on that histogram.
    /// Otherwise, use the default value of 1/3.
    pub fn estimate_range_selectivity(
        &self,
        tag_name: Option<&str>,
        property_name: &str,
        range: &RangeCondition,
    ) -> f64 {
        let stats = self
            .stats_manager
            .get_property_stats(tag_name, property_name);

        match stats {
            Some(s) => {
                // Give priority to using histograms for accurate estimations.
                if s.should_use_histogram() {
                    if let Some(ref histogram) = s.histogram {
                        return histogram.estimate_range_selectivity(range);
                    }
                }

                // Revert to the default values.
                defaults::RANGE
            }
            _ => defaults::RANGE,
        }
    }

    /// Estimated Range Condition Selectivity (simplified version)
    pub fn estimate_range_selectivity_simple(&self) -> f64 {
        defaults::RANGE
    }

    /// Conditional Selectivity of Estimation Ranges (with Boundary Values)
    ///
    /// Adjust the selectivity based on the size of the scope.
    pub fn estimate_range_selectivity_with_bounds(
        &self,
        min_val: f64,
        max_val: f64,
        range_size: f64,
    ) -> f64 {
        if max_val <= min_val {
            return defaults::RANGE;
        }
        let total_range = max_val - min_val;
        let selectivity = (range_size / total_range).clamp(0.001, 1.0);
        // Range queries usually do not retrieve a large amount of data; adding an upper limit helps to control the amount of data retrieved.
        selectivity.min(0.8)
    }

    /// Estimated to be less than conditional selectivity
    ///
    /// If there is statistical information available, the calculations are based on histograms.
    /// Otherwise, assume that the data is evenly distributed, and return 1/3.
    pub fn estimate_less_than_selectivity(&self, value: Option<f64>) -> f64 {
        // If there are specific values, you can try to adjust the settings based on the distribution of those values.
        // A simple heuristic is used here: assume that the data is evenly distributed.
        match value {
            Some(v) if v < 0.0 => 0.1, // Negative values are generally less common.
            Some(0.0) => 0.05,         // Zero values are usually very rare.
            _ => defaults::COMPARISON,
        }
    }

    /// The estimate is greater than the condition selectivity.
    pub fn estimate_greater_than_selectivity(&self, value: Option<f64>) -> f64 {
        match value {
            Some(v) if v < 0.0 => 0.9, // When comparing values, it is generally preferable to choose the option that represents the majority of the data. In this case, since “greater than a negative value” indicates a positive value, that option would correspond to the majority of the data points in the dataset.
            Some(0.0) => 0.95, // When the value is greater than zero, it usually indicates that the majority of the data falls into that category.
            _ => defaults::COMPARISON,
        }
    }

    /// Estimating the selectivity of LIKE conditions
    ///
    /// Adjust the selectivity based on the prefix and suffix wildcards of the pattern:
    /// - prefix%: high selectivity (about 0.1)
    /// - %suffix: medium selectivity (about 0.2)
    /// - %substring%: less selective (about 0.5)
    /// - No wildcard: exact match (about 0.05)
    pub fn estimate_like_selectivity(&self, pattern: &str) -> f64 {
        let has_prefix = pattern.starts_with('%');
        let has_suffix = pattern.ends_with('%');
        let middle_wildcards = pattern.matches('%').count() + pattern.matches('_').count();

        match (has_prefix, has_suffix) {
            (true, true) => {
                // The selectivity of the %xxx% mode is very low.
                0.5_f64.min(0.1 + middle_wildcards as f64 * 0.1)
            }
            (false, true) => {
                // The prefix matching with a percentage of xxx% has a relatively high degree of selectivity.
                0.1_f64.min(0.05 + middle_wildcards as f64 * 0.02)
            }
            (true, false) => {
                // The matching accuracy for suffixes with the “%xxx” pattern is moderately high.
                0.2_f64.min(0.1 + middle_wildcards as f64 * 0.05)
            }
            (false, false) => {
                // No wildcards; the match is nearly exact.
                0.05
            }
        }
    }

    /// Estimating the selectivity of the IN list
    ///
    /// Assuming that the selectivity of each value is the same, the total selectivity = number of values * selectivity of a single value.
    pub fn estimate_in_selectivity(&self, list_size: usize) -> f64 {
        let single_selectivity = defaults::EQUALITY;
        (list_size as f64 * single_selectivity).min(0.9)
    }

    /// Estimated IS NULL selectivity
    pub fn estimate_is_null_selectivity(&self) -> f64 {
        defaults::IS_NULL
    }

    /// The “IS NOT NULL” condition is used for selection purposes.
    pub fn estimate_is_not_null_selectivity(&self) -> f64 {
        defaults::IS_NOT_NULL
    }

    /// Estimated NOT conditional selectivity
    ///
    /// The selectivity of the NOT condition = 1 – the selectivity of the original condition
    pub fn estimate_not_selectivity(&self, inner_selectivity: f64) -> f64 {
        (1.0 - inner_selectivity).clamp(0.01, 0.99)
    }

    /// Estimating selectivity from expressions
    ///
    /// This is the main entry method, which distributes the data to the specific estimation methods based on the type of the expression.
    pub fn estimate_from_expression(&self, expr: &Expression, tag_name: Option<&str>) -> f64 {
        match expr {
            Expression::Binary { op, left, right } => {
                self.estimate_binary_expression(op, left, right, tag_name)
            }
            Expression::Unary { op, operand } => {
                self.estimate_unary_expression(op, operand, tag_name)
            }
            Expression::Function { name, args } => self.estimate_function_expression(name, args),
            Expression::Literal(_) => {
                // The selectivity of literal value conditions depends on the specific values; such conditions are generally considered to be highly selective.
                0.1
            }
            Expression::Property { .. } => {
                // attribute itself as a condition (e.g. WHERE n.active)
                // Assume that approximately half of the boolean attributes have the value “true”.
                0.5
            }
            _ => defaults::EQUALITY,
        }
    }

    /// Estimating the selectivity of binary expressions
    fn estimate_binary_expression(
        &self,
        op: &BinaryOperator,
        left: &Expression,
        right: &Expression,
        tag_name: Option<&str>,
    ) -> f64 {
        match op {
            BinaryOperator::Equal => {
                // Try to extract the attribute names and values from the expression.
                let property_name = self
                    .extract_property_name(left)
                    .or_else(|| self.extract_property_name(right));

                // Try to extract the value.
                let value = self
                    .extract_value(right)
                    .or_else(|| self.extract_value(left));

                if let Some(prop) = property_name {
                    self.estimate_equality_selectivity(tag_name, &prop, value.as_ref())
                } else {
                    defaults::EQUALITY
                }
            }
            BinaryOperator::NotEqual => {
                // Inequality queries usually select the majority of the data.
                defaults::NOT_EQUAL
            }
            BinaryOperator::LessThan => {
                let value = self.extract_numeric_value(right);
                self.estimate_less_than_selectivity(value)
            }
            BinaryOperator::LessThanOrEqual => {
                let value = self.extract_numeric_value(right);
                self.estimate_less_than_selectivity(value).clamp(0.01, 0.9)
            }
            BinaryOperator::GreaterThan => {
                let value = self.extract_numeric_value(right);
                self.estimate_greater_than_selectivity(value)
            }
            BinaryOperator::GreaterThanOrEqual => {
                let value = self.extract_numeric_value(right);
                self.estimate_greater_than_selectivity(value)
                    .clamp(0.01, 0.9)
            }
            BinaryOperator::And => {
                let left_sel = self.estimate_from_expression(left, tag_name);
                let right_sel = self.estimate_from_expression(right, tag_name);
                // The selectivity of the “AND” operator is usually slightly higher than that of the multiplication operator (because there may be correlations between the conditions).
                (left_sel * right_sel / defaults::AND_CORRELATION).min(1.0)
            }
            BinaryOperator::Or => {
                let left_sel = self.estimate_from_expression(left, tag_name);
                let right_sel = self.estimate_from_expression(right, tag_name);
                // OR 的选择性：P(A or B) = P(A) + P(B) - P(A and B)
                let combined =
                    left_sel + right_sel - left_sel * right_sel * defaults::OR_CORRELATION;
                combined.clamp(0.01, 0.99)
            }
            BinaryOperator::In => {
                // Estimating the size of the IN list
                let list_size = self.estimate_list_size(right);
                self.estimate_in_selectivity(list_size)
            }
            _ => defaults::EQUALITY,
        }
    }

    /// Estimating the selectivity of a unary expression
    fn estimate_unary_expression(
        &self,
        op: &crate::core::types::UnaryOperator,
        expr: &Expression,
        tag_name: Option<&str>,
    ) -> f64 {
        use crate::core::types::UnaryOperator;

        match op {
            UnaryOperator::Not => {
                let inner = self.estimate_from_expression(expr, tag_name);
                self.estimate_not_selectivity(inner)
            }
            UnaryOperator::IsNull => defaults::IS_NULL,
            UnaryOperator::IsNotNull => defaults::IS_NOT_NULL,
            _ => defaults::EQUALITY,
        }
    }

    /// Estimating the selectivity of function expressions
    fn estimate_function_expression(&self, name: &str, args: &[Expression]) -> f64 {
        let name_lower = name.to_lowercase();

        match name_lower.as_str() {
            "like" | "ilike" if args.len() >= 2 => {
                // Try to extract the LIKE pattern.
                if let Expression::Literal(crate::core::value::Value::String(pattern)) = &args[1] {
                    return self.estimate_like_selectivity(pattern);
                }
                defaults::EQUALITY
            }
            "exists" => defaults::EXISTS,
            "contains" | "has" => 0.2, // The content to be translated usually has a high degree of selectivity (i.e., only certain parts of the text are to be translated).
            "starts_with" => 0.1,      // Prefix matching
            "ends_with" => 0.2,        // Suffix matching
            "in" => {
                let list_size = args.len().saturating_sub(1);
                self.estimate_in_selectivity(list_size)
            }
            _ => defaults::EQUALITY,
        }
    }

    /// Extract the attribute names from the expression.
    fn extract_property_name(&self, expr: &Expression) -> Option<String> {
        match expr {
            Expression::Property { property, .. } => Some(property.clone()),
            _ => None,
        }
    }

    /// Extract the numerical values from the expression.
    fn extract_numeric_value(&self, expr: &Expression) -> Option<f64> {
        match expr {
            Expression::Literal(value) => match value {
                crate::core::value::Value::SmallInt(i) => Some(*i as f64),
                crate::core::value::Value::Int(i) => Some(*i as f64),
                crate::core::value::Value::BigInt(i) => Some(*i as f64),
                crate::core::value::Value::Float(f) => Some((*f).into()),
                crate::core::value::Value::Double(f) => Some(*f),
                _ => None,
            },
            _ => None,
        }
    }

    /// Extract values from the expression.
    fn extract_value(&self, expr: &Expression) -> Option<Value> {
        match expr {
            Expression::Literal(value) => Some(value.clone()),
            _ => None,
        }
    }

    /// Estimate the list size
    fn estimate_list_size(&self, expr: &Expression) -> usize {
        match expr {
            Expression::List(items) => items.len(),
            _ => 3, // The default assumption is that there are 3 elements.
        }
    }
}

impl Clone for SelectivityEstimator {
    fn clone(&self) -> Self {
        Self {
            stats_manager: self.stats_manager.clone(),
        }
    }
}
