//! GetEdges: Optimization rules for projection and push-down operations
//!
//! This rule pushes the projection operation down to the GetEdges node, thereby reducing the amount of data transmitted.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Project(col1, col2)
//!         |
//!   GetEdges
//! ```
//!
//! After:
//! ```text
//! GetEdges(col1, col2)
//! ```

use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::GetEdgesNode;
use crate::query::planning::plan::PlanNodeEnum;

/// GetEdges: Projection downscaling rules
///
/// Push the projection operation down to the GetEdges node.
#[derive(Debug)]
pub struct PushProjectDownGetEdgesRule;

impl PushProjectDownGetEdgesRule {
    pub fn new() -> Self {
        Self
    }

    fn can_push_down_project(
        project_node: &crate::query::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_get_edges_with_projection(
        &self,
        get_edges_node: &GetEdgesNode,
        project_columns: &[YieldColumn],
    ) -> GetEdgesNode {
        let col_names: Vec<String> = project_columns
            .iter()
            .map(|col| col.alias.clone())
            .collect();

        let mut new_node = get_edges_node.clone();
        new_node.set_col_names(col_names);
        new_node
    }
}

impl Default for PushProjectDownGetEdgesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownGetEdgesRule {
    fn name(&self) -> &'static str {
        "PushProjectDownGetEdgesRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project")
    }

    fn apply(
        &self,
        _ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let project_node = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        if !Self::can_push_down_project(project_node) {
            return Ok(None);
        }

        let input = project_node.input();
        let get_edges_node = match input {
            PlanNodeEnum::GetEdges(n) => n,
            _ => return Ok(None),
        };

        let columns = project_node.columns();
        let new_get_edges_node = self.create_get_edges_with_projection(get_edges_node, columns);
        let new_node = PlanNodeEnum::GetEdges(new_get_edges_node);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_node);

        Ok(Some(result))
    }
}

impl PushDownRule for PushProjectDownGetEdgesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(project) => {
                if project.columns().is_empty() {
                    return false;
                }
                matches!(target, PlanNodeEnum::GetEdges(_))
            }
            _ => false,
        }
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
    use crate::core::types::ContextualExpression;
    use crate::core::{Expression, YieldColumn};
    use crate::query::planning::plan::core::nodes::{GetEdgesNode, ProjectNode};
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use std::sync::Arc;

    fn create_yield_column(expr: Expression, alias: &str) -> YieldColumn {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        YieldColumn {
            expression: ctx_expr,
            alias: alias.to_string(),
            is_matched: false,
        }
    }

    #[test]
    fn test_rule_name() {
        let rule = PushProjectDownGetEdgesRule::new();
        assert_eq!(rule.name(), "PushProjectDownGetEdgesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownGetEdgesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_apply_with_get_edges() {
        let rule = PushProjectDownGetEdgesRule::new();
        let mut ctx = RewriteContext::new();

        let get_edges = GetEdgesNode::new(1, "src", "edge_type", "rank", "dst");
        let get_edges_enum = PlanNodeEnum::GetEdges(get_edges);

        let columns = vec![create_yield_column(
            Expression::Variable("edge".to_string()),
            "edge",
        )];
        let project = ProjectNode::new(get_edges_enum.clone(), columns)
            .expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        let result = rule
            .apply(&mut ctx, &project_enum)
            .expect("Failed to apply rule");

        assert!(result.is_some());
        let transform = result.expect("Failed to apply rewrite rule");
        assert!(transform.erase_curr);

        match &transform.new_nodes[0] {
            PlanNodeEnum::GetEdges(node) => {
                assert_eq!(node.col_names(), &["edge"]);
            }
            _ => panic!("Expectation for the GetEdges node"),
        }
    }

    #[test]
    fn test_push_down_rule_trait() {
        let rule = PushProjectDownGetEdgesRule::new();

        let get_edges =
            PlanNodeEnum::GetEdges(GetEdgesNode::new(1, "src", "edge_type", "rank", "dst"));
        let columns = vec![create_yield_column(
            Expression::Variable("test".to_string()),
            "test",
        )];
        let project =
            ProjectNode::new(get_edges.clone(), columns).expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        assert!(rule.can_push_down(&project_enum, &get_edges));
    }
}
