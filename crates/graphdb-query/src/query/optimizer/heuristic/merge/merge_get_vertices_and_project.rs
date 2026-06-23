//! Combine the rules for obtaining vertices and performing projection operations.

use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Merge the rules for obtaining vertices and performing projection operations.
///
/// # Conversion example
///
/// Before:
/// ```text
///   GetVertices
///       |
///   Project(col1)
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   GetVertices(src=col1.expr)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is the GetVertices node.
/// The child node is a Project node.
/// The project only projects one column, and this column serves as the source for the GetVertices function.
#[derive(Debug)]
pub struct MergeGetVerticesAndProjectRule;

impl MergeGetVerticesAndProjectRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }
}

impl Default for MergeGetVerticesAndProjectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for MergeGetVerticesAndProjectRule {
    fn name(&self) -> &'static str {
        "MergeGetVerticesAndProjectRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("GetVertices").with_dependency_name("Project")
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

        // Create a new GetVertices node.
        let mut new_get_vertices = get_vertices.clone();

        // Update the source reference to the expression in the Project column.
        let src_expr = columns[0].expression.clone();
        new_get_vertices.set_src_ref(src_expr);

        // Remove the existing dependencies and set up the new input.
        new_get_vertices.deps_mut().clear();
        new_get_vertices.deps_mut().push(project_input);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::GetVertices(new_get_vertices));

        Ok(Some(result))
    }
}

impl MergeRule for MergeGetVerticesAndProjectRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_get_vertices() && child.is_project()
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
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;

    #[test]
    fn test_rule_name() {
        let rule = MergeGetVerticesAndProjectRule::new();
        assert_eq!(rule.name(), "MergeGetVerticesAndProjectRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = MergeGetVerticesAndProjectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_merge_get_vertices_and_project() {
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

        // Create the GetVertices node.
        let get_vertices = GetVerticesNode::new(1, "default", "v");
        let mut get_vertices_node = PlanNodeEnum::GetVertices(get_vertices);

        // Manually setting dependencies
        if let PlanNodeEnum::GetVertices(ref mut gv) = get_vertices_node {
            gv.deps_mut().clear();
            gv.deps_mut().push(project_node);
        }

        // Application rules
        let rule = MergeGetVerticesAndProjectRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &get_vertices_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The merging of the GetVertices and Project nodes should succeed."
        );
    }
}
