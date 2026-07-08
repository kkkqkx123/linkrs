//! Combine the rules for obtaining vertices and performing deduplication operations.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Rules for merging the operations of obtaining vertices and removing duplicates
///
/// # Conversion example
///
/// Before:
/// ```text
///   GetVertices
///       |
///   Dedup
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   GetVertices(dedup=true)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is the GetVertices node.
/// The child node is a Dedup node.
/// The deduplication operation can be combined with the GetVertices function.
#[derive(Debug)]
pub struct MergeGetVerticesAndDedupRule;

impl MergeGetVerticesAndDedupRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for MergeGetVerticesAndDedupRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for MergeGetVerticesAndDedupRule {
    fn name(&self) -> &'static str {
        "MergeGetVerticesAndDedupRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("GetVertices").with_dependency_name("Dedup")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a GetVertices node.
        let get_vertices = match node {
            PlanNodeEnum::GetVertices(n) => n,
            _ => return Ok(None),
        };

        // GetVertices uses the MultipleInputNode and needs to retrieve the dependencies.
        let deps = get_vertices.dependencies();
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

        // Create a new GetVertices node.
        let mut new_get_vertices = get_vertices.clone();

        // Set the deduplication flag
        if !new_get_vertices.dedup() {
            new_get_vertices.set_dedup(true);
        }

        // Remove the existing dependencies and set the new input parameters.
        new_get_vertices.deps_mut().clear();
        new_get_vertices.deps_mut().push(dedup_input);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::GetVertices(new_get_vertices));

        Ok(Some(result))
    }
}

impl MergeRule for MergeGetVerticesAndDedupRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_get_vertices() && child.is_dedup()
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
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;

    #[test]
    fn test_rule_name() {
        let rule = MergeGetVerticesAndDedupRule::new();
        assert_eq!(rule.name(), "MergeGetVerticesAndDedupRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = MergeGetVerticesAndDedupRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_merge_get_vertices_and_dedup() {
        // Create the starting node.
        let start = PlanNodeEnum::Start(StartNode::new());

        // Creating a Deduplication Node
        let dedup = DedupNode::new(start).expect("Failed to create DedupNode");
        let dedup_node = PlanNodeEnum::Dedup(dedup);

        // Create the GetVertices node.
        let get_vertices = GetVerticesNode::new(1, "default", "v");
        let mut get_vertices_node = PlanNodeEnum::GetVertices(get_vertices);

        // Manually setting dependencies
        if let PlanNodeEnum::GetVertices(ref mut gv) = get_vertices_node {
            gv.deps_mut().clear();
            gv.deps_mut().push(dedup_node);
        }

        // Application rules
        let rule = MergeGetVerticesAndDedupRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &get_vertices_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The merger of the GetVertices and Dedup nodes should succeed."
        );

        // Validation results
        let transform_result = result.expect("Failed to apply rewrite rule");
        assert!(transform_result.erase_curr);
        assert_eq!(transform_result.new_nodes.len(), 1);

        // Verify that the new GetVertices node has the dedup flag set.
        if let PlanNodeEnum::GetVertices(ref new_gv) = transform_result.new_nodes[0] {
            assert!(
                new_gv.dedup(),
                "The new GetVertices node should have the dedup flag set."
            );
        } else {
            panic!("The “GetVertices” node");
        }
    }
}
