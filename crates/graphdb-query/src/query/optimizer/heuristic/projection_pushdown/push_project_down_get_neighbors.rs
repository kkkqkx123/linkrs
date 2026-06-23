//! The “GetNeighbors” projection implements optimization rules for the downcasting process.
//!
//! This rule pushes the projection operation down to the GetNeighbors node, thereby reducing the amount of data transmitted.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Project(col1, col2)
//!         |
//!   GetNeighbors
//! ```
//!
//! After:
//! ```text
//! GetNeighbors(col1, col2)
//! ```

use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::GetNeighborsNode;
use crate::query::planning::plan::PlanNodeEnum;

/// GetNeighbors projection pushdown rule
///
/// Push the projection operation down to the GetNeighbors node.
#[derive(Debug)]
pub struct PushProjectDownGetNeighborsRule;

impl PushProjectDownGetNeighborsRule {
    pub fn new() -> Self {
        Self
    }

    fn can_push_down_project(
        project_node: &crate::query::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_get_neighbors_with_projection(
        &self,
        get_neighbors_node: &GetNeighborsNode,
        project_columns: &[YieldColumn],
    ) -> GetNeighborsNode {
        let col_names: Vec<String> = project_columns
            .iter()
            .map(|col| col.alias.clone())
            .collect();

        let mut new_node = get_neighbors_node.clone();
        new_node.set_col_names(col_names);
        new_node
    }
}

impl Default for PushProjectDownGetNeighborsRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownGetNeighborsRule {
    fn name(&self) -> &'static str {
        "PushProjectDownGetNeighborsRule"
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
        let get_neighbors_node = match input {
            PlanNodeEnum::GetNeighbors(n) => n,
            _ => return Ok(None),
        };

        let columns = project_node.columns();
        let new_get_neighbors_node =
            self.create_get_neighbors_with_projection(get_neighbors_node, columns);
        let new_node = PlanNodeEnum::GetNeighbors(new_get_neighbors_node);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_node);

        Ok(Some(result))
    }
}

impl PushDownRule for PushProjectDownGetNeighborsRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(project) => {
                if project.columns().is_empty() {
                    return false;
                }
                matches!(target, PlanNodeEnum::GetNeighbors(_))
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
    use crate::query::planning::plan::core::nodes::{GetNeighborsNode, ProjectNode};
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
        let rule = PushProjectDownGetNeighborsRule::new();
        assert_eq!(rule.name(), "PushProjectDownGetNeighborsRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownGetNeighborsRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_apply_with_get_neighbors() {
        let rule = PushProjectDownGetNeighborsRule::new();
        let mut ctx = RewriteContext::new();

        let get_neighbors = GetNeighborsNode::new(1, "vids");
        let get_neighbors_enum = PlanNodeEnum::GetNeighbors(get_neighbors);

        let columns = vec![create_yield_column(
            Expression::Variable("neighbor".to_string()),
            "neighbor",
        )];
        let project = ProjectNode::new(get_neighbors_enum.clone(), columns)
            .expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        let result = rule
            .apply(&mut ctx, &project_enum)
            .expect("Failed to apply rule");

        assert!(result.is_some());
        let transform = result.expect("Failed to apply rewrite rule");
        assert!(transform.erase_curr);

        match &transform.new_nodes[0] {
            PlanNodeEnum::GetNeighbors(node) => {
                assert_eq!(node.col_names(), &["neighbor"]);
            }
            _ => panic!("Expectations for the GetNeighbors node"),
        }
    }

    #[test]
    fn test_push_down_rule_trait() {
        let rule = PushProjectDownGetNeighborsRule::new();

        let get_neighbors = PlanNodeEnum::GetNeighbors(GetNeighborsNode::new(1, "vids"));
        let columns = vec![create_yield_column(
            Expression::Variable("test".to_string()),
            "test",
        )];
        let project =
            ProjectNode::new(get_neighbors.clone(), columns).expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        assert!(rule.can_push_down(&project_enum, &get_neighbors));
    }
}
