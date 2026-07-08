//! EdgeIndexScan projection push-down optimization rule
//!
//! This rule pushes the projection operation down to the EdgeIndexScan node, thereby reducing the amount of data transmitted.
//!
//! # Translation example
//!
//! Before:
//! ```text
//! Project(col1, col2)
//!         |
//! EdgeIndexScan
//! ```
//!
//! After:
//! ```text
//! EdgeIndexScan(col1, col2)
//! ```

use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::EdgeIndexScanNode;
use crate::query::planning::plan::PlanNodeEnum;

/// EdgeIndexScan Projection Pushdown Rule
///
/// Push the projection operation down to the EdgeIndexScan node.
#[derive(Debug)]
pub struct PushProjectDownEdgeIndexScanRule;

impl PushProjectDownEdgeIndexScanRule {
    pub fn new() -> Self {
        Self
    }

    fn can_push_down_project(
        project_node: &crate::query::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_edge_index_scan_with_projection(
        &self,
        edge_index_scan_node: &EdgeIndexScanNode,
        project_columns: &[YieldColumn],
    ) -> EdgeIndexScanNode {
        let col_names: Vec<String> = project_columns
            .iter()
            .map(|col| col.alias.clone())
            .collect();

        let return_cols: Vec<String> = project_columns
            .iter()
            .map(|col| col.alias.clone())
            .collect();

        let mut new_node = edge_index_scan_node.clone();
        new_node.set_return_columns(return_cols);
        new_node.set_col_names(col_names);
        new_node
    }
}

impl Default for PushProjectDownEdgeIndexScanRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownEdgeIndexScanRule {
    fn name(&self) -> &'static str {
        "PushProjectDownEdgeIndexScanRule"
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
        let edge_index_scan_node = match input {
            PlanNodeEnum::EdgeIndexScan(n) => n,
            _ => return Ok(None),
        };

        let columns = project_node.columns();
        let new_edge_index_scan_node =
            self.create_edge_index_scan_with_projection(edge_index_scan_node, columns);
        let new_node = PlanNodeEnum::EdgeIndexScan(new_edge_index_scan_node);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_node);

        Ok(Some(result))
    }
}

impl PushDownRule for PushProjectDownEdgeIndexScanRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(project) => {
                if project.columns().is_empty() {
                    return false;
                }
                matches!(target, PlanNodeEnum::EdgeIndexScan(_))
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
    use crate::query::planning::plan::core::nodes::{EdgeIndexScanNode, ProjectNode};
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
        let rule = PushProjectDownEdgeIndexScanRule::new();
        assert_eq!(rule.name(), "PushProjectDownEdgeIndexScanRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownEdgeIndexScanRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_apply_with_edge_index_scan() {
        let rule = PushProjectDownEdgeIndexScanRule::new();
        let mut ctx = RewriteContext::new();

        let edge_index_scan = EdgeIndexScanNode::new(1, "edge_type", "index_name");
        let edge_index_scan_enum = PlanNodeEnum::EdgeIndexScan(edge_index_scan);

        let columns = vec![
            create_yield_column(Expression::Variable("src".to_string()), "src"),
            create_yield_column(Expression::Variable("dst".to_string()), "dst"),
        ];
        let project = ProjectNode::new(edge_index_scan_enum.clone(), columns)
            .expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        let result = rule
            .apply(&mut ctx, &project_enum)
            .expect("Failed to apply rule");

        assert!(result.is_some());
        let transform = result.expect("Failed to apply rewrite rule");
        assert!(transform.erase_curr);

        match &transform.new_nodes[0] {
            PlanNodeEnum::EdgeIndexScan(node) => {
                assert_eq!(node.col_names(), &["src", "dst"]);
                assert_eq!(node.return_columns(), &["src", "dst"]);
            }
            _ => panic!("There is an expectation for the EdgeIndexScan node to be available."),
        }
    }

    #[test]
    fn test_push_down_rule_trait() {
        let rule = PushProjectDownEdgeIndexScanRule::new();

        let edge_index_scan =
            PlanNodeEnum::EdgeIndexScan(EdgeIndexScanNode::new(1, "edge_type", "index_name"));
        let columns = vec![create_yield_column(
            Expression::Variable("test".to_string()),
            "test",
        )];
        let project = ProjectNode::new(edge_index_scan.clone(), columns)
            .expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        assert!(rule.can_push_down(&project_enum, &edge_index_scan));
    }
}
