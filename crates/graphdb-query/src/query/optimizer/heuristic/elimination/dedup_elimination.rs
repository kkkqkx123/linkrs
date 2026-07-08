//! Rules for eliminating duplicate operations
//!
//! The Dedup node can be removed when its child nodes themselves ensure the uniqueness of the results.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Dedup
//!       |
//! IndexScan (ensures uniqueness during index scanning)
//! ```
//!
//! After:
//! ```text
//!   IndexScan
//! ```
//!
//! # Applicable Conditions
//!
//! The child nodes of the Dedup node are IndexScan, GetVertices, or GetEdges.
//! These operations themselves ensure the uniqueness of the results.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{EliminationRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for eliminating duplicate operations
///
/// When the child nodes themselves ensure the uniqueness of the result, the Dedup node can be removed.
#[derive(Debug)]
pub struct DedupEliminationRule;

impl DedupEliminationRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Check whether the subnodes ensure uniqueness.
    fn child_guarantees_uniqueness(&self, child: &PlanNodeEnum) -> bool {
        // The index scan ensures uniqueness.
        if child.is_index_scan() {
            return true;
        }

        // Determination based on the type of node
        match child {
            // Primary key queries ensure uniqueness.
            PlanNodeEnum::GetVertices(_) => true,
            PlanNodeEnum::GetEdges(_) => true,
            // Nodes related to index scanning
            PlanNodeEnum::ScanVertices(node) => {
                // If the scanning has uniqueness constraints (such as scanning for a primary key)
                node.limit() == Some(1)
            }
            PlanNodeEnum::ScanEdges(node) => node.limit() == Some(1),
            // Other nodes that ensure uniqueness:
            PlanNodeEnum::Start(_) => true,
            _ => false,
        }
    }
}

impl Default for DedupEliminationRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for DedupEliminationRule {
    fn name(&self) -> &'static str {
        "DedupEliminationRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Dedup")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Dedup node.
        let dedup_node = match node {
            PlanNodeEnum::Dedup(n) => n,
            _ => return Ok(None),
        };

        // Obtain the input node
        let input = dedup_node.input();

        // Check whether the subnodes ensure uniqueness.
        if !self.child_guarantees_uniqueness(input) {
            return Ok(None);
        }

        // Create a conversion result that replaces the current Dedup node with the input node.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(input.clone());

        Ok(Some(result))
    }
}

impl EliminationRule for DedupEliminationRule {
    fn can_eliminate(&self, node: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Dedup(n) => {
                let input = n.input();
                self.child_guarantees_uniqueness(input)
            }
            _ => false,
        }
    }

    fn eliminate(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::optimizer::heuristic::rule::RewriteRule;

    #[test]
    fn test_dedup_elimination_rule_name() {
        let rule = DedupEliminationRule::new();
        assert_eq!(rule.name(), "DedupEliminationRule");
    }

    #[test]
    fn test_dedup_elimination_rule_pattern() {
        let rule = DedupEliminationRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_child_guarantees_uniqueness() {
        let rule = DedupEliminationRule::new();

        // The uniqueness of the Start node is guaranteed.
        let start_node =
            crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode::new();
        assert!(rule.child_guarantees_uniqueness(&PlanNodeEnum::Start(start_node)));
    }
}
