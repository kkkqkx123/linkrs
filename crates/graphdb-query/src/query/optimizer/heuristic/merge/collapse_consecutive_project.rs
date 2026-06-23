//! Merge consecutive projection rules
//!
//! When multiple Project nodes appear in succession, they should be merged into a single Project node.
//! Reduce the generation of unnecessary intermediate results.
//!
//! Example:
//! ```
//! Project(a, b) -> Project(c, d)  =>  Project(c, d)
//! ```
//!
//! Applicable Conditions:
//! Two Project nodes appear in succession.
//! The upper-level project does not rely on the alias resolution of the lower-level project.

use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::expression_utils::rewrite_contextual_expression;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use std::collections::HashMap;

/// Merge consecutive projection rules
///
/// # Example of conversion
///
/// Before:
/// ```text
///   Project(col2)
///       |
///   Project(col1)
///       |
///   ScanVertices
/// ```
///
/// After:
/// ```text
///   Project(col2)
///       |
///   ScanVertices
/// ```
///
/// # Applicable Conditions
///
/// The current node is a Project node.
/// The child node is also a Project node.
/// The column references from the upper-level project can be parsed as inputs for the lower-level project.
#[derive(Debug)]
pub struct CollapseConsecutiveProjectRule;

impl CollapseConsecutiveProjectRule {
    /// Create a rule instance
    pub fn new() -> Self {
        Self
    }

    /// Perform the merge operation.
    fn merge_projects(
        &self,
        parent_proj: &ProjectNode,
        child_proj: &ProjectNode,
        ctx: &RewriteContext,
    ) -> Option<ProjectNode> {
        // Construct a mapping from column names to expressions (from the sub-project)
        let mut rewrite_map = HashMap::new();
        for col in child_proj.columns() {
            if !col.alias.is_empty() {
                rewrite_map.insert(col.alias.clone(), col.expression.clone());
            }
        }

        let expr_context = ctx.expr_context();

        // Rewrite the list expression of the parent Project.
        let new_columns: Vec<YieldColumn> = parent_proj
            .columns()
            .iter()
            .map(|col| YieldColumn {
                expression: rewrite_contextual_expression(
                    &col.expression,
                    &rewrite_map,
                    expr_context.clone(),
                ),
                alias: col.alias.clone(),
                is_matched: col.is_matched,
            })
            .collect();

        // Create a new Project node, and enter the information for the sub-Project as input.
        let child_input = child_proj.input().clone();
        ProjectNode::new(child_input, new_columns).ok()
    }
}

impl Default for CollapseConsecutiveProjectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for CollapseConsecutiveProjectRule {
    fn name(&self) -> &'static str {
        "CollapseConsecutiveProjectRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project").with_dependency_name("Project")
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Check whether it is a Project node.
        let parent_proj = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Obtain child nodes
        let child_node = parent_proj.input();
        let child_proj = match child_node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        // Perform the merge.
        if let Some(new_proj) = self.merge_projects(parent_proj, child_proj, ctx) {
            let mut result = TransformResult::new();
            result.erase_curr = true;
            result.add_new_node(PlanNodeEnum::Project(new_proj));
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

impl MergeRule for CollapseConsecutiveProjectRule {
    fn can_merge(&self, parent: &PlanNodeEnum, child: &PlanNodeEnum) -> bool {
        parent.is_project() && child.is_project()
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
    use crate::core::Expression;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_rule_name() {
        let rule = CollapseConsecutiveProjectRule::new();
        assert_eq!(rule.name(), "CollapseConsecutiveProjectRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = CollapseConsecutiveProjectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_collapse_consecutive_projects() {
        use crate::core::types::expr::ExpressionMeta;
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        // Create the starting node.
        let start = PlanNodeEnum::Start(StartNode::new());

        // Create the context for the expression.
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // Create a lower-level Project node.
        let a_expr = Expression::Variable("a".to_string());
        let a_meta = ExpressionMeta::new(a_expr);
        let a_id = expr_ctx.register_expression(a_meta);
        let a_ctx_expr = ContextualExpression::new(a_id, expr_ctx.clone());

        let b_expr = Expression::Variable("b".to_string());
        let b_meta = ExpressionMeta::new(b_expr);
        let b_id = expr_ctx.register_expression(b_meta);
        let b_ctx_expr = ContextualExpression::new(b_id, expr_ctx.clone());

        let child_columns = vec![
            YieldColumn {
                expression: a_ctx_expr,
                alias: "col_a".to_string(),
                is_matched: false,
            },
            YieldColumn {
                expression: b_ctx_expr,
                alias: "col_b".to_string(),
                is_matched: false,
            },
        ];
        let child_proj =
            ProjectNode::new(start, child_columns).expect("Failed to create lower-level Project");
        let child_node = PlanNodeEnum::Project(child_proj);

        // Create an upper-level Project node that references the alias of the lower-level Project.
        let col_a_expr = Expression::Variable("col_a".to_string());
        let col_a_meta = ExpressionMeta::new(col_a_expr);
        let col_a_id = expr_ctx.register_expression(col_a_meta);
        let col_a_ctx_expr = ContextualExpression::new(col_a_id, expr_ctx);

        let parent_columns = vec![YieldColumn {
            expression: col_a_ctx_expr,
            alias: "result".to_string(),
            is_matched: false,
        }];
        let parent_proj = ProjectNode::new(child_node, parent_columns)
            .expect("Failed to create upper-level Project");
        let parent_node = PlanNodeEnum::Project(parent_proj);

        // Apply the rules
        let rule = CollapseConsecutiveProjectRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &parent_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The consecutive Project nodes should be merged successfully."
        );

        // Verification results
        let transform_result = result.expect("Failed to apply rewrite rule");
        assert!(transform_result.erase_curr);
        assert_eq!(transform_result.new_nodes.len(), 1);

        // Verify the new Project node.
        if let PlanNodeEnum::Project(ref new_proj) = transform_result.new_nodes[0] {
            let columns = new_proj.columns();
            assert_eq!(columns.len(), 1);
            assert_eq!(columns[0].alias, "result");
            // The verification expression has been rewritten to match the original reference.
            if let Some(expr_meta) = columns[0].expression.expression() {
                if let Expression::Variable(name) = expr_meta.inner() {
                    assert_eq!(name, "a");
                } else {
                    panic!("The expression should be “Variable”.");
                }
            } else {
                panic!("The expression should exist.");
            }
        } else {
            panic!("The “Project” node");
        }
    }
}
