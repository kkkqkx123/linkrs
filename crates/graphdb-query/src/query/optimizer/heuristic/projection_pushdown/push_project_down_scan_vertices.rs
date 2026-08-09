//! ScanVertices: Optimization rules for projection downscaling
//!
//! This rule pushes the projection operation down to the ScanVertices node, thereby reducing the amount of data transmitted.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//! Project(col1, col2)
//!         |
//!     ScanVertices
//! ```
//!
//! After:
//! ```text
//! ScanVertices(col1, col2)
//! ```

use crate::core::types::expr::extract_property_refs;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::ScanVerticesNode;
use crate::query::planning::plan::PlanNodeEnum;

/// ScanVertices Projection Pushdown Rule
///
/// Push the projection operation down to the ScanVertices node.
#[derive(Debug)]
pub struct PushProjectDownScanVerticesRule;

impl PushProjectDownScanVerticesRule {
    pub fn new() -> Self {
        Self
    }

    fn can_push_down_project(
        project_node: &crate::query::planning::plan::core::nodes::ProjectNode,
    ) -> bool {
        !project_node.columns().is_empty()
    }

    fn create_scan_vertices_with_projection(
        &self,
        scan_node: &ScanVerticesNode,
        project_columns: &[crate::core::YieldColumn],
    ) -> ScanVerticesNode {
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

impl Default for PushProjectDownScanVerticesRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownScanVerticesRule {
    fn name(&self) -> &'static str {
        "PushProjectDownScanVerticesRule"
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
            PlanNodeEnum::ScanVertices(scan_node) => {
                let new_scan =
                    self.create_scan_vertices_with_projection(scan_node, project_node.columns());
                if new_scan.projected_properties() == scan_node.projected_properties() {
                    return Ok(None);
                }
                PlanNodeEnum::ScanVertices(new_scan)
            }
            PlanNodeEnum::Filter(filter) => {
                // Walk the chain of Filters down to the ScanVertices node. The
                // planner may interleave a tag-filter (labels(in, tag)) between
                // the WHERE-filter and the scan, so the scan may be deeper than
                // one level. The filter directly above the scan is included as
                // well, since its condition may reference projected values.
                let mut chain: Vec<PlanNodeEnum> = Vec::new();
                let mut current = PlanNodeEnum::Filter(filter.clone());
                let scan_node = loop {
                    match &current {
                        PlanNodeEnum::Filter(f) => {
                            chain.push(current.clone());
                            current = f.input().clone();
                        }
                        PlanNodeEnum::ScanVertices(s) => break s.clone(),
                        _ => return Ok(None),
                    }
                };
                let mut new_scan =
                    self.create_scan_vertices_with_projection(&scan_node, project_node.columns());
                for level in &chain {
                    let PlanNodeEnum::Filter(f) = level else { unreachable!() };
                    let mut props = new_scan.projected_properties().to_vec();
                    props.extend(extract_property_refs(f.condition()));
                    props.sort();
                    props.dedup();
                    new_scan.set_projected_properties(props);
                }
                if new_scan.projected_properties() == scan_node.projected_properties() {
                    return Ok(None);
                }
                let mut input = PlanNodeEnum::ScanVertices(new_scan);
                for level in chain.into_iter().rev() {
                    let mut f = match level {
                        PlanNodeEnum::Filter(f) => f,
                        _ => unreachable!(),
                    };
                    f.set_input(input);
                    input = PlanNodeEnum::Filter(f);
                }
                input
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

impl PushDownRule for PushProjectDownScanVerticesRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        match node {
            PlanNodeEnum::Project(project) => {
                if project.columns().is_empty() {
                    return false;
                }
                matches!(target, PlanNodeEnum::ScanVertices(_))
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
    use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
    use crate::core::types::ContextualExpression;
    use crate::core::{Expression, YieldColumn};
    use crate::query::planning::plan::core::nodes::{ProjectNode, ScanVerticesNode};
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
        let rule = PushProjectDownScanVerticesRule::new();
        assert_eq!(rule.name(), "PushProjectDownScanVerticesRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownScanVerticesRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_apply_with_scan_vertices() {
        let rule = PushProjectDownScanVerticesRule::new();
        let mut ctx = RewriteContext::new();

        let scan_node = ScanVerticesNode::new(1, "default");
        let scan = PlanNodeEnum::ScanVertices(scan_node);

        let columns = vec![
            create_yield_column(Expression::property(Expression::variable("p"), "id"), "id"),
            create_yield_column(
                Expression::property(Expression::variable("p"), "name"),
                "name",
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
                PlanNodeEnum::ScanVertices(scan) => {
                    assert_eq!(scan.projected_properties(), &["id", "name"]);
                }
                _ => panic!("Expected ScanVertices below Project"),
            },
            _ => panic!("Expected Project to be preserved"),
        }
    }

    #[test]
    fn test_push_down_rule_trait() {
        let rule = PushProjectDownScanVerticesRule::new();

        let scan = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));
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
