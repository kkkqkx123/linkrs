//! Decorrelation heuristic rule
//!
//! Lightweight stat-free gate that converts simple, deterministic
//! `PatternApply` subqueries into `SemiJoin` / `AntiJoin` nodes during the
//! heuristic batch phase.
//!
//! The cost-based path (`SubqueryUnnestingOptimizer`) keeps the heavy
//! judgment: row-count estimates, selectivity correction, and cost
//! comparison. This rule only handles the provably-safe shapes, so the
//! CBO's `TooManyRows` / `TooComplex` decisions are never preempted by the
//! heuristic path.
//!
//! # Gate
//!
//! 1. Shape: the right input must be a single-table scan (vertex / edge /
//!    index) wrapped in equality filters and projections, optionally capped
//!    by a constant `Limit` (`is_simple_subquery_shape`).
//! 2. No aggregation anywhere in the subquery shape (aggregate subqueries
//!    are not equi semi joins).
//! 3. Determinism: all expressions in the subquery must be deterministic.

use crate::query::optimizer::analysis::BatchPlanAnalyzer;
use crate::query::optimizer::cost_based::subquery_unnesting::SubqueryUnnestingOptimizer;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::PlanNodeEnum;

/// Decorrelation rule for simple pattern-apply subqueries.
///
/// Converts `PatternApply` to `SemiJoin` / `AntiJoin` when the subquery
/// shape is provably safe without statistics.
#[derive(Debug)]
pub struct UnnestSimplePatternApplyRule;

impl UnnestSimplePatternApplyRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnnestSimplePatternApplyRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for UnnestSimplePatternApplyRule {
    fn name(&self) -> &'static str {
        "UnnestSimplePatternApplyRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("PatternApply")
    }

    fn apply(
        &self,
        _ctx: &mut crate::query::optimizer::heuristic::context::RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let apply = match node {
            PlanNodeEnum::PatternApply(n) => n,
            _ => return Ok(None),
        };

        // Shape gate: single-table scan wrapped in equality filters and
        // projections, optionally capped by a constant limit.
        let right = apply.right_input();
        if !SubqueryUnnestingOptimizer::is_simple_subquery_shape(right) {
            return Ok(None);
        }

        // Aggregated subqueries are not equi semi joins.
        if SubqueryUnnestingOptimizer::contains_aggregation(right) {
            return Ok(None);
        }

        // Determinism gate: non-deterministic expressions (e.g. random
        // functions) must not be unnest-ed by the heuristic path.
        let analysis = BatchPlanAnalyzer::new().analyze(right);
        if !analysis.expression_summary.is_fully_deterministic {
            return Ok(None);
        }

        match SubqueryUnnestingOptimizer::build_semi_join_from_pattern_apply(apply.clone()) {
            Ok(join) => {
                let mut result = TransformResult::new();
                result.add_new_node(join);
                Ok(Some(result))
            }
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::ContextualExpression;
    use crate::core::Expression;
    use crate::query::optimizer::heuristic::context::RewriteContext;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::query::planning::plan::core::nodes::PatternApplyNode;
    use crate::query::planning::plan::PlanNodeEnum;
    use std::sync::Arc;

    fn test_scan() -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "test");
        scan.set_tag("person");
        PlanNodeEnum::ScanVertices(scan)
    }

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    #[test]
    fn test_rule_name_and_pattern() {
        let rule = UnnestSimplePatternApplyRule::new();
        assert_eq!(rule.name(), "UnnestSimplePatternApplyRule");
        assert!(rule.pattern().node.is_some());
    }

    #[test]
    fn test_unnests_simple_equality_filter_subquery() {
        let condition = contextual(Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "age".to_string(),
            }),
            op: BinaryOperator::Equal,
            right: Box::new(Expression::Literal(crate::core::Value::Int(18))),
        });
        let filter = PlanNodeEnum::Filter(FilterNode::new(test_scan(), condition).expect("filter"));
        let pattern_apply = PlanNodeEnum::PatternApply(
            PatternApplyNode::new(test_scan(), filter, vec![], vec![], false)
                .expect("pattern apply should build"),
        );

        let rule = UnnestSimplePatternApplyRule::new();
        let result = rule
            .apply(&mut RewriteContext::new(), &pattern_apply)
            .expect("rewrite should succeed")
            .expect("simple subquery must unnest");
        match &result.new_nodes[0] {
            PlanNodeEnum::SemiJoin(join) => assert!(!join.is_anti()),
            other => panic!("expected SemiJoin, got {:?}", other.name()),
        }
    }

    #[test]
    fn test_keeps_anti_apply_as_anti_join() {
        let pattern_apply = PlanNodeEnum::PatternApply(
            PatternApplyNode::new(test_scan(), test_scan(), vec![], vec![], true)
                .expect("pattern apply should build"),
        );

        let rule = UnnestSimplePatternApplyRule::new();
        let result = rule
            .apply(&mut RewriteContext::new(), &pattern_apply)
            .expect("rewrite should succeed")
            .expect("anti subquery must unnest");
        match &result.new_nodes[0] {
            PlanNodeEnum::SemiJoin(join) => assert!(join.is_anti()),
            other => panic!("expected AntiJoin, got {:?}", other.name()),
        }
    }

    #[test]
    fn test_skips_non_pattern_apply_nodes() {
        let rule = UnnestSimplePatternApplyRule::new();
        let result = rule
            .apply(&mut RewriteContext::new(), &test_scan())
            .expect("rewrite should succeed");
        assert!(result.is_none());
    }
}
