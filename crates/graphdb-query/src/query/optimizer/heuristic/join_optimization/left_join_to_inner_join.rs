//! Rules for converting LeftJoin to InnerJoin
//!
//! When the right table column has a non-NULL filter condition, LeftJoin can be converted to InnerJoin.
//! This optimization reduces unnecessary NULL row processing.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Filter(b.col IS NOT NULL)
//!           |
//!   LeftJoin(A, B)
//! ```
//!
//! After:
//! ```text
//!   InnerJoin(A, B)
//! ```
//!
//! # Applicable Conditions
//!
//! The LeftJoin node has a Filter node above it.
//! The filter condition contains a non-NULL constraint on the right table column.
//! The filter condition can be safely pushed down.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::operators::BinaryOperator;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteError, RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::join::join_node::{
    HashInnerJoinNode, HashLeftJoinNode, InnerJoinNode, LeftJoinNode,
};
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for converting LeftJoin to InnerJoin
///
/// When the right table column has a non-NULL filter condition, convert LeftJoin to InnerJoin.
#[derive(Debug)]
pub struct LeftJoinToInnerJoinRule;

impl LeftJoinToInnerJoinRule {
    pub fn new() -> Self {
        Self
    }

    fn check_is_not_null_condition(&self, expr: &Expression, right_col_names: &[String]) -> bool {
        match expr {
            Expression::Function { name, args } if name == "is_not_null" && args.len() == 1 => {
                if let Expression::Variable(var_name) = &args[0] {
                    right_col_names.contains(var_name)
                } else {
                    false
                }
            }
            Expression::Binary { left, op, right } if *op == BinaryOperator::And => {
                self.check_is_not_null_condition(left, right_col_names)
                    || self.check_is_not_null_condition(right, right_col_names)
            }
            Expression::Binary { left, op, right } if *op == BinaryOperator::Or => {
                self.check_is_not_null_condition(left, right_col_names)
                    && self.check_is_not_null_condition(right, right_col_names)
            }
            Expression::Binary { left, op, right } => {
                if *op == BinaryOperator::NotEqual {
                    if let (
                        Expression::Variable(var_name),
                        Expression::Literal(crate::core::Value::Null(_)),
                    ) = (left.as_ref(), right.as_ref())
                    {
                        return right_col_names.contains(var_name);
                    }
                    if let (
                        Expression::Literal(crate::core::Value::Null(_)),
                        Expression::Variable(var_name),
                    ) = (left.as_ref(), right.as_ref())
                    {
                        return right_col_names.contains(var_name);
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn has_not_null_on_right(
        &self,
        expr: &ContextualExpression,
        right_col_names: &[String],
    ) -> bool {
        if let Some(expr_meta) = expr.expression() {
            self.check_is_not_null_condition(expr_meta.inner(), right_col_names)
        } else {
            false
        }
    }

    fn remove_not_null_conditions(
        &self,
        expr: &Expression,
        right_col_names: &[String],
    ) -> Option<Expression> {
        match expr {
            Expression::Function { name, args } if name == "is_not_null" && args.len() == 1 => {
                if let Expression::Variable(var_name) = &args[0] {
                    if right_col_names.contains(var_name) {
                        return None;
                    }
                }
                Some(expr.clone())
            }
            Expression::Binary { left, op, right } if *op == BinaryOperator::And => {
                let left_result = self.remove_not_null_conditions(left, right_col_names);
                let right_result = self.remove_not_null_conditions(right, right_col_names);
                match (left_result, right_result) {
                    (Some(l), Some(r)) => Some(Expression::and(l, r)),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            Expression::Binary { left, op, right } if *op == BinaryOperator::Or => {
                let left_result = self.remove_not_null_conditions(left, right_col_names);
                let right_result = self.remove_not_null_conditions(right, right_col_names);
                match (left_result, right_result) {
                    (Some(l), Some(r)) => Some(Expression::or(l, r)),
                    (Some(l), None) => Some(l),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            Expression::Binary { left, op, right } => {
                if *op == BinaryOperator::NotEqual {
                    if let (
                        Expression::Variable(var_name),
                        Expression::Literal(crate::core::Value::Null(_)),
                    ) = (left.as_ref(), right.as_ref())
                    {
                        if right_col_names.contains(var_name) {
                            return None;
                        }
                    }
                    if let (
                        Expression::Literal(crate::core::Value::Null(_)),
                        Expression::Variable(var_name),
                    ) = (left.as_ref(), right.as_ref())
                    {
                        if right_col_names.contains(var_name) {
                            return None;
                        }
                    }
                }
                Some(expr.clone())
            }
            _ => Some(expr.clone()),
        }
    }

    fn apply_to_hash_left_join(
        &self,
        filter: &FilterNode,
        join: &HashLeftJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let right_col_names = join.right_input().col_names().to_vec();
        let filter_condition = filter.condition();

        if !self.has_not_null_on_right(filter_condition, &right_col_names) {
            return Ok(None);
        }

        let new_join = HashInnerJoinNode::new(
            join.left_input().clone(),
            join.right_input().clone(),
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create HashInnerJoinNode: {:?}", e))
        })?;

        if let Some(expr_meta) = filter_condition.expression() {
            let remaining = self.remove_not_null_conditions(expr_meta.inner(), &right_col_names);

            let mut result = TransformResult::new();

            if let Some(rem_expr) = remaining {
                let ctx = filter_condition.context().clone();
                let meta = crate::core::types::expr::ExpressionMeta::new(rem_expr);
                let id = ctx.register_expression(meta);
                let new_ctx_expr = ContextualExpression::new(id, ctx);

                let new_filter = FilterNode::new(
                    PlanNodeEnum::HashInnerJoin(new_join),
                    new_ctx_expr,
                )
                .map_err(|e| {
                    RewriteError::rewrite_failed(format!("Failed to create FilterNode: {:?}", e))
                })?;

                result.erase_curr = true;
                result.add_new_node(PlanNodeEnum::Filter(new_filter));
            } else {
                result.erase_curr = true;
                result.add_new_node(PlanNodeEnum::HashInnerJoin(new_join));
            }

            return Ok(Some(result));
        }

        Ok(None)
    }

    fn apply_to_left_join(
        &self,
        filter: &FilterNode,
        join: &LeftJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let right_col_names = join.right_input().col_names().to_vec();
        let filter_condition = filter.condition();

        if !self.has_not_null_on_right(filter_condition, &right_col_names) {
            return Ok(None);
        }

        let new_join = InnerJoinNode::new(
            join.left_input().clone(),
            join.right_input().clone(),
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create InnerJoinNode: {:?}", e))
        })?;

        if let Some(expr_meta) = filter_condition.expression() {
            let remaining = self.remove_not_null_conditions(expr_meta.inner(), &right_col_names);

            let mut result = TransformResult::new();

            if let Some(rem_expr) = remaining {
                let ctx = filter_condition.context().clone();
                let meta = crate::core::types::expr::ExpressionMeta::new(rem_expr);
                let id = ctx.register_expression(meta);
                let new_ctx_expr = ContextualExpression::new(id, ctx);

                let new_filter = FilterNode::new(PlanNodeEnum::InnerJoin(new_join), new_ctx_expr)
                    .map_err(|e| {
                    RewriteError::rewrite_failed(format!("Failed to create FilterNode: {:?}", e))
                })?;

                result.erase_curr = true;
                result.add_new_node(PlanNodeEnum::Filter(new_filter));
            } else {
                result.erase_curr = true;
                result.add_new_node(PlanNodeEnum::InnerJoin(new_join));
            }

            return Ok(Some(result));
        }

        Ok(None)
    }
}

impl Default for LeftJoinToInnerJoinRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for LeftJoinToInnerJoinRule {
    fn name(&self) -> &'static str {
        "LeftJoinToInnerJoinRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let filter = match node {
            PlanNodeEnum::Filter(n) => n,
            _ => return Ok(None),
        };

        let input = filter.input();

        match input {
            PlanNodeEnum::HashLeftJoin(join) => self.apply_to_hash_left_join(filter, join),
            PlanNodeEnum::LeftJoin(join) => self.apply_to_left_join(filter, join),
            _ => Ok(None),
        }
    }
}

impl PushDownRule for LeftJoinToInnerJoinRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(node, PlanNodeEnum::Filter(_))
            && matches!(
                target,
                PlanNodeEnum::HashLeftJoin(_) | PlanNodeEnum::LeftJoin(_)
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

    #[test]
    fn test_rule_name() {
        let rule = LeftJoinToInnerJoinRule::new();
        assert_eq!(rule.name(), "LeftJoinToInnerJoinRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = LeftJoinToInnerJoinRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_check_is_not_null_condition() {
        let rule = LeftJoinToInnerJoinRule::new();
        let right_cols = vec!["b_col".to_string()];

        let expr = Expression::function("is_not_null", vec![Expression::variable("b_col")]);
        assert!(rule.check_is_not_null_condition(&expr, &right_cols));

        let expr2 = Expression::function("is_not_null", vec![Expression::variable("a_col")]);
        assert!(!rule.check_is_not_null_condition(&expr2, &right_cols));
    }
}
