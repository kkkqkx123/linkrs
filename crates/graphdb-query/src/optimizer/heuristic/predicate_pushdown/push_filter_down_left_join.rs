//! Rules that push the filtering conditions to the left hash join operation
//!
//! This rule identifies the Filter -> LeftJoin mode.
//! And push the filtering conditions to both sides of the connection.

use graphdb_core::types::ContextualExpression;
use graphdb_core::Expression;
use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::expression_utils::{check_col_name, split_filter};
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;

/// Rules that push the filtering conditions to the left hash join operation
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(a.col1 > 10 AND b.col2 < 20)
///           |
///   LeftJoin
///   /          \
/// Left      Right
/// ```
///
/// After:
/// ```text
///   LeftJoin
///   /          \
/// Filter      Filter
/// (a.col1>10) (b.col2<20)
///   |            |
/// Left        Right
/// ```
///
/// # Applicable Conditions
///
/// The filtering criteria can be separated into conditions on the left and right sides.
/// The conditions can be safely pushed out to both sides.
#[derive(Debug)]
pub struct PushFilterDownLeftJoinRule;

impl PushFilterDownLeftJoinRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownLeftJoinRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownLeftJoinRule {
    fn name(&self) -> &'static str {
        "PushFilterDownLeftJoinRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("LeftJoin")
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

        // Check whether the input node is a LeftJoin.
        let join = match input {
            PlanNodeEnum::LeftJoin(n) => n,
            _ => return Ok(None),
        };

        // Obtain the filtering criteria
        let filter_condition = filter_node.condition();

        // Obtaining the context is necessary for creating a ContextualExpression.
        let ctx = filter_condition.context().clone();

        // Obtain the column names for the left and right inputs.
        let left_col_names = join.left_input().col_names().to_vec();
        let right_col_names = join.right_input().col_names().to_vec();

        // Define the function for the left-side selector.
        let left_picker = |expr: &Expression| -> bool { check_col_name(&left_col_names, expr) };

        // Define the function for the selector on the right side.
        let right_picker = |expr: &Expression| -> bool { check_col_name(&right_col_names, expr) };

        // Split filter criteria
        let (left_picked, left_remained) = split_filter(filter_condition, left_picker);
        let (right_picked, right_remained) = split_filter(filter_condition, right_picker);

        // If there are no conditions that allow for the transformation to be performed, then no conversion will take place.
        if left_picked.is_none() && right_picked.is_none() {
            return Ok(None);
        }

        // Create a new LeftJoin node.
        let mut new_join = join.clone();
        let mut new_left = join.left_input().clone();
        let mut new_right = join.right_input().clone();

        // Handle the push from the lower left side.
        let left_pushed = left_picked.is_some();
        if let Some(left_filter) = left_picked {
            let left_expr = match left_filter.expression() {
                Some(meta) => meta.inner().clone(),
                None => return Ok(None),
            };
            let left_expr_meta = graphdb_core::types::expr::ExpressionMeta::new(left_expr);
            let left_id = ctx.register_expression(left_expr_meta);
            let left_ctx_expr = ContextualExpression::new(left_id, ctx.clone());
            let left_filter_node = FilterNode::new(new_left, left_ctx_expr).map_err(|e| {
                crate::optimizer::heuristic::result::RewriteError::rewrite_failed(format!(
                    "Failed to create FilterNode: {:?}",
                    e
                ))
            })?;
            new_left = PlanNodeEnum::Filter(left_filter_node);
        }

        // Handle the push from the lower right corner.
        let right_pushed = right_picked.is_some();
        if let Some(right_filter) = right_picked {
            let right_expr = match right_filter.expression() {
                Some(meta) => meta.inner().clone(),
                None => return Ok(None),
            };
            let right_expr_meta = graphdb_core::types::expr::ExpressionMeta::new(right_expr);
            let right_id = ctx.register_expression(right_expr_meta);
            let right_ctx_expr = ContextualExpression::new(right_id, ctx.clone());
            let right_filter_node = FilterNode::new(new_right, right_ctx_expr).map_err(|e| {
                crate::optimizer::heuristic::result::RewriteError::rewrite_failed(format!(
                    "Failed to create FilterNode: {:?}",
                    e
                ))
            })?;
            new_right = PlanNodeEnum::Filter(right_filter_node);
        }

        // Update the input for the Join node.
        new_join.set_left_input(new_left);
        new_join.set_right_input(new_right);

        // Construct the translation result.
        let mut result = TransformResult::new();

        // Check whether there are any remaining filter conditions.
        let remaining_condition = if left_pushed && right_pushed {
            None
        } else if left_pushed {
            right_remained
        } else {
            left_remained
        };

        if let Some(remained) = remaining_condition {
            result.erase_curr = false;
            let mut new_filter = filter_node.clone();
            let remained_expr = match remained.expression() {
                Some(meta) => meta.inner().clone(),
                None => return Ok(None),
            };
            let remained_meta = graphdb_core::types::ExpressionMeta::new(remained_expr);
            let remained_id = ctx.register_expression(remained_meta);
            let remained_ctx_expr = ContextualExpression::new(remained_id, ctx);
            new_filter.set_condition(remained_ctx_expr);
            result.add_new_node(PlanNodeEnum::Filter(new_filter));
        } else {
            result.erase_curr = true;
        }

        result.add_new_node(PlanNodeEnum::LeftJoin(new_join));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownLeftJoinRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(
            (node, target),
            (PlanNodeEnum::Filter(_), PlanNodeEnum::LeftJoin(_))
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
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::join::join_node::LeftJoinNode;
    use std::sync::Arc;

    #[test]
    fn test_rule_name() {
        let rule = PushFilterDownLeftJoinRule::new();
        assert_eq!(rule.name(), "PushFilterDownLeftJoinRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownLeftJoinRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_can_push_down() {
        let rule = PushFilterDownLeftJoinRule::new();

        let start = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start);

        let condition = Expression::Variable("test".to_string());
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(condition);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        let filter =
            FilterNode::new(start_enum.clone(), ctx_expr).expect("Failed to create FilterNode");
        let filter_enum = PlanNodeEnum::Filter(filter);

        let join = LeftJoinNode::new(start_enum.clone(), start_enum, vec![], vec![])
            .expect("Failed to create LeftJoinNode");
        let join_enum = PlanNodeEnum::LeftJoin(join);

        assert!(rule.can_push_down(&filter_enum, &join_enum));
    }
}
