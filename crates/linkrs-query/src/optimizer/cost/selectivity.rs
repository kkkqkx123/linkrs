//! Selective Estimator Module
//!
//! Used to estimate the selectivity of the query conditions

use std::sync::Arc;

use crate::optimizer::stats::feedback::selectivity::{
    SelectivityFeedbackManager, SelectivitySnapshot,
};
use crate::optimizer::stats::{RangeCondition, StatisticsManager};
use linkrs_core::types::BinaryOperator;
use linkrs_core::types::Expression;
use linkrs_core::value::Value;

/// Build the normalized condition key for a predicate expression.
///
/// The key is used by the statistics feedback loop to attach
/// estimated-vs-actual row ratios to a specific predicate. Literals and
/// numbers are replaced with placeholders so that structurally identical
/// predicates with different constants aggregate to the same key. When a
/// space is present the key is prefixed with `"{space}:"` so corrections are
/// scoped to a space and can be invalidated per space.
///
/// Ordered range comparisons additionally carry a coarse bound bucket
/// (`~8e2`): selectivity is a function of the bound, so `value < 800` must
/// not share corrections with `value < 10`, while nearby bounds
/// (`value < 801`) still aggregate. Equality and other shapes keep the
/// purely structural key.
pub fn condition_key(space: Option<&str>, expr: &Expression) -> String {
    let normalized =
        crate::optimizer::stats::feedback::fingerprint::normalize_query(&format!("{:?}", expr));
    let key = match space {
        Some(space) => format!("{}:{}", space, normalized),
        None => normalized,
    };
    match range_bound_bucket(expr) {
        Some(bucket) => format!("{key}~{bucket}"),
        None => key,
    }
}

/// Coarse bound bucket for ordered property-vs-literal comparisons.
///
/// Quantizes the literal to one significant digit so structurally identical
/// range predicates with very different bounds learn separately while
/// nearby bounds still share. Returns `None` for non-ordered shapes
/// (equality keeps the structural key) and non-numeric bounds.
fn range_bound_bucket(expr: &Expression) -> Option<String> {
    let Expression::Binary { op, left, right } = expr else {
        return None;
    };
    if !matches!(
        op,
        BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
    ) {
        return None;
    }
    let bound = [left.as_ref(), right.as_ref()]
        .into_iter()
        .find_map(|side| match side {
            Expression::Literal(value) => numeric_as_f64(value),
            _ => None,
        })?;
    let other_is_property = [left.as_ref(), right.as_ref()].iter().any(|side| {
        matches!(
            side,
            Expression::Property { .. }
                | Expression::TagProperty { .. }
                | Expression::EdgeProperty { .. }
                | Expression::LabelTagProperty { .. }
        )
    });
    if !other_is_property {
        return None;
    }
    Some(format!("bound{}", quantize_bound(bound)))
}

/// Quantize a bound to one significant digit (`800` and `801` share
/// `8e2`; `10` maps to `1e1`).
fn quantize_bound(bound: f64) -> String {
    if bound == 0.0 {
        return "0".to_string();
    }
    if !bound.is_finite() {
        return "nonfinite".to_string();
    }
    let sign = if bound < 0.0 { "-" } else { "" };
    let abs = bound.abs();
    let mut exponent = abs.log10().floor() as i32;
    let mut base = (abs / 10f64.powi(exponent)).floor() as i32;
    // Floating-point edge at exact powers of ten (e.g. 1000.0 may
    // quantize as base 10): renormalize instead of emitting `10eN`.
    if base >= 10 {
        base = 1;
        exponent += 1;
    }
    format!("{sign}{base}e{exponent}")
}

/// Selective Estimator
///
/// Provide selective estimates based on statistical information and heuristic rules.
#[derive(Debug)]
pub struct SelectivityEstimator {
    stats_manager: Arc<StatisticsManager>,
    /// Optional feedback-driven correction source (stats
    /// feedback loop). When present, `estimate_from_expression` prefers the
    /// corrected selectivity for a known condition key over the histogram /
    /// heuristic estimate and registers newly estimated conditions.
    feedback: Option<Arc<SelectivityFeedbackManager>>,
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
        Self {
            stats_manager,
            feedback: None,
        }
    }

    /// Create a selective estimator wired to a feedback correction manager.
    ///
    /// When feedback is enabled, `estimate_from_expression` consults the
    /// corrected selectivity for the normalized condition key first and
    /// registers newly estimated conditions for later correction.
    pub fn with_feedback(
        stats_manager: Arc<StatisticsManager>,
        feedback: Arc<SelectivityFeedbackManager>,
    ) -> Self {
        Self {
            stats_manager,
            feedback: Some(feedback),
        }
    }

    /// Return the underlying feedback correction manager, if any.
    pub fn feedback_manager(&self) -> Option<&Arc<SelectivityFeedbackManager>> {
        self.feedback.as_ref()
    }

    /// Capture the learned corrections for one planning decision to read
    /// through. Empty when no feedback manager is attached.
    pub fn snapshot(&self) -> SelectivitySnapshot {
        self.feedback
            .as_ref()
            .map(|feedback| feedback.snapshot())
            .unwrap_or_default()
    }

    /// Estimate a predicate without touching shared feedback state.
    ///
    /// Unlike [`estimate_from_expression`](Self::estimate_from_expression),
    /// this neither registers the condition nor refreshes its baseline: the
    /// snapshotted correction (if any) applies to the fresh estimate, so
    /// repeated calls stay identical until execution feedback lands.
    pub fn estimate_readonly(
        &self,
        space: Option<&str>,
        expr: &Expression,
        tag_name: Option<&str>,
        snapshot: &SelectivitySnapshot,
    ) -> f64 {
        self.estimate_inner(space, expr, tag_name, Some(snapshot))
    }

    /// Estimated Equivalence Condition Selectivity
    ///
    /// If there is histogram statistical information, use the histogram for an accurate estimation.
    /// Otherwise, if basic statistical information is available, use 1 divided by the number of distinct values.
    /// Otherwise the default value of 0.1 is used
    pub fn estimate_equality_selectivity(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        property_name: &str,
        value: Option<&Value>,
    ) -> f64 {
        let stats = space.and_then(|s| {
            self.stats_manager
                .get_property_stats(s, tag_name, property_name)
        });

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
        space: Option<&str>,
        tag_name: Option<&str>,
        property_name: &str,
    ) -> f64 {
        self.estimate_equality_selectivity(space, tag_name, property_name, None)
    }

    /// Conditional selectivity of the estimation range
    ///
    /// If there is histogram statistical information, the calculations are based on that histogram.
    /// Otherwise, use the default value of 1/3.
    pub fn estimate_range_selectivity(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        property_name: &str,
        range: &RangeCondition,
    ) -> f64 {
        let stats = space.and_then(|s| {
            self.stats_manager
                .get_property_stats(s, tag_name, property_name)
        });

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

    /// Range-comparison selectivity from the sampled min/max envelope.
    ///
    /// Linear interpolation over `[min, max]` assuming uniform distribution:
    /// `prop > bound` covers `(max - bound) / (max - min)` of the range.
    /// Falls back to the fixed heuristics when no envelope is available.
    pub fn estimate_ordered_comparison(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        property_name: &str,
        bound: Option<f64>,
        greater_than: bool,
    ) -> f64 {
        let Some(bound) = bound else {
            return if greater_than {
                self.estimate_greater_than_selectivity(None)
            } else {
                self.estimate_less_than_selectivity(None)
            };
        };
        let Some(stats) = space.and_then(|s| {
            self.stats_manager
                .get_property_stats(s, tag_name, property_name)
        }) else {
            return if greater_than {
                self.estimate_greater_than_selectivity(Some(bound))
            } else {
                self.estimate_less_than_selectivity(Some(bound))
            };
        };
        let (Some(min), Some(max)) = (
            stats.min_value.as_ref().and_then(numeric_as_f64),
            stats.max_value.as_ref().and_then(numeric_as_f64),
        ) else {
            return if greater_than {
                self.estimate_greater_than_selectivity(Some(bound))
            } else {
                self.estimate_less_than_selectivity(Some(bound))
            };
        };
        let span = max - min;
        if !span.is_finite() || span <= 0.0 {
            // Single-valued column: the comparison either keeps everything or
            // almost nothing.
            let keeps_all = if greater_than {
                bound < min
            } else {
                bound > max
            };
            return if keeps_all { 0.95 } else { 0.01 };
        }
        let fraction = if greater_than {
            (max - bound) / span
        } else {
            (bound - min) / span
        };
        fraction.clamp(0.001, 0.999).min(0.8)
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
    ///
    /// When a feedback correction manager is attached, the normalized
    /// condition key is looked up first: a registered correction overrides
    /// the histogram / heuristic estimate. Otherwise the estimate is
    /// computed and registered so later executions can correct it.
    pub fn estimate_from_expression(
        &self,
        space: Option<&str>,
        expr: &Expression,
        tag_name: Option<&str>,
    ) -> f64 {
        self.estimate_inner(space, expr, tag_name, None)
    }

    /// Shared estimate body. `None` snapshot is the recording path used by
    /// the row estimators (the feedback loop's write path); `Some` is the
    /// read-only path used by planning decisions.
    fn estimate_inner(
        &self,
        space: Option<&str>,
        expr: &Expression,
        tag_name: Option<&str>,
        snapshot: Option<&SelectivitySnapshot>,
    ) -> f64 {
        let key = condition_key(space, expr);
        let estimated = match expr {
            Expression::Binary { op, left, right } => {
                self.estimate_binary_expression(space, op, left, right, tag_name, snapshot)
            }
            Expression::Unary { op, operand } => {
                self.estimate_unary_expression(space, op, operand, tag_name, snapshot)
            }
            Expression::Function { name, args } => {
                let exprs: Vec<Expression> = args.iter().map(|a| a.as_expr().clone()).collect();
                self.estimate_function_expression(space, tag_name, name, &exprs)
            }
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
        };

        match snapshot {
            // Recording path: register the estimate as the feedback baseline
            // on first sight and keep tracking fresh statistics afterwards; a
            // learned correction overrides the fresh estimate when present.
            None => {
                if let Some(feedback) = &self.feedback {
                    feedback.register_condition(key.clone(), estimated);
                    feedback.refresh_estimated(&key, estimated);
                    if let Some(corrected) = feedback.get_corrected_selectivity(&key) {
                        return corrected;
                    }
                }
                estimated
            }
            // Read-only path: apply the snapshotted correction without
            // registering or refreshing anything.
            Some(snapshot) => snapshot.corrected(&key, estimated).unwrap_or(estimated),
        }
    }

    /// Estimating the selectivity of binary expressions
    fn estimate_binary_expression(
        &self,
        space: Option<&str>,
        op: &BinaryOperator,
        left: &Expression,
        right: &Expression,
        tag_name: Option<&str>,
        snapshot: Option<&SelectivitySnapshot>,
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
                    self.estimate_equality_selectivity(space, tag_name, &prop, value.as_ref())
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
                if let Some(prop) = self.extract_property_name(left) {
                    self.estimate_ordered_comparison(space, tag_name, &prop, value, false)
                } else {
                    self.estimate_less_than_selectivity(value)
                }
            }
            BinaryOperator::LessThanOrEqual => {
                let value = self.extract_numeric_value(right);
                if let Some(prop) = self.extract_property_name(left) {
                    self.estimate_ordered_comparison(space, tag_name, &prop, value, false)
                        .clamp(0.01, 0.9)
                } else {
                    self.estimate_less_than_selectivity(value).clamp(0.01, 0.9)
                }
            }
            BinaryOperator::GreaterThan => {
                let value = self.extract_numeric_value(right);
                if let Some(prop) = self.extract_property_name(left) {
                    self.estimate_ordered_comparison(space, tag_name, &prop, value, true)
                } else {
                    self.estimate_greater_than_selectivity(value)
                }
            }
            BinaryOperator::GreaterThanOrEqual => {
                let value = self.extract_numeric_value(right);
                if let Some(prop) = self.extract_property_name(left) {
                    self.estimate_ordered_comparison(space, tag_name, &prop, value, true)
                        .clamp(0.01, 0.9)
                } else {
                    self.estimate_greater_than_selectivity(value)
                        .clamp(0.01, 0.9)
                }
            }
            BinaryOperator::And => {
                let left_sel = self.estimate_inner(space, left, tag_name, snapshot);
                let right_sel = self.estimate_inner(space, right, tag_name, snapshot);
                // The selectivity of the “AND” operator is usually slightly higher than that of the multiplication operator (because there may be correlations between the conditions).
                (left_sel * right_sel / defaults::AND_CORRELATION).min(1.0)
            }
            BinaryOperator::Or => {
                let left_sel = self.estimate_inner(space, left, tag_name, snapshot);
                let right_sel = self.estimate_inner(space, right, tag_name, snapshot);
                // OR selectivity: P(A or B) = P(A) + P(B) - P(A and B)
                let combined =
                    left_sel + right_sel - left_sel * right_sel * defaults::OR_CORRELATION;
                combined.clamp(0.01, 0.99)
            }
            BinaryOperator::In => {
                // Estimating the size of the IN list, narrowed by the
                // property NDV when column statistics are available.
                let list_size = self.estimate_list_size(right);
                if let Some(prop) = self
                    .extract_property_name(left)
                    .or_else(|| self.extract_property_name(right))
                {
                    return self
                        .estimate_in_selectivity_for_property(space, tag_name, &prop, list_size);
                }
                self.estimate_in_selectivity(list_size)
            }
            _ => defaults::EQUALITY,
        }
    }

    /// Estimating the selectivity of a unary expression
    fn estimate_unary_expression(
        &self,
        space: Option<&str>,
        op: &linkrs_core::types::UnaryOperator,
        expr: &Expression,
        tag_name: Option<&str>,
        snapshot: Option<&SelectivitySnapshot>,
    ) -> f64 {
        use linkrs_core::types::UnaryOperator;

        match op {
            UnaryOperator::Not => {
                let inner = self.estimate_inner(space, expr, tag_name, snapshot);
                self.estimate_not_selectivity(inner)
            }
            UnaryOperator::IsNull => self
                .null_fraction_for(space, tag_name, expr)
                .unwrap_or(defaults::IS_NULL),
            UnaryOperator::IsNotNull => self
                .null_fraction_for(space, tag_name, expr)
                .map(|f| 1.0 - f)
                .unwrap_or(defaults::IS_NOT_NULL),
            _ => defaults::EQUALITY,
        }
    }

    /// Null fraction for the column referenced by a unary operand, if tracked.
    fn null_fraction_for(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        expr: &Expression,
    ) -> Option<f64> {
        let space = space?;
        let property = self.extract_property_name(expr)?;
        self.stats_manager
            .get_property_stats(space, tag_name, &property)
            .and_then(|s| s.null_fraction())
    }

    /// Equality selectivity floor for a property from collected NDV.
    ///
    /// Returns `0.0` when no usable statistics exist so callers can keep
    /// their pattern heuristic untouched. Otherwise `1 / NDV`: a fuzzy or
    /// prefix match cannot be more selective than the equality on the same
    /// column, so the heuristic is floored (never shrunk) by this value.
    /// This only corrects underestimation on low-cardinality columns; on
    /// high-cardinality columns the pattern heuristic stays as the
    /// conservative upper bound.
    fn ndv_equality_floor(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        property: Option<&str>,
    ) -> f64 {
        let Some(prop) = property else {
            return 0.0;
        };
        let Some(space) = space else {
            return 0.0;
        };
        self.stats_manager
            .get_property_stats(space, tag_name, prop)
            .map(|s| s.distinct_values)
            .filter(|ndv| *ndv > 0)
            .map(|ndv| 1.0 / ndv as f64)
            .unwrap_or(0.0)
    }

    /// NDV-aware IN-list selectivity for one property column.
    ///
    /// `list_size / NDV` capped at 0.9 when statistics exist; otherwise the
    /// fixed per-value heuristic.
    fn estimate_in_selectivity_for_property(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        property_name: &str,
        list_size: usize,
    ) -> f64 {
        if let Some(space) = space {
            if let Some(stats) =
                self.stats_manager
                    .get_property_stats(space, tag_name, property_name)
            {
                if stats.distinct_values > 0 {
                    return (list_size as f64 / stats.distinct_values as f64).clamp(0.001, 0.9);
                }
            }
        }
        self.estimate_in_selectivity(list_size)
    }

    /// Estimating the selectivity of function expressions
    fn estimate_function_expression(
        &self,
        space: Option<&str>,
        tag_name: Option<&str>,
        name: &str,
        args: &[Expression],
    ) -> f64 {
        let name_lower = name.to_lowercase();
        let first_property = args.first().and_then(|a| self.extract_property_name(a));

        match name_lower.as_str() {
            "like" | "ilike" if args.len() >= 2 => {
                // Try to extract the LIKE pattern.
                if let Expression::Literal(linkrs_core::value::Value::String(pattern)) = &args[1] {
                    let heuristic = self.estimate_like_selectivity(pattern);
                    let floor = self.ndv_equality_floor(space, tag_name, first_property.as_deref());
                    return heuristic.max(floor);
                }
                defaults::EQUALITY
            }
            "exists" => defaults::EXISTS,
            "contains" | "has" => {
                let floor = self.ndv_equality_floor(space, tag_name, first_property.as_deref());
                0.2_f64.max(floor)
            }
            "starts_with" => {
                let floor = self.ndv_equality_floor(space, tag_name, first_property.as_deref());
                0.1_f64.max(floor)
            }
            "ends_with" => {
                let floor = self.ndv_equality_floor(space, tag_name, first_property.as_deref());
                0.2_f64.max(floor)
            }
            "in" => {
                let list_size = args.len().saturating_sub(1);
                if let Some(prop) = first_property {
                    return self
                        .estimate_in_selectivity_for_property(space, tag_name, &prop, list_size);
                }
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
            Expression::Literal(value) => numeric_as_f64(value),
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

/// Numeric projection shared by expression literals and stored min/max
/// envelope values.
fn numeric_as_f64(value: &Value) -> Option<f64> {
    match value {
        Value::SmallInt(i) => Some(*i as f64),
        Value::Int(i) => Some(*i as f64),
        Value::BigInt(i) => Some(*i as f64),
        Value::Float(f) => Some((*f).into()),
        Value::Double(f) => Some(*f),
        _ => None,
    }
}

impl Clone for SelectivityEstimator {
    fn clone(&self) -> Self {
        Self {
            stats_manager: self.stats_manager.clone(),
            feedback: self.feedback.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::stats::PropertyStatistics;

    fn estimator_with_envelope(
        space: &str,
        tag: &str,
        prop: &str,
        min: i32,
        max: i32,
    ) -> SelectivityEstimator {
        let manager = Arc::new(StatisticsManager::new());
        let mut stat = PropertyStatistics::new(prop.to_string(), Some(tag.to_string()));
        stat.observe_value(&Value::Int(min));
        stat.observe_value(&Value::Int(max));
        manager.update_property_stats(space, stat);
        SelectivityEstimator::new(manager)
    }

    fn comparison_expr(prop: &str, bound: i64, op: BinaryOperator) -> Expression {
        Expression::Binary {
            op,
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: prop.to_string(),
            }),
            right: Box::new(Expression::Literal(Value::BigInt(bound))),
        }
    }

    #[test]
    fn greater_than_interpolates_over_envelope() {
        let est = estimator_with_envelope("s", "person", "age", 0, 100);
        // age > 80 covers 20% of [0, 100].
        let sel = est.estimate_from_expression(
            Some("s"),
            &comparison_expr("age", 80, BinaryOperator::GreaterThan),
            Some("person"),
        );
        assert!((sel - 0.2).abs() < 0.01, "sel={sel}");
    }

    #[test]
    fn less_than_beyond_max_is_nearly_total() {
        let est = estimator_with_envelope("s", "person", "age", 0, 100);
        let sel = est.estimate_from_expression(
            Some("s"),
            &comparison_expr("age", 500, BinaryOperator::LessThan),
            Some("person"),
        );
        assert!(sel > 0.7, "sel={sel}");
    }

    #[test]
    fn missing_envelope_falls_back_to_heuristic() {
        let manager = Arc::new(StatisticsManager::new());
        let est = SelectivityEstimator::new(manager.clone());
        let sel = est.estimate_from_expression(
            Some("s"),
            &comparison_expr("unknown", 10, BinaryOperator::GreaterThan),
            Some("person"),
        );
        assert!((sel - defaults::COMPARISON).abs() < 1e-9, "sel={sel}");
    }

    fn estimator_with_ndv(
        space: &str,
        tag: &str,
        prop: &str,
        distinct: u64,
    ) -> SelectivityEstimator {
        let manager = Arc::new(StatisticsManager::new());
        let mut stat = PropertyStatistics::new(prop.to_string(), Some(tag.to_string()));
        stat.distinct_values = distinct;
        manager.update_property_stats(space, stat);
        SelectivityEstimator::new(manager)
    }

    fn property_expr(prop: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: prop.to_string(),
        }
    }

    #[test]
    fn in_list_uses_ndv_when_available() {
        let est = estimator_with_ndv("s", "person", "age", 100);
        let expr = Expression::Binary {
            op: BinaryOperator::In,
            left: Box::new(property_expr("age")),
            right: Box::new(Expression::List(vec![
                Expression::Literal(Value::Int(1)),
                Expression::Literal(Value::Int(2)),
                Expression::Literal(Value::Int(3)),
            ])),
        };
        let sel = est.estimate_from_expression(Some("s"), &expr, Some("person"));
        // 3 / 100 = 0.03, far below the fixed 0.3 heuristic.
        assert!((sel - 0.03).abs() < 1e-9, "sel={sel}");
    }

    #[test]
    fn in_list_without_stats_keeps_heuristic() {
        let manager = Arc::new(StatisticsManager::new());
        let est = SelectivityEstimator::new(manager);
        let expr = Expression::Binary {
            op: BinaryOperator::In,
            left: Box::new(property_expr("age")),
            right: Box::new(Expression::List(vec![
                Expression::Literal(Value::Int(1)),
                Expression::Literal(Value::Int(2)),
                Expression::Literal(Value::Int(3)),
            ])),
        };
        let sel = est.estimate_from_expression(Some("s"), &expr, Some("person"));
        assert!((sel - 0.3).abs() < 1e-9, "sel={sel}");
    }

    #[test]
    fn readonly_estimate_neither_registers_nor_refreshes() {
        use std::sync::Arc;

        let stats = Arc::new(StatisticsManager::new());
        let feedback = Arc::new(SelectivityFeedbackManager::new());
        let estimator =
            SelectivityEstimator::with_feedback(Arc::clone(&stats), Arc::clone(&feedback));
        let expr = Expression::Binary {
            op: BinaryOperator::Equal,
            left: Box::new(property_expr("age")),
            right: Box::new(Expression::Literal(Value::Int(30))),
        };

        let pristine = estimator.snapshot();
        assert!(pristine.is_empty());
        let first = estimator.estimate_readonly(None, &expr, None, &pristine);
        assert!((first - defaults::EQUALITY).abs() < 1e-9, "sel={first}");
        assert!(
            estimator.snapshot().is_empty(),
            "read-only estimates must not register conditions"
        );

        let recorded = estimator.estimate_from_expression(None, &expr, None);
        assert!((recorded - first).abs() < 1e-9, "recorded={recorded}");
        assert_eq!(estimator.snapshot().len(), 1);

        // Learned feedback applies through later snapshots only, and only
        // after enough samples: a single update stays below the stability
        // bar and keeps pricing the fresh estimate.
        let key = condition_key(None, &expr);
        feedback.update_feedback_ratio(&key, 2.0);
        let unstable = estimator.snapshot();
        assert_eq!(
            estimator.estimate_readonly(None, &expr, None, &unstable),
            first,
            "single-sample corrections must not move planning reads"
        );
        feedback.update_feedback_ratio(&key, 2.0);
        feedback.update_feedback_ratio(&key, 2.0);
        let learned = estimator.snapshot();
        let corrected = estimator.estimate_readonly(None, &expr, None, &learned);
        // EWMA with alpha 0.3 over three 2.0 ratios: 1.0 -> 1.3 -> 1.51 -> 1.657.
        let expected = (defaults::EQUALITY * 1.657).clamp(0.1 * defaults::EQUALITY, 0.99);
        assert!(
            (corrected - expected).abs() < 1e-9,
            "corrected={corrected} expected={expected}"
        );
        assert_eq!(
            estimator.estimate_readonly(None, &expr, None, &pristine),
            first
        );
    }

    #[test]
    fn fuzzy_match_floored_by_ndv_on_low_cardinality() {
        // Prefix heuristic is 0.1, but with only 2 distinct values the match
        // cannot be more selective than the 0.5 equality floor.
        let est = estimator_with_ndv("s", "person", "name", 2);
        let expr = Expression::Function {
            name: "starts_with".to_string(),
            args: vec![linkrs_core::types::expr::FunctionArg::positional(
                property_expr("name"),
            )],
        };
        let sel = est.estimate_from_expression(Some("s"), &expr, Some("person"));
        assert!((sel - 0.5).abs() < 1e-9, "sel={sel}");
    }

    fn lt_expr(bound: i32) -> Expression {
        Expression::Binary {
            op: BinaryOperator::LessThan,
            left: Box::new(property_expr("value")),
            right: Box::new(Expression::Literal(Value::Int(bound))),
        }
    }

    #[test]
    fn range_bound_bucket_separates_distant_bounds() {
        // `value < 800` and `value < 801` share one bucket; `value < 10`
        // learns separately so one bound's correction cannot leak into
        // a selectivity regime it never measured.
        let near_a = condition_key(Some("s"), &lt_expr(800));
        let near_b = condition_key(Some("s"), &lt_expr(801));
        let far = condition_key(Some("s"), &lt_expr(10));
        assert_eq!(near_a, near_b, "near_a={near_a} near_b={near_b}");
        assert_ne!(near_a, far, "near={near_a} far={far}");
        assert!(
            near_a.contains("~bound8e2"),
            "range key must carry its bucket: {near_a}"
        );
    }

    #[test]
    fn equality_keeps_structural_key_without_bound_bucket() {
        let expr = Expression::Binary {
            op: BinaryOperator::Equal,
            left: Box::new(property_expr("age")),
            right: Box::new(Expression::Literal(Value::Int(30))),
        };
        let key = condition_key(Some("s"), &expr);
        assert!(
            !key.contains("~bound"),
            "equality must not gain a bound bucket: {key}"
        );
    }
}
