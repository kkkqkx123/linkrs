//! ScanEdges: Projection Downstream Optimization Rules
//!
//! This rule pushes the projection operation down to the ScanEdges node, thereby reducing the amount of data transmitted.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Project(col1, col2)
//!         |
//!     ScanEdges
//! ```
//!
//! After:
//! ```text
//! ScanEdges(col1, col2)
//! ```

use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::ScanEdgesNode;
use crate::query::planning::plan::PlanNodeEnum;

/// ScanEdges Projection Pushdown Rule
///
/// Push the projection operation down to the ScanEdges node.
#[derive(Debug)]
pub struct PushProjectDownScanEdgesRule;

impl PushProjectDownScanEdgesRule {
    pub fn new() -> Self {
        Self
    }

    fn can_push_down_project(
        project_node: &crate::query::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_scan_edges_with_projection(
        &self,
        scan_node: &ScanEdgesNode,
        project_columns: &[YieldColumn],
    ) -> ScanEdgesNode {
        let col_names: Vec<String> = project_columns
            .iter()
            .map(|col| col.alias.clone())
            .collect();

        let mut new_node = scan_node.clone();
        new_node.set_col_names(col_names);
        new_node
    }
}

impl Default for PushProjectDownScanEdgesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownScanEdgesRule {
    fn name(&self) -> &'static str {
        "PushProjectDownScanEdgesRule"
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
        let scan_node = match input {
            PlanNodeEnum::ScanEdges(n) => n,
            _ => return Ok(None),
        };

        let columns = project_node.columns();
        let new_scan_node = self.create_scan_edges_with_projection(scan_node, columns);
        let new_node = PlanNodeEnum::ScanEdges(new_scan_node);

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_node);

        Ok(Some(result))
    }
}

impl PushDownRule for PushProjectDownScanEdgesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(project) => {
                if project.columns().is_empty() {
                    return false;
                }
                matches!(target, PlanNodeEnum::ScanEdges(_))
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
    use crate::query::planning::plan::core::nodes::{ProjectNode, ScanEdgesNode};
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
        let rule = PushProjectDownScanEdgesRule::new();
        assert_eq!(rule.name(), "PushProjectDownScanEdgesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownScanEdgesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_apply_with_scan_edges() {
        let rule = PushProjectDownScanEdgesRule::new();
        let mut ctx = RewriteContext::new();

        let scan_node = ScanEdgesNode::new(1, "edge_type");
        let scan = PlanNodeEnum::ScanEdges(scan_node);

        let columns = vec![
            create_yield_column(Expression::Variable("src".to_string()), "src"),
            create_yield_column(Expression::Variable("dst".to_string()), "dst"),
        ];
        let project =
            ProjectNode::new(scan.clone(), columns).expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        let result = rule
            .apply(&mut ctx, &project_enum)
            .expect("Failed to apply rule");

        assert!(result.is_some());
        let transform = result.expect("Failed to apply rewrite rule");
        assert!(transform.erase_curr);
        assert_eq!(transform.new_nodes.len(), 1);

        match &transform.new_nodes[0] {
            PlanNodeEnum::ScanEdges(node) => {
                assert_eq!(node.col_names(), &["src", "dst"]);
            }
            _ => panic!("Expectation for the ScanEdges node"),
        }
    }

    #[test]
    fn test_push_down_rule_trait() {
        let rule = PushProjectDownScanEdgesRule::new();

        let scan = PlanNodeEnum::ScanEdges(ScanEdgesNode::new(1, "edge_type"));
        let columns = vec![create_yield_column(
            Expression::Variable("test".to_string()),
            "test",
        )];
        let project =
            ProjectNode::new(scan.clone(), columns).expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        assert!(rule.can_push_down(&project_enum, &scan));
    }
}
