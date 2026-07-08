//! Optimization rules for set operations on the empty set
//!
//! Optimizing the handling of the empty set in set operations:
//! - Minus: If the value for "minus" is empty, the main input should be returned directly.
//! - **Intersect:** If either of the inputs is empty, return the empty set.
//!
//! # Conversion example
//!
//! Before (Minus):
//! ```text
//!   Minus
//!    /   \
//! A. Start (Empty set)
//! ```
//!
//! After:
//! ```text
//!   A
//! ```
//!
//! Before (Intersect):
//! ```text
//!   Intersect
//!    /       \
//! A       Start (Empty set)
//! ```
//!
//! After:
//! ```text
//! Start (the empty set)
//! ```

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Optimization rules for set operations involving the empty set
///
/// Optimize the handling of the empty set in set operations to avoid unnecessary calculations.
#[derive(Debug)]
pub struct EliminateEmptySetOperationRule;

impl EliminateEmptySetOperationRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Determine whether a node is an empty set node.
    fn is_empty_node(&self, node: &PlanNodeEnum) -> bool {
        match node {
            // The “Start” node represents the empty set.
            PlanNodeEnum::Start(_) => true,
            // Scan operation with a limit of 0
            PlanNodeEnum::ScanVertices(n) => n.limit() == Some(0),
            PlanNodeEnum::ScanEdges(n) => n.limit() == Some(0),
            PlanNodeEnum::GetVertices(n) => n.limit() == Some(0),
            PlanNodeEnum::GetEdges(n) => n.limit() == Some(0),
            PlanNodeEnum::Limit(n) => n.count() == 0,
            // Recursive checking of a single input node
            PlanNodeEnum::Filter(n) => self.is_empty_node(n.input()),
            PlanNodeEnum::Project(n) => self.is_empty_node(n.input()),
            PlanNodeEnum::Dedup(n) => self.is_empty_node(n.input()),
            _ => false,
        }
    }

    /// Create an empty set node.
    fn create_empty_node(&self) -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }
}

impl Default for EliminateEmptySetOperationRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for EliminateEmptySetOperationRule {
    fn name(&self) -> &'static str {
        "EliminateEmptySetOperationRule"
    }

    fn pattern(&self) -> Pattern {
        // Match the Minus or Intersect nodes
        Pattern::multi(vec!["Minus", "Intersect"])
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            // Processing the Minus node
            PlanNodeEnum::Minus(minus_node) => {
                let minus_input = minus_node.minus_input();

                // If the input for the subtraction is empty, simply return the main input.
                if self.is_empty_node(minus_input) {
                    let mut result = TransformResult::new();
                    result.erase_curr = true;
                    result.add_new_node(minus_node.input().clone());
                    return Ok(Some(result));
                }

                Ok(None)
            }
            // Processing the Intersect node
            PlanNodeEnum::Intersect(intersect_node) => {
                let input = intersect_node.input();
                let intersect_input = intersect_node.intersect_input();

                // If either of the inputs is empty, return an empty set.
                if self.is_empty_node(input) || self.is_empty_node(intersect_input) {
                    let mut result = TransformResult::new();
                    result.erase_curr = true;
                    result.add_new_node(self.create_empty_node());
                    return Ok(Some(result));
                }

                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

impl EliminationRule for EliminateEmptySetOperationRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Minus(minus_node) => self.is_empty_node(minus_node.minus_input()),
            PlanNodeEnum::Intersect(intersect_node) => {
                self.is_empty_node(intersect_node.input())
                    || self.is_empty_node(intersect_node.intersect_input())
            }
            _ => false,
        }
    }

    fn eliminate(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(_ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_eliminate_empty_set_operation_rule_name() {
        let rule = EliminateEmptySetOperationRule::new();
        assert_eq!(rule.name(), "EliminateEmptySetOperationRule");
    }

    #[test]
    fn test_eliminate_empty_set_operation_rule_pattern() {
        let rule = EliminateEmptySetOperationRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_is_empty_node() {
        let rule = EliminateEmptySetOperationRule::new();

        // The Start node is the empty set.
        let start_node = StartNode::new();
        assert!(rule.is_empty_node(&PlanNodeEnum::Start(start_node)));
    }

    #[test]
    fn test_create_empty_node() {
        let rule = EliminateEmptySetOperationRule::new();
        let empty_node = rule.create_empty_node();
        assert!(matches!(empty_node, PlanNodeEnum::Start(_)));
    }
}
