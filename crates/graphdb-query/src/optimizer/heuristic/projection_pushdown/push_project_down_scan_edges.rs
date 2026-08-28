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

use graphdb_core::types::expr::extract_property_refs;
use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::planning::plan::core::nodes::ScanEdgesNode;
use crate::planning::plan::PlanNodeEnum;

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
        project_node: &crate::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_scan_edges_with_projection(
        &self,
        scan_node: &ScanEdgesNode,
        project_columns: &[graphdb_core::YieldColumn],
    ) -> ScanEdgesNode {
        let mut properties: Vec<String> = project_columns
            .iter()
            .flat_map(|column| extract_property_refs(&column.expression))
            .collect();
        properties.sort();
        properties.dedup();

        let mut new_node = scan_node.clone();
        new_node.set_projected_properties(properties);
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

        let mut new_project = project_node.clone();
        let new_input = match project_node.input() {
            PlanNodeEnum::ScanEdges(scan_node) => {
                let new_scan =
                    self.create_scan_edges_with_projection(scan_node, project_node.columns());
                if new_scan.projected_properties() == scan_node.projected_properties() {
                    return Ok(None);
                }
                PlanNodeEnum::ScanEdges(new_scan)
            }
            PlanNodeEnum::Filter(filter) => {
                let PlanNodeEnum::ScanEdges(scan_node) = filter.input() else {
                    return Ok(None);
                };
                let mut new_scan =
                    self.create_scan_edges_with_projection(scan_node, project_node.columns());
                let mut properties = new_scan.projected_properties().to_vec();
                properties.extend(extract_property_refs(filter.condition()));
                properties.sort();
                properties.dedup();
                if properties == scan_node.projected_properties() {
                    return Ok(None);
                }
                new_scan.set_projected_properties(properties);
                let mut new_filter = filter.clone();
                new_filter.set_input(PlanNodeEnum::ScanEdges(new_scan));
                PlanNodeEnum::Filter(new_filter)
            }
            _ => return Ok(None),
        };
        new_project.set_input(new_input);
        let new_node = PlanNodeEnum::Project(new_project);

        let mut result = TransformResult::new();
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
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::ContextualExpression;
    use graphdb_core::{Expression, YieldColumn};
    use crate::planning::plan::core::nodes::{ProjectNode, ScanEdgesNode};
    use std::sync::Arc;

    fn create_yield_column(expr: Expression, alias: &str) -> YieldColumn {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
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
            create_yield_column(
                Expression::property(Expression::variable("e"), "src"),
                "src",
            ),
            create_yield_column(
                Expression::property(Expression::variable("e"), "dst"),
                "dst",
            ),
        ];
        let project =
            ProjectNode::new(scan.clone(), columns).expect("Failed to create ProjectNode");
        let project_enum = PlanNodeEnum::Project(project);

        let result = rule
            .apply(&mut ctx, &project_enum)
            .expect("Failed to apply rule");

        assert!(result.is_some());
        let transform = result.expect("Failed to apply rewrite rule");
        assert!(!transform.erase_curr);
        assert_eq!(transform.new_nodes.len(), 1);

        match &transform.new_nodes[0] {
            PlanNodeEnum::Project(node) => match node.input() {
                PlanNodeEnum::ScanEdges(scan) => {
                    assert_eq!(scan.projected_properties(), &["dst", "src"]);
                }
                _ => panic!("Expected ScanEdges below Project"),
            },
            _ => panic!("Expected Project to be preserved"),
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
