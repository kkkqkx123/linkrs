//! Rules that push the filtering conditions up to the AllPaths operation
//!
//! This rule identifies the Filter -> AllPaths mode.
//! And push the filtering conditions up to the AllPaths node.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Push the filtering criteria to the rules that are applied by the AllPaths operation.
///
/// # Conversion example
///
/// Before:
/// ```text
///   Filter(e.likeness > 78)
///           |
///   AllPaths
/// ```
///
/// After:
/// ```text
///   AllPaths(filter: e.likeness > 78)
/// ```
///
/// # Applicable Conditions
///
/// The AllPaths node retrieves the properties of the edges.
/// The minimum number of steps for AllPaths is equal to the maximum number of steps.
/// The filtering criteria can be pushed down to the storage layer.
#[derive(Debug)]
pub struct PushFilterDownAllPathsRule;

impl PushFilterDownAllPathsRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }
}

impl Default for PushFilterDownAllPathsRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushFilterDownAllPathsRule {
    fn name(&self) -> &'static str {
        "PushFilterDownAllPathsRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Filter").with_dependency_name("AllPaths")
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

        // Check whether the input node is of the AllPaths type.
        let all_paths = match input {
            PlanNodeEnum::AllPaths(n) => n,
            _ => return Ok(None),
        };

        // Obtain the filtering criteria
        let filter_condition = filter_node.condition();

        // Create a new AllPaths node.
        let mut new_all_paths = all_paths.clone();

        // Set the filter
        new_all_paths.set_filter(filter_condition.clone());

        // Construct the translation result.
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::AllPaths(new_all_paths));

        Ok(Some(result))
    }
}

impl PushDownRule for PushFilterDownAllPathsRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(
            (node, target),
            (PlanNodeEnum::Filter(_), PlanNodeEnum::AllPaths(_))
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
        let rule = PushFilterDownAllPathsRule::new();
        assert_eq!(rule.name(), "PushFilterDownAllPathsRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushFilterDownAllPathsRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}
