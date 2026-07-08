//! Rules that push the filtering conditions to the GetNeighbors operation
//!
//! This rule identifies the Filter -> GetNeighbors mode.
//! And push the filtering conditions to the GetNeighbors node.

use crate::core::types::expr::ExpressionMeta;
use crate::core::types::operators::BinaryOperator;
use crate::core::types::ContextualExpression;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Rules that push the filtering conditions upstream to the GetNeighbors operation
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(e.likeness > 78)
///           |
///   GetNeighbors
/// ```
///
/// After:
/// ```text
///   GetNeighbors(filter: e.likeness > 78)
/// ```
///
/// # Applicable Conditions
///
/// The GetNeighbors node retrieves the properties of the edges.
/// The filtering criteria can be pushed down to the storage layer.
#[derive(Debug)]
pub struct PushFilterDownGetNbrsRule;

impl PushFilterDownGetNbrsRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownGetNbrsRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownGetNbrsRule {
    fn name(&self) -> &'static str {
        "PushFilterDownGetNbrsRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("GetNeighbors")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Filter node.
        let filter_node = match node {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input node
        let input = filter_node.input();

        // Check whether the input node is 'GetNeighbors'.
        let get_nbrs = match input {
            PlanNodeEnum::GetNeighbors(n) => n,
            _ => return Ok(None),
        };

        // Check whether GetNeighbors retrieves the edge attributes.
        let edge_props = get_nbrs.edge_props();
        if edge_props.is_empty() {
            return Ok(None);
        }

        // Obtain the filtering criteria
        let condition = filter_node.condition();

        // Obtaining the underlying Expression
        let expr = match condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Create a new GetNeighbors node.
        let mut new_get_nbrs = get_nbrs.clone();

        // Merge the existing filter conditions – Use the Expression::And to combine the expressions.
        let combined_expr = if let Some(existing_ctx) = get_nbrs.expression() {
            if let Some(existing_expr) = existing_ctx.get_expression() {
                Expression::Binary {
                    op: BinaryOperator::And,
                    left: Box::new(existing_expr.clone()),
                    right: Box::new(expr),
                }
            } else {
                expr
            }
        } else {
            expr
        };

        // Retrieve the context from the condition of the filter node, and register the new combined expression.
        let context = condition.context().clone();
        let new_meta = ExpressionMeta::new(combined_expr);
        let new_id = context.register_expression(new_meta);
        let new_filter = ContextualExpression::new(new_id, context);
        new_get_nbrs.set_expression(new_filter);

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::GetNeighbors(new_get_nbrs));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownGetNbrsRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(
            (node, target),
            (PlanNodeEnum::Filter(_), PlanNodeEnum::GetNeighbors(_))
        )
    }

    fn push_down(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
        _target: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetNeighborsNode;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownGetNbrsRule::new();
        assert_eq!(rule.name(), "PushFilterDownGetNbrsRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownGetNbrsRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_can_push_down() {
        let rule = PushFilterDownGetNbrsRule::new();
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        let start = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let condition = Expression::Variable("test".to_string());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(condition);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        let filter =
            FilterNode::new(start_enum.clone(), ctx_expr).expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        let get_nbrs = GetNeighborsNode::new(1, "v");
        let get_nbrs_enum = PlanNodeEnum::GetNeighbors(get_nbrs);

        assert!(rule.can_push_down(&filter_enum, &get_nbrs_enum));
    }
}
