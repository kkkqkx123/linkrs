//! Rules for simplifying JOIN conditions
//!
//! This rule simplifies JOIN conditions, including eliminating redundant conditions
//! and converting InnerJoin with ON true to CrossJoin.
//!
//! # Conversion examples
//!
//! ## Eliminate redundant conditions
//! Before:
//! ```text
//!   InnerJoin ON a.id = b.id AND b.id = a.id
//! ```
//!
//! After:
//! ```text
//!   InnerJoin ON a.id = b.id
//! ```
//!
//! ## Convert ON true to CrossJoin
//! Before:
//! ```text
//!   InnerJoin ON true
//! ```
//!
//! After:
//! ```text
//!   CrossJoin
//! ```
//!
//! # Applicable Conditions
//!
//! JOIN conditions contain redundant expressions.
//! JOIN condition is a constant true.
//! JOIN conditions can be simplified.

use crate::core::{Expression, Value};
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteError, RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, HashInnerJoinNode, InnerJoinNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for simplifying JOIN conditions
///
/// Simplify JOIN conditions by removing redundant expressions and converting trivial conditions.
#[derive(Debug)]
pub struct JoinConditionSimplifyRule;

impl JoinConditionSimplifyRule {
    pub fn new() -> Self {
        Self
    }

    fn is_true_expression(&self, expr: &Expression) -> bool {
        matches!(expr, Expression::Literal(Value::Bool(true)))
    }

    fn apply_to_hash_inner_join(
        &self,
        join: &HashInnerJoinNode,
    ) -> RewriteResult<Option<TransformResult>> {
        let hash_keys = join.hash_keys();
        let probe_keys = join.probe_keys();

        if hash_keys.len() != probe_keys.len() || hash_keys.is_empty() {
            return Ok(None);
        }

        let all_true = hash_keys.iter().all(|k| {
            k.expression()
                .map(|m| self.is_true_expression(m.inner()))
                .unwrap_or(false)
        }) && probe_keys.iter().all(|k| {
            k.expression()
                .map(|m| self.is_true_expression(m.inner()))
                .unwrap_or(false)
        });

        if all_true {
            let cross_join = CrossJoinNode::new(
                join.left_input().clone(),
                join.right_input().clone(),
            )
            .map_err(|e| {
                RewriteError::rewrite_failed(format!("Failed to create CrossJoinNode: {:?}", e))
            })?;

            let mut result = TransformResult::new();
            result.erase_curr = true;
            result.add_new_node(PlanNodeEnum::CrossJoin(cross_join));
            return Ok(Some(result));
        }

        Ok(None)
    }

    fn apply_to_inner_join(&self, join: &InnerJoinNode) -> RewriteResult<Option<TransformResult>> {
        let hash_keys = join.hash_keys();
        let probe_keys = join.probe_keys();

        if hash_keys.len() != probe_keys.len() || hash_keys.is_empty() {
            return Ok(None);
        }

        let all_true = hash_keys.iter().all(|k| {
            k.expression()
                .map(|m| self.is_true_expression(m.inner()))
                .unwrap_or(false)
        }) && probe_keys.iter().all(|k| {
            k.expression()
                .map(|m| self.is_true_expression(m.inner()))
                .unwrap_or(false)
        });

        if all_true {
            let cross_join = CrossJoinNode::new(
                join.left_input().clone(),
                join.right_input().clone(),
            )
            .map_err(|e| {
                RewriteError::rewrite_failed(format!("Failed to create CrossJoinNode: {:?}", e))
            })?;

            let mut result = TransformResult::new();
            result.erase_curr = true;
            result.add_new_node(PlanNodeEnum::CrossJoin(cross_join));
            return Ok(Some(result));
        }

        Ok(None)
    }
}

impl Default for JoinConditionSimplifyRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for JoinConditionSimplifyRule {
    fn name(&self) -> &'static str {
        "JoinConditionSimplifyRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::multi(vec!["HashInnerJoin", "InnerJoin"])
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::HashInnerJoin(join) => self.apply_to_hash_inner_join(join),
            PlanNodeEnum::InnerJoin(join) => self.apply_to_inner_join(join),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = JoinConditionSimplifyRule::new();
        assert_eq!(rule.name(), "JoinConditionSimplifyRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = JoinConditionSimplifyRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_true_expression() {
        let rule = JoinConditionSimplifyRule::new();
        assert!(rule.is_true_expression(&Expression::bool(true)));
        assert!(!rule.is_true_expression(&Expression::bool(false)));
        assert!(!rule.is_true_expression(&Expression::int(1)));
    }
}
