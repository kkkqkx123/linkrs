//! Merge the rules for obtaining neighbors and performing projection operations.

use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Merge the rules for obtaining neighbors and performing projection operations.
///
/// # Conversion example
///
/// Before:
/// ```text
///   GetNeighbors
///       |
///   Project(col1)
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   GetNeighbors(src=col1.expr)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is the GetNeighbors node.
/// The child node is a Project node.
/// The project only projects one column, and this column serves as the source for the GetNeighbors function.
#[derive(Debug)]
pub struct MergeGetNbrsAndProjectRule;

impl MergeGetNbrsAndProjectRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }
}

impl Default for MergeGetNbrsAndProjectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for MergeGetNbrsAndProjectRule {
    fn name(&self) -> &'static str {
        "MergeGetNbrsAndProjectRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("GetNeighbors").with_dependency_name("Project")
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

        // Check whether the first dependency is a Project node.
        let project_node = match deps.first() {
            Some(PlanNodeEnum::Project(n)) => n,
            _ => return Ok(None),
        };

        // Check whether the Project only projects one column.
        let columns = project_node.columns();
        if columns.len() != 1 {
            return Ok(None);
        }

        // Obtain the input for the Project as the new input.
        let project_input = project_node.input().clone();

        // Create a new GetNeighbors node.
        let mut new_get_neighbors = get_neighbors.clone();

        // Update the source reference to the expression in the Project column.
        let src_expr = columns[0].expression.clone();
        if let Some(expr_meta) = src_expr.expression() {
            if let Expression::Variable(name) = expr_meta.inner() {
                new_get_neighbors.set_src_vids(name.clone());
            }
        }

        // Remove the existing dependencies and set up the new input.
        new_get_neighbors.deps_mut().clear();
        new_get_neighbors.deps_mut().push(project_input);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::GetNeighbors(new_get_neighbors));

        Ok(Some(result))
    }
}

impl MergeRule for MergeGetNbrsAndProjectRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_get_neighbors() && child.is_project()
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
    use crate::core::types::ContextualExpression;
    use crate::core::{Expression, YieldColumn};
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetNeighborsNode;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;

    #[test]
    fn test_rule_name() {
        let rule = MergeGetNbrsAndProjectRule::new();
        assert_eq!(rule.name(), "MergeGetNbrsAndProjectRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = MergeGetNbrsAndProjectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_merge_get_nbrs_and_project() {
        use crate::core::types::expr::ExpressionMeta;
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        // Create the starting node.
        let start = PlanNodeEnum::Start(StartNode::new());

        // Create the context for the expression.
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // Create a Project node and project a column.
        let vid_expr = Expression::Variable("vid".to_string());
        let vid_meta = ExpressionMeta::new(vid_expr);
        let vid_id = expr_ctx.register_expression(vid_meta);
        let vid_ctx_expr = ContextualExpression::new(vid_id, expr_ctx);

        let columns = vec![YieldColumn {
            expression: vid_ctx_expr,
            alias: "v".to_string(),
            is_matched: false,
        }];
        let project = ProjectNode::new(start, columns).expect("Failed to create ProjectNode");
        let project_node = PlanNodeEnum::Project(project);

        // Create the GetNeighbors node.
        let get_neighbors = GetNeighborsNode::new(1, "v");
        let mut get_neighbors_node = PlanNodeEnum::GetNeighbors(get_neighbors);

        // Manually setting dependencies
        if let PlanNodeEnum::GetNeighbors(ref mut gn) = get_neighbors_node {
            gn.deps_mut().clear();
            gn.deps_mut().push(project_node);
        }

        // Application rules
        let rule = MergeGetNbrsAndProjectRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &get_neighbors_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The merging of the GetNeighbors and Project nodes should succeed."
        );
    }
}
