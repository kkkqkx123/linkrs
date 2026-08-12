//! Subquery decorrelation optimization module
//!
//! "Analysis-based subquery decorrelation optimization strategy" — This
//! strategy converts simple PatternApply subqueries into SemiJoin/AntiJoin
//! operations (semi-join unnesting). PatternApply is the correlated
//! subquery operator: the left input is the main pipeline, the right input
//! is the per-row subquery. Unnesting replaces the apply (per-row
//! execution) with a single build/probe pass over both inputs.
//!
//! ## Optimization Strategies
//!
//! Convert the eligible PatternApply subquery into a SemiJoin (EXISTS) or
//! AntiJoin (NOT EXISTS) node. Avoid executing subqueries repeatedly.
//!
//! ## Applicable Conditions
//!
//! 1. The right input for PatternApply is a simple query (single-table
//!    scan + equality filtering, optionally wrapped in a constant Limit).
//! 2. The filtering conditions are deterministic (excluding rand(), now(),
//!    etc.)
//! 3. The complexity of the expressions should be less than 50 (avoid using
//!    complex expressions).
//! 4. The subquery estimates that the number of rows is less than 1000
//!    (based on statistical information).
//!
//! ## Usage Examples
//!
//! ```rust
//! use graphdb::query::optimizer::strategy::SubqueryUnnestingOptimizer;
//! use graphdb::query::optimizer::OptimizerEngine;
//!
//! let optimizer = SubqueryUnnestingOptimizer::new();
//! let decision = optimizer.should_unnest(&pattern_apply, &analysis, &stats_view, &selectivity);
//! ```

use crate::core::types::operators::BinaryOperator;
use crate::core::Expression;
use crate::query::optimizer::analysis::BatchPlanAnalysis;
use crate::query::optimizer::cost::SelectivityEstimator;
use crate::query::optimizer::cost_based::row_estimates::estimate_node_output_rows_corrected;
use crate::query::optimizer::stats::feedback::cardinality::CardinalityFeedbackManager;
use crate::query::optimizer::stats::feedback::decision::DecorrelationAdvice;
use crate::query::optimizer::stats::StatsView;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::{PatternApplyNode, SemiJoinNode};

/// Maximum constant row bound accepted by [`SubqueryUnnestingOptimizer::has_bounded_input`]
/// for heuristic decorrelation without statistics.
pub const MAX_BOUNDED_SUBQUERY_ROWS: i64 = 1000;

/// Decentralized decision-making using subqueries
#[derive(Debug, Clone, PartialEq)]
pub enum UnnestDecision {
    /// Convert to SemiJoin/AntiJoin
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
    /// Measured execution feedback prefers the unnested hash path.
    Empirical,
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
    /// The subquery contains an aggregation; semi-join unnesting does not
    /// apply (aggregation over a correlated set is not an equi semi join).
    AggregateSubquery,
    /// Measured execution feedback prefers keeping the nested-loop path.
    EmpiricalKeep,
}

/// Subquery decorrelation optimizer
///
/// Based on batch plan analysis and statistical information, a decision is
/// made as to whether to convert PatternApply to SemiJoin/AntiJoin.
#[derive(Debug, Clone)]
pub struct SubqueryUnnestingOptimizer {
    /// The maximum number of estimated rows allowed for a subquery
    max_subquery_rows: u64,
    /// The maximum allowable complexity of the expression
    max_complexity: u32,
}

impl Default for SubqueryUnnestingOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl SubqueryUnnestingOptimizer {
    /// Create a new optimizer.
    pub fn new() -> Self {
        Self {
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

    /// Determine whether decorrelation should be performed.
    ///
    /// # Parameters
    /// `pattern_apply`: The PatternApply node
    /// `analysis`: The batch plan analysis result
    /// `stats`: The space-scoped statistics for the query
    /// `selectivity`: The selectivity estimator used for filter row estimates
    /// `cardinality`: The learned per-shape cardinality corrections (may be
    ///   consulted to correct the row estimates of both inputs)
    /// `advice`: Measured execution feedback of both decorrelation paths
    ///
    /// # Decision
    /// 1. Empirical feedback: when past executions of the same shape have
    ///    measured one path clearly faster, follow the measurement.
    /// 2. Otherwise compare corrected cost estimates.
    pub fn should_unnest(
        &self,
        pattern_apply: &PatternApplyNode,
        analysis: &BatchPlanAnalysis,
        stats: &StatsView,
        selectivity: &SelectivityEstimator,
        cardinality: &CardinalityFeedbackManager,
        advice: &DecorrelationAdvice,
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

        // 3. Checking subqueries for simplicity (shape)
        if !Self::is_simple_subquery_shape(pattern_apply.right_input()) {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::TooComplex,
            };
        }

        // 3b. Aggregated subqueries are not equi semi joins; keep the apply.
        if Self::contains_aggregation(pattern_apply.right_input()) {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::AggregateSubquery,
            };
        }

        // 3c. Empirical feedback: measured execution evidence overrides the
        // static cost comparison once enough runs have been observed.
        if advice.prefer_unnest && advice.confidence >= 0.7 {
            return UnnestDecision::ShouldUnnest {
                reason: UnnestReason::Empirical,
                original_cost: 0.0,
                unnested_cost: 0.0,
            };
        }
        if advice.prefer_keep && advice.confidence >= 0.7 {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::EmpiricalKeep,
            };
        }

        // 4. Checking the number of estimated rows for subqueries
        let estimated_rows = self.estimate_subquery_rows(
            pattern_apply.right_input(),
            stats,
            selectivity,
            cardinality,
        );
        if estimated_rows > self.max_subquery_rows {
            return UnnestDecision::KeepPatternApply {
                reason: KeepReason::TooManyRows,
            };
        }

        // 5. Comparison of costs
        let left_rows = estimate_node_output_rows_corrected(
            pattern_apply.left_input(),
            stats,
            selectivity,
            cardinality,
        )
        .max(1) as f64;
        let original_cost =
            self.estimate_pattern_apply_cost(left_rows, estimated_rows, advice.apply_cost_per_row);
        let unnested_cost = self.estimate_hash_join_cost(left_rows, estimated_rows);

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

    /// Check whether the subquery has a decorrelatable shape: a scan of a
    /// single table (vertex/edge/index) wrapped in equality filters and
    /// projections, optionally capped by a constant Limit.
    ///
    /// Shared with the heuristic decorrelation rule (stat-free shape gate).
    pub fn is_simple_subquery_shape(node: &PlanNodeEnum) -> bool {
        match node {
            // Single table scans
            PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::IndexScan(_) => true,

            // Constant-limit cap keeps the subquery bounded without
            // statistics; non-constant limits (parameter/expression based)
            // are rejected.
            PlanNodeEnum::Limit(n) => {
                let count = n.count();
                (0..=MAX_BOUNDED_SUBQUERY_ROWS).contains(&count)
                    && Self::is_simple_subquery_shape(SingleInputNode::input(n))
            }

            // Simple filtration
            PlanNodeEnum::Filter(n) => {
                let condition = n.condition();
                if let Some(expr_meta) = condition.expression() {
                    if !Self::is_simple_equality_condition(expr_meta.inner()) {
                        return false;
                    }
                }
                Self::is_simple_subquery_shape(SingleInputNode::input(n))
            }

            // Simple projection
            PlanNodeEnum::Project(n) => Self::is_simple_subquery_shape(SingleInputNode::input(n)),

            // Not supported in other cases
            _ => false,
        }
    }

    /// Check whether the subquery input is provably bounded: a plain leaf
    /// scan or a constant `Limit` wrapper with a literal count within
    /// [`MAX_BOUNDED_SUBQUERY_ROWS`].
    pub fn has_bounded_input(node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::IndexScan(_) => true,
            PlanNodeEnum::Limit(n) => {
                let count = n.count();
                (0..=MAX_BOUNDED_SUBQUERY_ROWS).contains(&count)
            }
            _ => false,
        }
    }

    /// Whether the subquery contains an aggregation anywhere in its shape.
    ///
    /// Shared with the heuristic decorrelation rule (stat-free shape gate).
    pub fn contains_aggregation(node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Aggregate(_) | PlanNodeEnum::Window(_) => true,
            _ => node.children().into_iter().any(Self::contains_aggregation),
        }
    }

    /// Check whether the condition is a simple equality comparison.
    fn is_simple_equality_condition(expr: &Expression) -> bool {
        match expr {
            Expression::Binary { op, left, right } => match op {
                BinaryOperator::Equal => {
                    Self::is_simple_expression(left.as_ref())
                        && Self::is_simple_expression(right.as_ref())
                }
                BinaryOperator::And => {
                    Self::is_simple_equality_condition(left.as_ref())
                        && Self::is_simple_equality_condition(right.as_ref())
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// Check whether the expression is simple (consisting of literals,
    /// variables, or properties).
    fn is_simple_expression(expr: &Expression) -> bool {
        matches!(
            expr,
            Expression::Literal(_) | Expression::Variable(_) | Expression::Property { .. }
        )
    }

    /// Estimating the number of rows returned by a subquery
    ///
    /// Delegates to the shared cost-based estimator: leaf scans use the tag
    /// statistics, filters apply `SelectivityEstimator::estimate_from_expression`
    /// (EWMA-corrected), limits cap the input, and learned per-shape
    /// cardinality corrections are applied.
    fn estimate_subquery_rows(
        &self,
        node: &PlanNodeEnum,
        stats: &StatsView,
        selectivity: &SelectivityEstimator,
        cardinality: &CardinalityFeedbackManager,
    ) -> u64 {
        estimate_node_output_rows_corrected(node, stats, selectivity, cardinality)
    }

    /// Estimate the cost of applying the PatternApply method
    ///
    /// `measured_per_row_cost` replaces the fixed subquery-startup coefficient
    /// when execution feedback has measured the nested-loop path.
    fn estimate_pattern_apply_cost(
        &self,
        left_rows: f64,
        subquery_rows: u64,
        measured_per_row_cost: Option<f64>,
    ) -> f64 {
        // Nested loops that execute the subquery once per left row; the
        // default coefficient models subquery startup + execution, a measured
        // value (us per row) is used when feedback is available.
        match measured_per_row_cost {
            Some(cost) => left_rows * cost * 0.001,
            None => left_rows * (subquery_rows as f64 * 0.1),
        }
    }

    /// Estimating the cost of a HashJoin operation
    fn estimate_hash_join_cost(&self, left_rows: f64, subquery_rows: u64) -> f64 {
        // Simplified estimation: hash connections
        let right_rows = subquery_rows as f64;

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
    /// The transformed SemiJoin (EXISTS) / AntiJoin (NOT EXISTS) node
    pub fn unnest(
        &self,
        pattern_apply: PatternApplyNode,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        Self::build_semi_join_from_pattern_apply(pattern_apply)
    }

    /// Shared transformation: PatternApply → SemiJoin / AntiJoin.
    ///
    /// PatternApply has semi-join semantics: it keeps the left rows that
    /// have (EXISTS) / do not have (NOT EXISTS) a matching right row. The
    /// old InnerJoin conversion was unsound for both cases (duplicated left
    /// rows and leaked right columns).
    ///
    /// The keys are a direct passthrough: `PatternApplyNode` already carries
    /// per-side key expressions (`hash_keys` for the left/outer layout,
    /// `probe_keys` for the right/subquery layout), matching the
    /// `SemiJoinNode` key convention.
    pub fn build_semi_join_from_pattern_apply(
        pattern_apply: PatternApplyNode,
    ) -> Result<PlanNodeEnum, crate::query::planning::planner::PlannerError> {
        let left_input = pattern_apply.left_input().clone();
        let right_input = pattern_apply.right_input().clone();

        let join_node = if pattern_apply.is_anti_predicate() {
            SemiJoinNode::new_anti(
                left_input,
                right_input,
                pattern_apply.hash_keys().to_vec(),
                pattern_apply.probe_keys().to_vec(),
            )?
        } else {
            SemiJoinNode::new_semi(
                left_input,
                right_input,
                pattern_apply.hash_keys().to_vec(),
                pattern_apply.probe_keys().to_vec(),
            )?
        };

        Ok(PlanNodeEnum::SemiJoin(join_node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::ContextualExpression;
    use crate::query::optimizer::analysis::BatchPlanAnalyzer;
    use crate::query::optimizer::stats::StatisticsManager;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::operation::sort_node::LimitNode;
    use crate::query::planning::plan::core::nodes::PlanNodeEnum;
    use std::sync::Arc;

    fn test_scan() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        PlanNodeEnum::ScanVertices(scan)
    }

    fn test_selectivity() -> SelectivityEstimator {
        SelectivityEstimator::new(std::sync::Arc::new(StatisticsManager::new()))
    }

    #[test]
    fn test_optimizer_creation() {
        let optimizer = SubqueryUnnestingOptimizer::new();
        assert_eq!(optimizer.max_subquery_rows, 1000);
        assert_eq!(optimizer.max_complexity, 50);
    }

    #[test]
    fn test_optimizer_with_config() {
        let _optimizer = SubqueryUnnestingOptimizer::new()
            .with_max_rows(500)
            .with_max_complexity(30);
    }

    #[test]
    fn test_simple_expression_check() {
        let literal = Expression::Literal(crate::core::Value::Int(42));
        assert!(SubqueryUnnestingOptimizer::is_simple_expression(&literal));

        let variable = Expression::Variable("n".to_string());
        assert!(SubqueryUnnestingOptimizer::is_simple_expression(&variable));

        let property = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        assert!(SubqueryUnnestingOptimizer::is_simple_expression(&property));

        let binary = Expression::Binary {
            left: Box::new(Expression::Literal(crate::core::Value::Int(1))),
            op: crate::core::types::operators::BinaryOperator::Add,
            right: Box::new(Expression::Literal(crate::core::Value::Int(2))),
        };
        assert!(!SubqueryUnnestingOptimizer::is_simple_expression(&binary));
    }

    #[test]
    fn test_shape_accepts_scans_and_limits() {
        assert!(SubqueryUnnestingOptimizer::is_simple_subquery_shape(
            &test_scan()
        ));

        let limit = LimitNode::new(test_scan(), 0, 10).expect("limit should build");
        assert!(SubqueryUnnestingOptimizer::is_simple_subquery_shape(
            &PlanNodeEnum::Limit(limit)
        ));
        assert!(SubqueryUnnestingOptimizer::has_bounded_input(
            &PlanNodeEnum::Limit(LimitNode::new(test_scan(), 0, 10).expect("limit should build"))
        ));

        let large = LimitNode::new(test_scan(), 0, 5000).expect("limit should build");
        assert!(!SubqueryUnnestingOptimizer::has_bounded_input(
            &PlanNodeEnum::Limit(large)
        ));
    }

    #[test]
    fn test_shape_rejects_aggregation() {
        use crate::query::planning::plan::core::nodes::AggregateNode;
        let aggregate =
            AggregateNode::new(test_scan(), vec![], vec![]).expect("aggregate should build");
        assert!(SubqueryUnnestingOptimizer::contains_aggregation(
            &PlanNodeEnum::Aggregate(aggregate)
        ));
    }

    #[test]
    fn test_simple_subquery_unnests() {
        let scan = test_scan();
        let condition = {
            let ctx = Arc::new(ExpressionAnalysisContext::new());
            let expr = Expression::Binary {
                left: Box::new(Expression::Property {
                    object: Box::new(Expression::Variable("n".to_string())),
                    property: "age".to_string(),
                }),
                op: BinaryOperator::Equal,
                right: Box::new(Expression::Literal(crate::core::Value::Int(18))),
            };
            let id = ctx.register_expression(ExpressionMeta::new(expr));
            ContextualExpression::new(id, ctx)
        };
        let filter = PlanNodeEnum::Filter(FilterNode::new(scan, condition).expect("filter"));

        let pattern_apply = PatternApplyNode::new(test_scan(), filter, vec![], vec![], false)
            .expect("pattern apply should build");

        let analysis =
            BatchPlanAnalyzer::new().analyze(&PlanNodeEnum::PatternApply(pattern_apply.clone()));
        let selectivity = test_selectivity();
        let stats_manager = StatisticsManager::new();
        let stats = StatsView::new(&stats_manager, Some("test"));
        let decision = SubqueryUnnestingOptimizer::new().should_unnest(
            &pattern_apply,
            &analysis,
            &stats,
            &selectivity,
            &CardinalityFeedbackManager::new(),
            &DecorrelationAdvice::default(),
        );
        assert!(
            matches!(decision, UnnestDecision::ShouldUnnest { .. }),
            "a deterministic single-scan equality-filtered subquery must unnest"
        );
    }

    #[test]
    fn test_unnest_produces_semi_join_with_split_keys() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let hash_key_expr = Expression::Property {
            object: Box::new(Expression::Variable("t".to_string())),
            property: "city".to_string(),
        };
        let probe_key_expr = Expression::Property {
            object: Box::new(Expression::Variable("p".to_string())),
            property: "city".to_string(),
        };
        let hash_key_id = ctx.register_expression(ExpressionMeta::new(hash_key_expr));
        let hash_key = ContextualExpression::new(hash_key_id, ctx.clone());
        let probe_key_id = ctx.register_expression(ExpressionMeta::new(probe_key_expr));
        let probe_key = ContextualExpression::new(probe_key_id, ctx);

        let pattern_apply = PatternApplyNode::new(
            test_scan(),
            test_scan(),
            vec![hash_key],
            vec![probe_key],
            false,
        )
        .expect("pattern apply should build");

        let transformed = SubqueryUnnestingOptimizer::new()
            .unnest(pattern_apply)
            .expect("unnest should succeed");
        match &transformed {
            PlanNodeEnum::SemiJoin(join) => {
                assert!(!join.is_anti());
                assert_eq!(join.hash_keys().len(), 1);
                assert_eq!(join.probe_keys().len(), 1);
                let hash_meta = join.hash_keys()[0].expression().expect("hash key meta");
                let probe_meta = join.probe_keys()[0].expression().expect("probe key meta");
                assert_eq!(
                    hash_meta.inner(),
                    &Expression::Property {
                        object: Box::new(Expression::Variable("t".to_string())),
                        property: "city".to_string(),
                    }
                );
                assert_eq!(
                    probe_meta.inner(),
                    &Expression::Property {
                        object: Box::new(Expression::Variable("p".to_string())),
                        property: "city".to_string(),
                    }
                );
            }
            _ => panic!("expected SemiJoin, got {:?}", transformed.name()),
        }
    }

    #[test]
    fn test_unnest_anti_produces_anti_join() {
        let pattern_apply =
            PatternApplyNode::new(test_scan(), test_scan(), vec![], vec![], true)
                .expect("pattern apply should build");

        let transformed = SubqueryUnnestingOptimizer::new()
            .unnest(pattern_apply)
            .expect("unnest should succeed");
        match &transformed {
            PlanNodeEnum::SemiJoin(join) => assert!(join.is_anti()),
            _ => panic!("expected AntiJoin, got {:?}", transformed.name()),
        }
    }
}
