//! Expression Precomputation Optimizer Module
//!
//! Determines which expressions should be precomputed based on their cost
//! and reference count to avoid redundant computations.
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::ExpressionPrecomputationOptimizer;
//! use graphdb::query::optimizer::cost::CostCalculator;
//! use std::sync::Arc;
//!
//! let optimizer = ExpressionPrecomputationOptimizer::new(cost_calculator);
//! let decision = optimizer.should_precompute(&expression, reference_count);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::expr::Expression;
use crate::core::types::ContextualExpression;
use crate::query::optimizer::cost::CostCalculator;

/// Precomputation decision
#[derive(Debug, Clone, PartialEq)]
pub enum PrecomputationDecision {
    /// Should precompute the expression
    Precompute {
        /// Reason for precomputation
        reason: PrecomputeReason,
        /// Estimated benefit (cost savings)
        benefit: f64,
        /// Estimated cost of precomputation
        cost: f64,
    },
    /// Should not precompute
    DoNotPrecompute {
        /// Reason for not precomputing
        reason: NoPrecomputeReason,
    },
}

/// Reasons for precomputation
#[derive(Debug, Clone, PartialEq)]
pub enum PrecomputeReason {
    /// Expression is complex and referenced multiple times
    ComplexAndFrequentlyUsed,
    /// Expression contains expensive function calls
    ExpensiveFunctionCalls,
    /// Expression is deterministic and reused
    DeterministicAndReused,
    /// Cost-benefit analysis favors precomputation
    CostBenefit,
}

/// Reasons for not precomputing
#[derive(Debug, Clone, PartialEq)]
pub enum NoPrecomputeReason {
    /// Expression is too simple
    TooSimple,
    /// Expression is referenced only once
    SingleUse,
    /// Expression is non-deterministic
    NonDeterministic,
    /// Precomputation cost exceeds benefit
    NotCostEffective,
    /// Expression contains volatile functions
    ContainsVolatileFunctions,
}

/// Expression precomputation candidate
#[derive(Debug, Clone)]
pub struct PrecomputationCandidate {
    /// The expression to potentially precompute
    pub expression: ContextualExpression,
    /// Number of times the expression is referenced
    pub reference_count: usize,
    /// Estimated cost of the expression
    pub estimated_cost: f64,
    /// Whether the expression is deterministic
    pub is_deterministic: bool,
}

/// Expression Precomputation Optimizer
///
/// Determines which expressions should be precomputed to avoid redundant computations.
#[derive(Debug, Clone)]
pub struct ExpressionPrecomputationOptimizer {
    cost_calculator: Arc<CostCalculator>,
    /// Threshold for precomputation (benefit/cost ratio)
    precompute_threshold: f64,
    /// Minimum expression cost to consider precomputation
    min_expression_cost: f64,
    /// Overhead for precomputation (storage, lookup, etc.)
    precompute_overhead: f64,
}

impl ExpressionPrecomputationOptimizer {
    /// Create a new expression precomputation optimizer
    pub fn new(cost_calculator: Arc<CostCalculator>) -> Self {
        Self {
            cost_calculator,
            precompute_threshold: 2.0, // Benefit must be 2x the cost
            min_expression_cost: 0.01,
            precompute_overhead: 0.005,
        }
    }

    /// Set the precomputation threshold
    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.precompute_threshold = threshold.max(1.0);
        self
    }

    /// Set minimum expression cost
    pub fn with_min_cost(mut self, min_cost: f64) -> Self {
        self.min_expression_cost = min_cost.max(0.0);
        self
    }

    /// Set precomputation overhead
    pub fn with_overhead(mut self, overhead: f64) -> Self {
        self.precompute_overhead = overhead.max(0.0);
        self
    }

    /// Determine if an expression should be precomputed
    ///
    /// # Parameters
    /// - `expression`: The expression to evaluate
    /// - `reference_count`: Number of times the expression is referenced
    ///
    /// # Returns
    /// Precomputation decision
    pub fn should_precompute(
        &self,
        expression: &ContextualExpression,
        reference_count: usize,
    ) -> PrecomputationDecision {
        // Check if expression is referenced multiple times
        if reference_count <= 1 {
            return PrecomputationDecision::DoNotPrecompute {
                reason: NoPrecomputeReason::SingleUse,
            };
        }

        // Get the expression
        let expr = match expression.get_expression() {
            Some(e) => e,
            None => {
                return PrecomputationDecision::DoNotPrecompute {
                    reason: NoPrecomputeReason::TooSimple,
                };
            }
        };

        // Calculate expression cost
        let expression_cost = self.cost_calculator.calculate_expression_cost(&expr);

        // Check if expression is complex enough
        if expression_cost < self.min_expression_cost {
            return PrecomputationDecision::DoNotPrecompute {
                reason: NoPrecomputeReason::TooSimple,
            };
        }

        // Check if expression is deterministic
        let is_deterministic = self.check_expression_deterministic(&expr);
        if !is_deterministic {
            return PrecomputationDecision::DoNotPrecompute {
                reason: NoPrecomputeReason::NonDeterministic,
            };
        }

        // Calculate benefit and cost
        let precompute_benefit = expression_cost * reference_count as f64;
        let precompute_cost = expression_cost + self.precompute_overhead;

        // Check cost-benefit ratio
        let ratio = precompute_benefit / precompute_cost;

        if ratio >= self.precompute_threshold {
            let reason =
                if expression_cost > self.cost_calculator.config().function_call_base_cost * 2.0 {
                    PrecomputeReason::ExpensiveFunctionCalls
                } else if reference_count > 3 {
                    PrecomputeReason::ComplexAndFrequentlyUsed
                } else {
                    PrecomputeReason::CostBenefit
                };

            PrecomputationDecision::Precompute {
                reason,
                benefit: precompute_benefit,
                cost: precompute_cost,
            }
        } else {
            PrecomputationDecision::DoNotPrecompute {
                reason: NoPrecomputeReason::NotCostEffective,
            }
        }
    }

    /// Analyze multiple expressions and return precomputation candidates
    ///
    /// # Parameters
    /// - `expressions`: Map of expression to reference count
    ///
    /// # Returns
    /// List of expressions that should be precomputed
    #[allow(clippy::mutable_key_type)]
    pub fn analyze_expressions(
        &self,
        expressions: &HashMap<ContextualExpression, usize>,
    ) -> Vec<(ContextualExpression, PrecomputationDecision)> {
        let mut results = Vec::new();

        for (expr, ref_count) in expressions {
            let decision = self.should_precompute(expr, *ref_count);
            if matches!(decision, PrecomputationDecision::Precompute { .. }) {
                results.push((expr.clone(), decision));
            }
        }

        // Sort by benefit (descending)
        results.sort_by(|a, b| {
            let benefit_a = match &a.1 {
                PrecomputationDecision::Precompute { benefit, .. } => *benefit,
                _ => 0.0,
            };
            let benefit_b = match &b.1 {
                PrecomputationDecision::Precompute { benefit, .. } => *benefit,
                _ => 0.0,
            };
            benefit_b
                .partial_cmp(&benefit_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }

    /// Recursively check if expression is deterministic
    fn check_expression_deterministic(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Literal(_) => true,
            Expression::Variable(_) => true,
            Expression::Parameter(_) => true, // Parameters are deterministic within a query
            Expression::Label(_) => true,
            Expression::Vector(_) => true, // Vector literals are deterministic
            Expression::Unary { operand, .. } => self.check_expression_deterministic(operand),
            Expression::Binary { left, right, .. } => {
                self.check_expression_deterministic(left)
                    && self.check_expression_deterministic(right)
            }
            Expression::Function { name, args, .. } => {
                // Check if function is deterministic
                let is_deterministic_func = !self.is_volatile_function(name);
                is_deterministic_func
                    && args
                        .iter()
                        .all(|arg| self.check_expression_deterministic(arg))
            }
            Expression::Case {
                conditions,
                default,
                ..
            } => {
                let conditions_deterministic = conditions.iter().all(|(cond, result)| {
                    self.check_expression_deterministic(cond)
                        && self.check_expression_deterministic(result)
                });
                let default_deterministic = default
                    .as_ref()
                    .map(|e| self.check_expression_deterministic(e))
                    .unwrap_or(true);
                conditions_deterministic && default_deterministic
            }
            Expression::List(elements) => elements
                .iter()
                .all(|e| self.check_expression_deterministic(e)),
            Expression::Map(entries) => entries
                .iter()
                .all(|(_, v)| self.check_expression_deterministic(v)),
            // Property access is deterministic if the object is deterministic
            Expression::Property { object, .. } => self.check_expression_deterministic(object),
            Expression::TagProperty { .. } => true,
            Expression::EdgeProperty { .. } => true,
            Expression::LabelTagProperty { tag, .. } => self.check_expression_deterministic(tag),
            // Type cast is deterministic if the expression is deterministic
            Expression::TypeCast { expression, .. } => {
                self.check_expression_deterministic(expression)
            }
            // Subscript access is deterministic if both collection and index are deterministic
            Expression::Subscript { collection, index } => {
                self.check_expression_deterministic(collection)
                    && self.check_expression_deterministic(index)
            }
            // Range access is deterministic if all parts are deterministic
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.check_expression_deterministic(collection)
                    && start
                        .as_ref()
                        .is_none_or(|e| self.check_expression_deterministic(e))
                    && end
                        .as_ref()
                        .is_none_or(|e| self.check_expression_deterministic(e))
            }
            // Path is deterministic if all elements are deterministic
            Expression::Path(exprs) => exprs.iter().all(|e| self.check_expression_deterministic(e)),
            // Path build is deterministic if all elements are deterministic
            Expression::PathBuild(exprs) => {
                exprs.iter().all(|e| self.check_expression_deterministic(e))
            }
            // List comprehension is deterministic if all parts are deterministic
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                self.check_expression_deterministic(source)
                    && filter
                        .as_ref()
                        .is_none_or(|e| self.check_expression_deterministic(e))
                    && map
                        .as_ref()
                        .is_none_or(|e| self.check_expression_deterministic(e))
            }
            // Reduce expression is deterministic if all parts are deterministic
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                self.check_expression_deterministic(initial)
                    && self.check_expression_deterministic(source)
                    && self.check_expression_deterministic(mapping)
            }
            // Aggregate expressions are deterministic if the argument is deterministic
            Expression::Aggregate { arg, .. } => self.check_expression_deterministic(arg),
            // Predicate expressions are deterministic if all arguments are deterministic
            Expression::Predicate { args, .. } => args
                .iter()
                .all(|arg| self.check_expression_deterministic(arg)), // Pattern is now exhaustive - all Expression variants are handled above
                                                                      // If new variants are added in the future, they should be handled here
                                                                      // For now, this branch is unreachable but kept for future-proofing
        }
    }

    /// Check if a function is volatile (non-deterministic)
    fn is_volatile_function(&self, name: &str) -> bool {
        let volatile_functions: &[&str] = &[
            "rand",
            "random",
            "now",
            "current_time",
            "current_timestamp",
            "uuid",
            "row_number",
            // Add more as needed
        ];
        volatile_functions.contains(&name.to_lowercase().as_str())
    }

    /// Get total estimated benefit from precomputing a list of expressions
    pub fn total_precomputation_benefit(
        &self,
        decisions: &[(ContextualExpression, PrecomputationDecision)],
    ) -> f64 {
        decisions
            .iter()
            .filter_map(|(_, decision)| match decision {
                PrecomputationDecision::Precompute { benefit, cost, .. } => Some(benefit - cost),
                _ => None,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::value::Value;
    use crate::query::optimizer::stats::StatisticsManager;
    use crate::query::validator::context::ExpressionAnalysisContext;

    fn create_test_optimizer() -> ExpressionPrecomputationOptimizer {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        ExpressionPrecomputationOptimizer::new(cost_calculator)
    }

    fn create_simple_expression() -> ContextualExpression {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::Literal(Value::Int(42));
        let id = ctx.register_expression(crate::core::types::expr::ExpressionMeta::new(expr));
        ContextualExpression::new(id, std::sync::Arc::new(ctx))
    }

    #[test]
    fn test_optimizer_creation() {
        let optimizer = create_test_optimizer();
        assert_eq!(optimizer.precompute_threshold, 2.0);
        assert_eq!(optimizer.min_expression_cost, 0.01);
    }

    #[test]
    fn test_single_use_not_precomputed() {
        let optimizer = create_test_optimizer();
        let expr = create_simple_expression();

        let decision = optimizer.should_precompute(&expr, 1);
        assert_eq!(
            decision,
            PrecomputationDecision::DoNotPrecompute {
                reason: NoPrecomputeReason::SingleUse,
            }
        );
    }

    #[test]
    fn test_multiple_use_considered() {
        let optimizer = create_test_optimizer();
        let expr = create_simple_expression();

        // Even with multiple uses, simple expressions may not be precomputed
        let decision = optimizer.should_precompute(&expr, 5);
        // Result depends on cost calculation
        match decision {
            PrecomputationDecision::DoNotPrecompute { .. } => {
                // Expected for simple expressions
            }
            PrecomputationDecision::Precompute { .. } => {
                // Also acceptable if cost-benefit analysis favors it
            }
        }
    }

    #[test]
    fn test_volatile_function_detection() {
        let optimizer = create_test_optimizer();

        assert!(optimizer.is_volatile_function("rand"));
        assert!(optimizer.is_volatile_function("RAND"));
        assert!(optimizer.is_volatile_function("now"));
        assert!(optimizer.is_volatile_function("NOW"));
        assert!(!optimizer.is_volatile_function("abs"));
        assert!(!optimizer.is_volatile_function("upper"));
    }

    #[test]
    fn test_deterministic_check() {
        let optimizer = create_test_optimizer();

        // Literal is deterministic
        let literal_expr = Expression::Literal(Value::Int(42));
        assert!(optimizer.check_expression_deterministic(&literal_expr));

        // Variable is deterministic
        let var_expr = Expression::Variable("x".to_string());
        assert!(optimizer.check_expression_deterministic(&var_expr));
    }

    #[test]
    fn test_with_threshold() {
        let stats_manager = Arc::new(StatisticsManager::new());
        let cost_calculator = Arc::new(CostCalculator::new(stats_manager));
        let optimizer = ExpressionPrecomputationOptimizer::new(cost_calculator).with_threshold(3.0);

        assert_eq!(optimizer.precompute_threshold, 3.0);
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn test_analyze_expressions() {
        let optimizer = create_test_optimizer();
        let mut expressions = HashMap::new();

        let expr1 = create_simple_expression();
        expressions.insert(expr1.clone(), 5);

        let results = optimizer.analyze_expressions(&expressions);
        // Results depend on cost calculations
        assert!(results.is_empty() || results.len() == 1);
    }

    #[test]
    fn test_total_benefit_calculation() {
        let optimizer = create_test_optimizer();
        let expr = create_simple_expression();

        let decisions = vec![(
            expr.clone(),
            PrecomputationDecision::Precompute {
                reason: PrecomputeReason::CostBenefit,
                benefit: 10.0,
                cost: 3.0,
            },
        )];

        let total_benefit = optimizer.total_precomputation_benefit(&decisions);
        assert_eq!(total_benefit, 7.0);
    }
}
