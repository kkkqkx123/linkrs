//! Rule that pushes pushable filter conjuncts onto the ScanVertices node as
//! its vertex filter.
//!
//! The scan's vertex filter is converted into storage-side [`ScanPredicate`]s
//! at plan assembly time, so the filtering happens inside the storage scan.
//! The `Filter` node itself is kept unchanged — the pushed predicate is a
//! pure pre-filter and the full condition still runs on top of the scan, so
//! the rewrite can never change results.

use crate::core::types::ContextualExpression;
use crate::core::types::ExpressionMeta;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::scan_predicate::{and_of, pushable_conjuncts};

/// Rule that pushes filter conjuncts into the scan node's vertex filter.
#[derive(Debug)]
pub struct PushFilterDownScanVerticesRule;

impl PushFilterDownScanVerticesRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownScanVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownScanVerticesRule {
    fn name(&self) -> &'static str {
        "PushFilterDownScanVerticesRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("ScanVertices")
    }

    fn apply(
        &self,
        _ctx: &mut crate::query::optimizer::heuristic::context::RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let filter = match node {
            PlanNodeEnum::Filter(f) => f,
            _ => return Ok(None),
        };
        let scan = match filter.input() {
            PlanNodeEnum::ScanVertices(s) => s,
            _ => return Ok(None),
        };
        if scan.vertex_filter().is_some() {
            return Ok(None);
        }

        let condition = filter.condition();
        let Some(meta) = condition.expression() else {
            return Ok(None);
        };
        let conjuncts = pushable_conjuncts(meta.inner());
        if conjuncts.is_empty() {
            return Ok(None);
        }
        let Some(pushed_expr) = and_of(conjuncts) else {
            return Ok(None);
        };

        let ctx = condition.context().clone();
        let pushed_id = ctx.register_expression(ExpressionMeta::new(pushed_expr));
        let pushed_filter = ContextualExpression::new(pushed_id, ctx);

        let mut new_scan = scan.clone();
        new_scan.set_vertex_filter(pushed_filter);

        let mut new_filter = filter.clone();
        new_filter.set_input(PlanNodeEnum::ScanVertices(new_scan));

        let mut result = TransformResult::new();
        result.add_new_node(PlanNodeEnum::Filter(new_filter));

        Ok(Some(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::Expression;
    use crate::core::Value;

    fn scan_node() -> crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode {
        crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode::new(
            0, "test",
        )
    }

    fn filter_node(
        condition: crate::core::types::ContextualExpression,
        input: PlanNodeEnum,
    ) -> PlanNodeEnum {
        PlanNodeEnum::Filter(
            crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode::new(
                input, condition,
            )
            .expect("filter node"),
        )
    }

    fn contextual(expr: Expression) -> crate::core::types::ContextualExpression {
        let ctx = std::sync::Arc::new(crate::core::types::expr::ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn prop(name: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable("v".to_string())),
            property: name.to_string(),
        }
    }

    #[test]
    fn pushes_comparison_conjuncts_onto_scan() {
        let expr = Expression::Binary {
            left: Box::new(prop("age")),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(18))),
        };
        let node = filter_node(contextual(expr), PlanNodeEnum::ScanVertices(scan_node()));

        let rule = PushFilterDownScanVerticesRule::new();
        let result = rule
            .apply(&mut crate::query::optimizer::heuristic::context::RewriteContext::new(), &node)
            .expect("rewrite");
        let result = result.expect("some result");
        let new_node = result.new_nodes.first().expect("node");

        let PlanNodeEnum::Filter(filter) = new_node else {
            panic!("expected filter");
        };
        let PlanNodeEnum::ScanVertices(scan) = filter.input() else {
            panic!("expected scan");
        };
        assert!(scan.vertex_filter().is_some());
    }

    #[test]
    fn skips_non_scan_inputs_and_unpushable_conditions() {
        let rule = PushFilterDownScanVerticesRule::new();
        let ctx = &mut crate::query::optimizer::heuristic::context::RewriteContext::new();

        let not_equal = Expression::Binary {
            left: Box::new(prop("name")),
            op: BinaryOperator::NotEqual,
            right: Box::new(Expression::Literal(Value::string("bob"))),
        };
        let node = filter_node(contextual(not_equal), PlanNodeEnum::ScanVertices(scan_node()));
        assert!(rule.apply(ctx, &node).expect("rewrite").is_none());

        let non_scan_input = filter_node(contextual(prop("x")), PlanNodeEnum::Start(
            crate::query::planning::plan::core::nodes::StartNode::default(),
        ));
        assert!(rule.apply(ctx, &non_scan_input).expect("rewrite").is_none());
    }
}
