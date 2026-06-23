//! Subquery de-association optimization module
//!
//! "Analysis-based subquery deserialization optimization strategy" – This strategy converts simple PatternApply subqueries into HashInnerJoin operations.
//!
//! ## Optimization Strategies
//!
//! Convert the eligible PatternApply subquery into a HashInnerJoin.
//! Avoid executing subqueries repeatedly.
//!
//! ## Applicable Conditions
//!
//! The right input for PatternApply is a simple query (single-table scan + equality filtering).
//! 2. The filtering conditions are deterministic (excluding rand(), now(), etc.)
//! 3. The complexity of the expressions should be less than 50 (avoid using complex expressions).
//! 4. The subquery estimates that the number of rows is less than 1000 (based on statistical information).
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::SubqueryUnnestingOptimizer;
//! use graphdb::query::optimizer::OptimizerEngine;
//!
//! let optimizer = SubqueryUnnestingOptimizer::new(engine.stats_manager());
//! let decision = optimizer.should_unnest(&pattern_apply, &analysis);
//! ```

use crate::core::types::expr::ExpressionMeta;
use crate::core::types::operators::BinaryOperator;
use crate::core::types::ContextualExpression;
use crate::core::Expression;
use crate::query::optimizer::analysis::BatchPlanAnalysis;
use crate::query::optimizer::stats::StatisticsManager;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::{HashInnerJoinNode, PatternApplyNode};
use crate::query::validator::context::ExpressionAnalysisContext;

/// Decentralized decision-making using subqueries
#[derive(Debug, Clone, PartialEq)]
pub enum UnnestDecision {
    /// Convert to HashInnerJoin
    ShouldUnnest {
        /// Reason for the decision
        reason: UnnestReason,
        /// Estimated original cost
        original_cost: f64,
        /// Estimated cost after the conversion
        unnested_cost: f64,
    },
    /// Keep the current pattern and apply optimization
    KeepPatternApply {
        /// Reason for retention
        reason: KeepReason,
    },
}

/// Reason for conversion
#[derive(Debug, Clone, PartialEq)]
pub enum UnnestReason {
    /// Simple subquery; the conversion is more efficient.
    SimpleSubquery,
    /// Based on cost analysis
    CostBased,
}

/// Reasons for reservations
#[derive(Debug, Clone, PartialEq)]
pub enum KeepReason {
    /// The subquery is too complex.
    TooComplex,
    /// The subquery contains a non-deterministic function.
    NonDeterministic,
    /// The number of rows estimated by the subquery is too large.
    TooManyRows,
    /// The subquery contains complex conditions.
    ComplexCondition,
}

/// Subquery desaggregation optimizer
///
/// Based on batch plan analysis and statistical information, a decision is made as to whether to convert PatternApply to HashInnerJoin.
#[derive(Debug, Clone)]
pub struct SubqueryUnnestingOptimizer {
    /// Statistics Information Manager
    stats_manager: StatisticsManager,
    /// The maximum number of estimated rows allowed for a subquery
    max_subquery_rows: u64,
    /// The maximum allowable complexity of the expression
    max_complexity: u32,
}

impl SubqueryUnnestingOptimizer {
    /// Create a new optimizer.
    pub fn new(stats_manager: &StatisticsManager) -> Self {
        Self {
            stats_manager: stats_manager.clone(),
            max_subquery_rows: 1000,
            max_complexity: 50,
        }
    }

    /// Set a threshold for the maximum number of rows in subqueries
    pub fn with_max_rows(mut self, max_rows: u64) -> Self {
        self.max_subquery_rows = max_rows;
        self
    }

    /// Set a threshold for the maximum complexity.
    pub fn with_max_complexity(mut self, max_complexity: u32) -> Self {
        self.max_complexity = max_complexity;
        self
    }

    /// Determine whether decoupling should be performed.
    ///
    /// # Parameters
    /// `pattern_apply`: The PatternApply node
    /// `analysis`: The batch plan analysis result
    ///
    /// # Decision
    /// De-associative decision-making
    pub fn should_unnest(
        &self,
        pattern_apply: &PatternApplyNode,
        analysis: &BatchPlanAnalysis,
    ) -> UnnestDecision {
        // 1. Check determinism from batch analysis
        if !analysis.expression_summary.is_fully_deterministic {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::NonDeterministic,
            };
        }

        // 2. Check complexity from batch analysis
        if analysis.expression_summary.total_complexity > self.max_complexity {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::ComplexCondition,
            };
        }

        // 3. Checking subqueries for simplicity
        if !self.is_simple_subquery(pattern_apply.right_input()) {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::TooComplex,
            };
        }

        // 4. Checking the number of estimated rows for subqueries
        let estimated_rows = self.estimate_subquery_rows(pattern_apply.right_input());
        if estimated_rows > self.max_subquery_rows {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::TooManyRows,
            };
        }

        // 5. Comparison of costs (simplified version)
        let original_cost = self.estimate_pattern_apply_cost(estimated_rows);
        let unnested_cost = self.estimate_hash_join_cost(estimated_rows);

        if unnested_cost < original_cost {
            UnnestDecision::ShouldUnnest {
                reason: UnnestReason::CostBased,
                original_cost,
                unnested_cost,
            }
        } else {
            UnnestDecision::ShouldUnnest {
                reason: UnnestReason::SimpleSubquery,
                original_cost,
                unnested_cost,
            }
        }
    }

    /// Check whether the subquery is simple (involving a scan of a single table and equality filtering).
    fn is_simple_subquery(&self, node: &PlanNodeEnum) -> bool {
        match node {
            // Single Table Scan
            PlanNodeEnum::ScanVertices(_) => true,
            // simple filtration
            PlanNodeEnum::Filter(n) => {
                // Check if the filter condition is an equal comparison
                let condition = n.condition();

                // Check if it is a simple comparison of equal values
                if let Some(expr_meta) = condition.expression() {
                    if !self.is_simple_equality_condition(expr_meta.inner()) {
                        return false;
                    }
                }
                // Recursively checking the input
                self.is_simple_subquery(
                    crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(n),
                )
            }

            // simple projection
            PlanNodeEnum::Project(n) => self.is_simple_subquery(
                crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(n),
            ),

            // Not supported in other cases
            _ => false,
        }
    }

    /// Check whether the condition is a simple equality comparison.
    fn is_simple_equality_condition(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Binary { op, left, right } => match op {
                BinaryOperator::Equal => {
                    self.is_simple_expression(left.as_ref())
                        && self.is_simple_expression(right.as_ref())
                }
                BinaryOperator::And => {
                    self.is_simple_equality_condition(left.as_ref())
                        && self.is_simple_equality_condition(right.as_ref())
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Check whether the expression is simple (consisting of literals, variables, or properties).
    fn is_simple_expression(&self, expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Literal(_) | Expression::Variable(_) | Expression::Property { .. }
        )
    }

    /// Estimating the number of rows returned by a subquery
    fn estimate_subquery_rows(&self, node: &PlanNodeEnum) -> u64 {
        match node {
            PlanNodeEnum::ScanVertices(n) => {
                // Get the number of label vertices from statistics
                if let Some(tag_name) = n.tag() {
                    if let Some(stats) = self.stats_manager.get_tag_stats(tag_name) {
                        stats.vertex_count
                    } else {
                        1
                    }
                } else {
                    1000 // default value
                }
            }
            PlanNodeEnum::Filter(n) => {
                // Filtered estimate is 30% of original rows
                (self.estimate_subquery_rows(crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(n)) as f64 * 0.3) as u64
            }
            PlanNodeEnum::Project(n) => self.estimate_subquery_rows(
                crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode::input(
                    n,
                ),
            ),
            _ => 1000, // default value
        }
    }

    /// Estimate the cost of applying the PatternApply method
    fn estimate_pattern_apply_cost(&self, subquery_rows: u64) -> f64 {
        // Simplified estimation: nested loops that execute subqueries each time
        // Assuming an average of 100 rows in the left table
        let left_rows = 100.0;
        left_rows * (subquery_rows as f64 * 0.1) // Subquery startup cost + execution cost
    }

    /// Estimating the cost of a HashJoin operation
    fn estimate_hash_join_cost(&self, subquery_rows: u64) -> f64 {
        // Simplified estimation: hash connections
        let right_rows = subquery_rows as f64;
        let left_rows = 100.0;

        // Cost of building the hash table + cost of probing
        let build_cost = right_rows;
        let probe_cost = left_rows * 0.5; // Hash detection is fast.

        build_cost + probe_cost
    }

    /// Perform the de-association transformation.
    ///
    /// # Parameters
    /// - `pattern_apply`: PatternApply node
    ///
    /// # Returns
    /// The transformed HashInnerJoin node
    pub fn unnest(
        &self,
        pattern_apply: PatternApplyNode,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let key_cols = pattern_apply.key_cols().to_vec();

        let left_var = pattern_apply
            .left_input_var()
            .cloned()
            .unwrap_or_else(|| "left".to_string());
        let right_var = pattern_apply
            .right_input_var()
            .cloned()
            .unwrap_or_else(|| "right".to_string());

        let expr_ctx = std::sync::Arc::new(ExpressionAnalysisContext::new());

        let mut hash_keys = Vec::new();
        let mut probe_keys = Vec::new();

        for key_col in &key_cols {
            if let Some(original_meta) = key_col.expression() {
                let original_expr = original_meta.inner();
                let left_key_expr = self.replace_all_variables(original_expr, &left_var);
                let left_key_meta = ExpressionMeta::new(left_key_expr);
                let left_key_id = expr_ctx.register_expression(left_key_meta);
                let left_key_contextual = ContextualExpression::new(left_key_id, expr_ctx.clone());
                hash_keys.push(left_key_contextual);

                let right_key_expr = self.replace_all_variables(original_expr, &right_var);
                let right_key_meta = ExpressionMeta::new(right_key_expr);
                let right_key_id = expr_ctx.register_expression(right_key_meta);
                let right_key_contextual =
                    ContextualExpression::new(right_key_id, expr_ctx.clone());
                probe_keys.push(right_key_contextual);
            }
        }

        let left_input = pattern_apply.left_input().clone();
        let right_input = pattern_apply.right_input().clone();

        let hash_join_node =
            HashInnerJoinNode::new(left_input, right_input, hash_keys, probe_keys)?;

        Ok(PlanNodeEnum::HashInnerJoin(hash_join_node))
    }

    /// Replace all variable references in the expression with the specified variables.
    ///
    /// This method recursively traverses the expression tree and replaces all Variable nodes with the specified variable name.
    /// This is used to convert the variables in the original expression when transforming PatternApply to HashInnerJoin.
    /// The placeholders (usually “_”) should be replaced with the variable names provided on the left and on the right.
    ///
    /// # Parameter
    /// `expr`: The expression that needs to be converted.
    /// `new_var`: The name of the new variable
    ///
    /// # Returns
    /// The expression with all variables replaced
    fn replace_all_variables(&self, expr: &Expression, new_var: &str) -> Expression {
        match expr {
            Expression::Variable(_) => Expression::Variable(new_var.to_string()),
            Expression::Property { object, property } => Expression::Property {
                object: Box::new(self.replace_all_variables(object, new_var)),
                property: property.clone(),
            },
            Expression::Binary { op, left, right } => Expression::Binary {
                op: *op,
                left: Box::new(self.replace_all_variables(left, new_var)),
                right: Box::new(self.replace_all_variables(right, new_var)),
            },
            Expression::Unary { op, operand } => Expression::Unary {
                op: *op,
                operand: Box::new(self.replace_all_variables(operand, new_var)),
            },
            Expression::Function { name, args } => Expression::Function {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| self.replace_all_variables(arg, new_var))
                    .collect(),
            },
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => Expression::Aggregate {
                func: func.clone(),
                arg: Box::new(self.replace_all_variables(arg, new_var)),
                distinct: *distinct,
            },
            Expression::List(items) => Expression::List(
                items
                    .iter()
                    .map(|item| self.replace_all_variables(item, new_var))
                    .collect(),
            ),
            Expression::Map(entries) => Expression::Map(
                entries
                    .iter()
                    .map(|(k, v): &(String, Expression)| {
                        (k.clone(), self.replace_all_variables(v, new_var))
                    })
                    .collect(),
            ),
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => Expression::Case {
                test_expr: test_expr
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
                conditions: conditions
                    .iter()
                    .map(|(w, t)| {
                        (
                            self.replace_all_variables(w, new_var),
                            self.replace_all_variables(t, new_var),
                        )
                    })
                    .collect(),
                default: default
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
            },
            Expression::TypeCast {
                expression,
                target_type,
            } => Expression::TypeCast {
                expression: Box::new(self.replace_all_variables(expression, new_var)),
                target_type: target_type.clone(),
            },
            Expression::Subscript { collection, index } => Expression::Subscript {
                collection: Box::new(self.replace_all_variables(collection, new_var)),
                index: Box::new(self.replace_all_variables(index, new_var)),
            },
            Expression::Range {
                collection,
                start,
                end,
            } => Expression::Range {
                collection: Box::new(self.replace_all_variables(collection, new_var)),
                start: start
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
                end: end
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
            },
            Expression::Path(exprs) => Expression::Path(
                exprs
                    .iter()
                    .map(|e| self.replace_all_variables(e, new_var))
                    .collect(),
            ),
            Expression::Label(_) => expr.clone(),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => Expression::ListComprehension {
                variable: variable.clone(),
                source: Box::new(self.replace_all_variables(source, new_var)),
                filter: filter
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
                map: map
                    .as_ref()
                    .map(|e| Box::new(self.replace_all_variables(e, new_var))),
            },
            Expression::LabelTagProperty { tag, property } => Expression::LabelTagProperty {
                tag: Box::new(self.replace_all_variables(tag, new_var)),
                property: property.clone(),
            },
            Expression::TagProperty { tag_name, property } => Expression::TagProperty {
                tag_name: tag_name.clone(),
                property: property.clone(),
            },
            Expression::EdgeProperty {
                edge_name,
                property,
            } => Expression::EdgeProperty {
                edge_name: edge_name.clone(),
                property: property.clone(),
            },
            Expression::Predicate { func, args } => Expression::Predicate {
                func: func.clone(),
                args: args
                    .iter()
                    .map(|arg| self.replace_all_variables(arg, new_var))
                    .collect(),
            },
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => Expression::Reduce {
                accumulator: accumulator.clone(),
                initial: Box::new(self.replace_all_variables(initial, new_var)),
                variable: variable.clone(),
                source: Box::new(self.replace_all_variables(source, new_var)),
                mapping: Box::new(self.replace_all_variables(mapping, new_var)),
            },
            Expression::PathBuild(exprs) => Expression::PathBuild(
                exprs
                    .iter()
                    .map(|e| self.replace_all_variables(e, new_var))
                    .collect(),
            ),
            Expression::Parameter(_) => expr.clone(),
            Expression::Literal(_) => expr.clone(),
            Expression::Vector(_) => expr.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_optimizer_creation() {
        let stats_manager = StatisticsManager::new();
        let optimizer = SubqueryUnnestingOptimizer::new(&stats_manager);
        assert_eq!(optimizer.max_subquery_rows, 1000);
        assert_eq!(optimizer.max_complexity, 50);
    }

    #[test]
    fn test_optimizer_with_config() {
        let stats_manager = StatisticsManager::new();
        let _optimizer = SubqueryUnnestingOptimizer::new(&stats_manager)
            .with_max_rows(500)
            .with_max_complexity(30);
    }

    #[test]
    fn test_simple_expression_check() {
        let stats_manager = StatisticsManager::new();
        let optimizer = SubqueryUnnestingOptimizer::new(&stats_manager);

        let literal = Expression::Literal(crate::core::Value::Int(42));
        assert!(optimizer.is_simple_expression(&literal));

        let variable = Expression::Variable("n".to_string());
        assert!(optimizer.is_simple_expression(&variable));

        let property = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        assert!(optimizer.is_simple_expression(&property));

        let binary = Expression::Binary {
            left: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            op: crate::core::types::operators::BinaryOperator::Add,
            right: Box::new(Expression::Literal(crate::core::Value::Int(2))),
        };
        assert!(!optimizer.is_simple_expression(&binary));
    }
}
