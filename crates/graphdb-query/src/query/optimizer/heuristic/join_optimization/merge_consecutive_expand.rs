//! Rules for merging consecutive ExpandAll operations into Traverse
//!
//! This rule merges multiple consecutive ExpandAll operations into a single Traverse operation,
//! which is more efficient for multi-step graph traversal.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   ExpandAll(e1) → ExpandAll(e2)
//! ```
//!
//! After:
//! ```text
//!   Traverse(steps=2)
//! ```
//!
//! # Applicable Conditions
//!
//! Two consecutive ExpandAll operations
//! The output of the first ExpandAll feeds into the second
//! Compatible edge types and directions

use crate::core::types::EdgeDirection;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::traversal::traversal_node::{
    ExpandAllNode, TraverseNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Rules for merging consecutive ExpandAll operations into Traverse
#[derive(Debug)]
pub struct MergeConsecutiveExpandRule;

impl MergeConsecutiveExpandRule {
    pub fn new() -> Self {
        Self
    }

    fn parse_direction(dir: &str) -> EdgeDirection {
        match dir.to_uppercase().as_str() {
            "OUT" => EdgeDirection::Out,
            "IN" => EdgeDirection::In,
            _ => EdgeDirection::Both,
        }
    }

    fn can_merge(&self, first: &ExpandAllNode, second: &ExpandAllNode) -> bool {
        if first.step_limit().is_some() || second.step_limit().is_some() {
            return false;
        }

        true
    }

    fn merge_edge_types(&self, first: &[String], second: &[String]) -> Vec<String> {
        let mut merged = first.to_vec();
        for et in second {
            if !merged.contains(et) {
                merged.push(et.clone());
            }
        }
        merged
    }

    fn apply_to_expand_all(&self, outer: &ExpandAllNode) -> RewriteResult<Option<TransformResult>> {
        let inner = match outer.inputs().first() {
            Some(PlanNodeEnum::ExpandAll(inner)) => inner,
            _ => return Ok(None),
        };

        if !self.can_merge(inner, outer) {
            return Ok(None);
        }

        let edge_types = self.merge_edge_types(inner.edge_types(), outer.edge_types());
        let direction = Self::parse_direction(outer.direction());

        let mut traverse = TraverseNode::new(0, "", 1, 2);
        traverse.set_edge_types(edge_types);
        traverse.set_direction(direction);

        if let Some(input) = inner.inputs().first() {
            traverse.set_input(input.clone());
        }

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Traverse(traverse));

        Ok(Some(result))
    }
}

impl Default for MergeConsecutiveExpandRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for MergeConsecutiveExpandRule {
    fn name(&self) -> &'static str {
        "MergeConsecutiveExpandRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("ExpandAll")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        match node {
            PlanNodeEnum::ExpandAll(expand) => self.apply_to_expand_all(expand),
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_name() {
        let rule = MergeConsecutiveExpandRule::new();
        assert_eq!(rule.name(), "MergeConsecutiveExpandRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = MergeConsecutiveExpandRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_parse_direction() {
        assert!(matches!(
            MergeConsecutiveExpandRule::parse_direction("OUT"),
            EdgeDirection::Out
        ));
        assert!(matches!(
            MergeConsecutiveExpandRule::parse_direction("IN"),
            EdgeDirection::In
        ));
        assert!(matches!(
            MergeConsecutiveExpandRule::parse_direction("BOTH"),
            EdgeDirection::Both
        ));
    }
}
