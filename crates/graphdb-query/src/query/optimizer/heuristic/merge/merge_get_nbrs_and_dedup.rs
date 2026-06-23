//! Combine the rules for retrieving neighboring elements and performing deduplication operations.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Combine the rules for obtaining neighboring elements and performing deduplication operations.
///
/// # Conversion example
///
/// Before:
/// ```text
///   GetNeighbors
///       |
///   Dedup
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   GetNeighbors(dedup=true)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is the GetNeighbors node.
/// The child node is a Dedup node.
/// The deduplication operation can be merged into the GetNeighbors function.
#[derive(Debug)]
pub struct MergeGetNbrsAndDedupRule;

impl MergeGetNbrsAndDedupRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }
}

impl Default for MergeGetNbrsAndDedupRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for MergeGetNbrsAndDedupRule {
    fn name(&self) -> &'static str {
        "MergeGetNbrsAndDedupRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("GetNeighbors").with_dependency_name("Dedup")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a GetNeighbors node.
        let get_neighbors = match node {
            PlanNodeEnum::GetNeighbors(n) => n,
            _ => return Ok(None),
        };

        // GetNeighbors uses the MultipleInputNode and needs to retrieve the dependencies.
        let deps = get_neighbors.dependencies();
        if deps.is_empty() {
            return Ok(None);
        }

        // Check whether the first dependency is a Dedup node.
        let dedup_node = match deps.first() {
            Some(PlanNodeEnum::Dedup(n)) => n,
            _ => return Ok(None),
        };

        // Use the input from Dedup as the new input.
        let dedup_input = dedup_node.input().clone();

        // Create a new GetNeighbors node.
        let mut new_get_neighbors = get_neighbors.clone();

        // Set the deduplication flag
        if !new_get_neighbors.dedup() {
            new_get_neighbors.set_dedup(true);
        }

        // Remove the existing dependencies and set up the new input.
        new_get_neighbors.deps_mut().clear();
        new_get_neighbors.deps_mut().push(dedup_input);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::GetNeighbors(new_get_neighbors));

        Ok(Some(result))
    }
}

impl MergeRule for MergeGetNbrsAndDedupRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_get_neighbors() && child.is_dedup()
    }

    fn create_merged_node(
        &self,
        ctx: &mut RewriteContext,
        parent: &PlanNodeEnum,
        _child: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        self.apply(ctx, parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetNeighborsNode;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;

    #[test]
    fn test_rule_name() {
        let rule = MergeGetNbrsAndDedupRule::new();
        assert_eq!(rule.name(), "MergeGetNbrsAndDedupRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = MergeGetNbrsAndDedupRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_merge_get_nbrs_and_dedup() {
        // Create the starting node.
        let start = PlanNodeEnum::Start(StartNode::new());

        // Creating a Deduplication Node
        let dedup = DedupNode::new(start).expect("Failed to create DedupNode");
        let dedup_node = PlanNodeEnum::Dedup(dedup);

        // Create the GetNeighbors node.
        let get_neighbors = GetNeighborsNode::new(1, "v");
        let mut get_neighbors_node = PlanNodeEnum::GetNeighbors(get_neighbors);

        // Manually setting dependencies
        if let PlanNodeEnum::GetNeighbors(ref mut gn) = get_neighbors_node {
            gn.deps_mut().clear();
            gn.deps_mut().push(dedup_node);
        }

        // Apply the rules
        let rule = MergeGetNbrsAndDedupRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &get_neighbors_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The merging of the GetNeighbors and Dedup nodes should succeed."
        );

        // Verification results
        let transform_result = result.expect("Failed to apply rewrite rule");
        assert!(transform_result.erase_curr);
        assert_eq!(transform_result.new_nodes.len(), 1);

        // Verify that the new GetNeighbors node has the dedup flag set.
        if let PlanNodeEnum::GetNeighbors(ref new_gn) = transform_result.new_nodes[0] {
            assert!(
                new_gn.dedup(),
                "The new GetNeighbors node should have the dedup flag set."
            );
        } else {
            panic!("The “GetNeighbors” node");
        }
    }
}
