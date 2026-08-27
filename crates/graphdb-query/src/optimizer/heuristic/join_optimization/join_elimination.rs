//! Rules for eliminating redundant JOIN operations
//!
//! This rule eliminates JOINs that don't contribute to the query result,
//! such as JOINs with always-true conditions (CrossJoin with no filter)
//! or JOINs where one side is a single-row constant table.
//!
//! # Conversion examples
//!
//! ## Case 1: JOIN with always-true condition
//! Before:
//! ```text
//!   InnerJoin(ON true) → Left → Right
//! ```
//! After:
//! ```text
//!   CrossJoin → Left → Right
//! ```
//!
//! ## Case 2: JOIN with self (redundant)
//! Before:
//! ```text
//!   InnerJoin(ON a.id = a.id) → ScanVertices(a) → ScanVertices(a)
//! ```
//! After:
//! ```text
//!   ScanVertices(a)
//! ```

use crate::core::types::expr::contextual::ContextualExpression;
use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::join::join_node::{
    CrossJoinNode, InnerJoinNode, LeftJoinNode,
};
use crate::planning::plan::PlanNodeEnum;

/// Rules for eliminating redundant JOIN operations
#[derive(Debug)]
pub struct JoinEliminationRule;

impl JoinEliminationRule {
    pub fn new() -> Self {
        Self
    }

    fn is_always_true_condition(
        &self,
        hash_keys: &[ContextualExpression],
        probe_keys: &[ContextualExpression],
    ) -> bool {
        if hash_keys.is_empty() && probe_keys.is_empty() {
            return true;
        }
        false
    }

    fn is_self_join(&self, left: &PlanNodeEnum, right: &PlanNodeEnum) -> bool {
        match (left, right) {
            (PlanNodeEnum::ScanVertices(l), PlanNodeEnum::ScanVertices(r)) => {
                l.space_id() == r.space_id() && l.tag() == r.tag()
            }
            (PlanNodeEnum::ScanEdges(l), PlanNodeEnum::ScanEdges(r)) => {
                l.space_id() == r.space_id() && l.edge_type() == r.edge_type()
            }
            _ => false,
        }
    }

    fn apply_to_inner_join(&self, join: &InnerJoinNode) -> RewriteResult<Option<TransformResult>> {
        let left = join.left_input();
        let right = join.right_input();

        if self.is_self_join(left, right) {
            let mut result = TransformResult::new();
            result.erase_curr = true;
            result.add_new_node(left.clone());
            return Ok(Some(result));
        }

        if self.is_always_true_condition(join.hash_keys(), join.probe_keys()) {
            match CrossJoinNode::new(left.clone(), right.clone()) {
                Ok(cross_join) => {
                    let mut result = TransformResult::new();
                    result.erase_curr = true;
                    result.add_new_node(PlanNodeEnum::CrossJoin(cross_join));
                    return Ok(Some(result));
                }
                Err(_) => return Ok(None),
            }
        }

        Ok(None)
    }

    fn apply_to_left_join(&self, join: &LeftJoinNode) -> RewriteResult<Option<TransformResult>> {
        let _left = join.left_input();
        let _right = join.right_input();

        Ok(None)
    }
}

impl Default for JoinEliminationRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for JoinEliminationRule {
    fn name(&self) -> &'static str {
        "JoinEliminationRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Join")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::InnerJoin(join) => self.apply_to_inner_join(join),
            PlanNodeEnum::LeftJoin(join) => self.apply_to_left_join(join),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = JoinEliminationRule::new();
        assert_eq!(rule.name(), "JoinEliminationRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = JoinEliminationRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}
