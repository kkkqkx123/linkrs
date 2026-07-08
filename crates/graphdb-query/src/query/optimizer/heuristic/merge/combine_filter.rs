//! Rules that combine multiple filtering operations

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;

/// Rules that combine multiple filtering operations
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(col2 > 200)
///       |
///   Filter(col1 > 100)
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   Filter(col1 > 100 AND col2 > 200)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is a Filter node.
/// The child node is also a Filter node.
/// The two filtering conditions can be combined.
#[derive(Debug)]
pub struct CombineFilterRule;

impl CombineFilterRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Merge two conditional expressions
    fn combine_conditions(&self, top: &Expression, child: &Expression) -> Expression {
        Expression::Binary {
            left: Box::new(child.clone()),
            op: crate::core::types::operators::BinaryOperator::And,
            right: Box::new(top.clone()),
        }
    }
}

impl Default for CombineFilterRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for CombineFilterRule {
    fn name(&self) -> &'static str {
        "CombineFilterRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("Filter")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Filter node.
        let top_filter = match node {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input node
        let input = top_filter.input();

        // Check whether the input node is also a Filter.
        let child_filter = match input {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };

        // Obtain two filtering criteria.
        let top_condition = top_filter.condition();
        let child_condition = child_filter.condition();

        // The obtained expression is used for merging.
        let top_expr = match top_condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        let child_expr = match child_condition.expression() {
            Some(meta) => meta.inner().clone(),
            None => return Ok(None),
        };

        // Merge conditions
        let combined_condition = self.combine_conditions(&top_expr, &child_expr);

        // Obtain the context.
        let ctx = top_condition.context().clone();

        // Create metadata for the merged expression.
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(combined_condition);
        let id = ctx.register_expression(expr_meta);
        let combined_ctx_expr = ContextualExpression::new(id, ctx);

        // Obtaining the input for the sub-filter
        let child_input = child_filter.input().clone();

        // Create a merged Filter node.
        let combined_filter_node = match FilterNode::new(child_input, combined_ctx_expr) {
            Ok(node) => node,
            Err(_) => return Ok(None),
        };

        // Create the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Filter(combined_filter_node));

        Ok(Some(result))
    }
}

impl MergeRule for CombineFilterRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_filter() && child.is_filter()
    }

    fn create_merged_node(
        &self,
        _ctx: &mut RewriteContext,
        parent: &PlanNodeEnum,
        _child: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(_ctx, parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Expression;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_rule_name() {
        let rule = CombineFilterRule::new();
        assert_eq!(rule.name(), "CombineFilterRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = CombineFilterRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_combine_filters() {
        let start = PlanNodeEnum::Start(StartNode::new());
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // Lower-level filter: col1 > 100
        let child_condition = Expression::Binary {
            left: Box::new(Expression::Variable("col1".to_string())),
            op: crate::core::types::operators::BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(crate::core::Value::Int(100))),
        };
        let child_expr_meta = crate::core::types::expr::ExpressionMeta::new(child_condition);
        let child_id = expr_ctx.register_expression(child_expr_meta);
        let child_ctx_expr = ContextualExpression::new(child_id, expr_ctx.clone());
        let child_filter =
            FilterNode::new(start, child_ctx_expr).expect("Failed to create FilterNode");
        let child_node = PlanNodeEnum::Filter(child_filter);

        // Upper-level filter: col2 > 200
        let top_condition = Expression::Binary {
            left: Box::new(Expression::Variable("col2".to_string())),
            op: crate::core::types::operators::BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(crate::core::Value::Int(200))),
        };
        let top_expr_meta = crate::core::types::expr::ExpressionMeta::new(top_condition);
        let top_id = expr_ctx.register_expression(top_expr_meta);
        let top_ctx_expr = ContextualExpression::new(top_id, expr_ctx);
        let top_filter =
            FilterNode::new(child_node.clone(), top_ctx_expr).expect("Failed to create FilterNode");
        let top_node = PlanNodeEnum::Filter(top_filter);

        // Application rules
        let rule = CombineFilterRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &top_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The merging of the two Filter nodes should be successful."
        );

        let transform_result = result.expect("Failed to apply rewrite rule");
        assert!(transform_result.erase_curr);
        assert_eq!(transform_result.new_nodes.len(), 1);
    }
}
