//! Push filter conditions into the preserved side of a left join.
//!
//! A left join emits NULL-extended rows for unmatched left inputs. Only
//! predicates on the left side can move below the join without changing
//! which rows survive; right-side predicates stay above it.

use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::expression_utils::{check_col_name, split_filter};
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use linkrs_core::types::ContextualExpression;
use linkrs_core::Expression;

/// Push left-side predicates below a left join.
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
///   Filter(b.col2 < 20)
///           |
///   LeftJoin
///   /          \
/// Filter      Right
/// (a.col1>10)
///   |
/// Left
/// ```
///
/// # Applicable Conditions
///
/// Only predicates whose columns belong to the preserved left input are
/// pushed. A predicate on the right input would turn unmatched rows into
/// matches once the upper filter is removed, so it remains above the join.
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

        // Obtain the column names for the preserved left input.
        let left_col_names = join.left_input().col_names().to_vec();

        // Only the preserved left side may absorb a predicate. Right-side
        // columns are NULL on unmatched rows; pushing such a predicate below
        // the join would change the filtering result.
        let left_picker = |expr: &Expression| -> bool { check_col_name(&left_col_names, expr) };
        let (left_picked, left_remained) = split_filter(filter_condition, left_picker);

        if left_picked.is_none() {
            return Ok(None);
        }

        // Create a new LeftJoin node. The right input stays unchanged.
        let mut new_join = join.clone();
        let mut new_left = join.left_input().clone();

        // Handle the push into the preserved left input.
        if let Some(left_filter) = left_picked {
            let left_expr = match left_filter.expression() {
                Some(meta) => meta.inner().clone(),
                None => return Ok(None),
            };
            let left_expr_meta = linkrs_core::types::expr::ExpressionMeta::new(left_expr);
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

        // Only the left input is narrowed; unmatched rows must still be
        // produced from the right input.
        new_join.set_left_input(new_left);

        // Construct the translation result.
        let mut result = TransformResult::new();

        if let Some(remained) = left_remained {
            result.erase_curr = false;
            let mut new_filter = filter_node.clone();
            let remained_expr = match remained.expression() {
                Some(meta) => meta.inner().clone(),
                None => return Ok(None),
            };
            let remained_meta = linkrs_core::types::ExpressionMeta::new(remained_expr);
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
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::planning::plan::core::nodes::join::join_node::LeftJoinNode;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
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
        let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(condition);
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

    #[test]
    fn right_predicate_stays_above_left_join() {
        let mut left = StartNode::new();
        left.set_col_names(vec!["a".to_string()]);
        let mut right = StartNode::new();
        right.set_col_names(vec!["b".to_string()]);
        let join = LeftJoinNode::new(
            PlanNodeEnum::Start(left),
            PlanNodeEnum::Start(right),
            vec![],
            vec![],
        )
        .expect("left join should build");

        let property = |name: &str| Expression::Property {
            object: Box::new(Expression::Variable(name.to_string())),
            property: name.to_string(),
        };
        let condition = Expression::Binary {
            op: linkrs_core::BinaryOperator::And,
            left: Box::new(property("a")),
            right: Box::new(property("b")),
        };
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(linkrs_core::types::expr::ExpressionMeta::new(condition));
        let filter = FilterNode::new(
            PlanNodeEnum::LeftJoin(join),
            ContextualExpression::new(id, ctx),
        )
        .expect("filter should build");

        let rule = PushFilterDownLeftJoinRule::new();
        let result = rule
            .apply(&mut RewriteContext::new(), &PlanNodeEnum::Filter(filter))
            .expect("rewrite should succeed")
            .expect("left predicate should be pushed");
        let rewritten = result
            .new_nodes
            .iter()
            .find_map(|node| match node {
                PlanNodeEnum::LeftJoin(join) => Some(join),
                _ => None,
            })
            .expect("rewritten left join should be present");

        assert!(matches!(rewritten.left_input(), PlanNodeEnum::Filter(_)));
        assert!(matches!(rewritten.right_input(), PlanNodeEnum::Start(_)));
        assert!(
            result
                .new_nodes
                .iter()
                .any(|node| matches!(node, PlanNodeEnum::Filter(_))),
            "right predicate must remain in an upper filter"
        );
    }
}
