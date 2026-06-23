//! Rules for combining multiple projection operations

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::expr::ExpressionMeta;
use crate::core::YieldColumn;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::expression_utils::rewrite_contextual_expression;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{MergeRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::validator::context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Rules for combining multiple projection operations
///
/// # Conversion example
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
pub struct CollapseProjectRule;

impl CollapseProjectRule {
    /// Create a rule instance.
    pub fn new() -> Self {
        Self
    }

    /// Check whether the expression represents a simple property reference.
    fn is_property_expr(expr: &ContextualExpression) -> bool {
        let expr_meta = match expr.expression() {
            Some(e) => e,
            None => return false,
        };
        let inner_expr = expr_meta.inner();
        matches!(
            inner_expr,
            Expression::Variable(_) | Expression::Property { .. }
        )
    }

    /// Collect all attribute references in the expression.
    fn collect_property_refs(expr: &ContextualExpression, refs: &mut Vec<String>) {
        let expr_meta = match expr.expression() {
            Some(e) => e,
            None => return,
        };
        let inner_expr = expr_meta.inner();

        match inner_expr {
            Expression::Variable(name) => refs.push(name.clone()),
            Expression::Property { object, property } => {
                if let Expression::Variable(obj_name) = object.as_ref() {
                    refs.push(format!("{}.{}", obj_name, property));
                } else {
                    refs.push(property.clone());
                }
            }
            Expression::Binary { left, right, .. } => {
                // A ContextualExpression needs to be created in order to perform the recursion.
                let left_meta = ExpressionMeta::new((**left).clone());
                let right_meta = ExpressionMeta::new((**right).clone());
                let left_ctx = Arc::new(ExpressionAnalysisContext::new());
                let left_id = left_ctx.register_expression(left_meta);
                let right_id = left_ctx.register_expression(right_meta);
                let left_expr = ContextualExpression::new(left_id, left_ctx.clone());
                let right_expr = ContextualExpression::new(right_id, left_ctx);
                Self::collect_property_refs(&left_expr, refs);
                Self::collect_property_refs(&right_expr, refs);
            }
            Expression::Unary { operand, .. } => {
                let operand_meta = ExpressionMeta::new((**operand).clone());
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                let id = ctx.register_expression(operand_meta);
                let operand_expr = ContextualExpression::new(id, ctx);
                Self::collect_property_refs(&operand_expr, refs);
            }
            Expression::Function { args, .. } => {
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                for arg in args {
                    let arg_meta = ExpressionMeta::new(arg.clone());
                    let id = ctx.register_expression(arg_meta);
                    let arg_expr = ContextualExpression::new(id, ctx.clone());
                    Self::collect_property_refs(&arg_expr, refs);
                }
            }
            Expression::Aggregate { arg, .. } => {
                let arg_meta = ExpressionMeta::new((**arg).clone());
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                let id = ctx.register_expression(arg_meta);
                let arg_expr = ContextualExpression::new(id, ctx);
                Self::collect_property_refs(&arg_expr, refs);
            }
            Expression::List(list) => {
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                for item in list {
                    let item_meta = ExpressionMeta::new(item.clone());
                    let id = ctx.register_expression(item_meta);
                    let item_expr = ContextualExpression::new(id, ctx.clone());
                    Self::collect_property_refs(&item_expr, refs);
                }
            }
            Expression::Map(map) => {
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                for (_, value) in map {
                    let value_meta = ExpressionMeta::new(value.clone());
                    let id = ctx.register_expression(value_meta);
                    let value_expr = ContextualExpression::new(id, ctx.clone());
                    Self::collect_property_refs(&value_expr, refs);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let ctx = Arc::new(ExpressionAnalysisContext::new());
                if let Some(test) = test_expr {
                    let test_meta = ExpressionMeta::new((**test).clone());
                    let id = ctx.register_expression(test_meta);
                    let test_expr = ContextualExpression::new(id, ctx.clone());
                    Self::collect_property_refs(&test_expr, refs);
                }
                for (when, then) in conditions {
                    let when_meta = ExpressionMeta::new(when.clone());
                    let then_meta = ExpressionMeta::new(then.clone());
                    let when_id = ctx.register_expression(when_meta);
                    let then_id = ctx.register_expression(then_meta);
                    let when_expr = ContextualExpression::new(when_id, ctx.clone());
                    let then_expr = ContextualExpression::new(then_id, ctx.clone());
                    Self::collect_property_refs(&when_expr, refs);
                    Self::collect_property_refs(&then_expr, refs);
                }
                if let Some(else_e) = default {
                    let else_meta = ExpressionMeta::new((**else_e).clone());
                    let id = ctx.register_expression(else_meta);
                    let else_expr = ContextualExpression::new(id, ctx);
                    Self::collect_property_refs(&else_expr, refs);
                }
            }
            _ => {}
        }
    }
}

impl Default for CollapseProjectRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for CollapseProjectRule {
    fn name(&self) -> &'static str {
        "CollapseProjectRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project").with_dependency_name("Project")
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let parent_proj = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        let child_node = parent_proj.input();
        let child_proj = match child_node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        let parent_cols = parent_proj.columns();
        let child_cols = child_proj.columns();

        // Collect all property references from the upper-level Project.
        let mut all_prop_refs: Vec<String> = Vec::new();
        for col in parent_cols {
            Self::collect_property_refs(&col.expression, &mut all_prop_refs);
        }

        // Check for any duplicate citations.
        let mut unique_refs = std::collections::HashSet::new();
        let mut multi_ref_cols = std::collections::HashSet::new();
        for prop_ref in &all_prop_refs {
            if !unique_refs.insert(prop_ref.clone()) {
                multi_ref_cols.insert(prop_ref.clone());
            }
        }

        // Construct a rewrite mapping: Column name -> ContextualExpression
        let mut rewrite_map = std::collections::HashMap::new();
        let child_col_names = child_proj.col_names();

        for (i, col_name) in child_col_names.iter().enumerate() {
            if unique_refs.contains(col_name) {
                let col_expr = &child_cols[i].expression;
                // If a column is referenced multiple times and does not represent a simple attribute expression, then this optimization should be disabled.
                if !Self::is_property_expr(col_expr) && multi_ref_cols.contains(col_name) {
                    return Ok(None);
                }
                rewrite_map.insert(col_name.clone(), col_expr.clone());
            }
        }

        let expr_context = ctx.expr_context();

        // Rewrite the columns of the upper-level Project
        let new_columns: Vec<YieldColumn> = parent_cols
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

        // Create a new Project node, and enter the information for the subordinate Project as the input.
        let new_input = child_proj.input().clone();
        let new_proj = match ProjectNode::new(new_input, new_columns) {
            Ok(node) => node,
            Err(_) => return Ok(None),
        };

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::Project(new_proj));

        Ok(Some(result))
    }
}

impl MergeRule for CollapseProjectRule {
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
    use crate::core::types::expr::ExpressionMeta;
    use crate::core::YieldColumn;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
    use std::sync::Arc;
    use ExpressionAnalysisContext;

    #[test]
    fn test_rule_name() {
        let rule = CollapseProjectRule::new();
        assert_eq!(rule.name(), "CollapseProjectRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = CollapseProjectRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }

    #[test]
    fn test_collapse_simple_project() {
        let start = PlanNodeEnum::Start(StartNode::new());
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());

        // Lower-level Project: col1
        let child_expr = Expression::Variable("a".to_string());
        let child_meta = ExpressionMeta::new(child_expr);
        let child_id = expr_ctx.register_expression(child_meta);
        let child_ctx_expr = ContextualExpression::new(child_id, expr_ctx.clone());

        let child_columns = vec![YieldColumn {
            expression: child_ctx_expr,
            alias: "col1".to_string(),
            is_matched: false,
        }];
        let child_proj =
            ProjectNode::new(start, child_columns).expect("Failed to create ProjectNode");
        let child_node = PlanNodeEnum::Project(child_proj);

        // Upper-level project: col2 = col1
        let parent_expr = Expression::Variable("col1".to_string());
        let parent_meta = ExpressionMeta::new(parent_expr);
        let parent_id = expr_ctx.register_expression(parent_meta);
        let parent_ctx_expr = ContextualExpression::new(parent_id, expr_ctx);

        let parent_columns = vec![YieldColumn {
            expression: parent_ctx_expr,
            alias: "col2".to_string(),
            is_matched: false,
        }];
        let parent_proj = ProjectNode::new(child_node.clone(), parent_columns)
            .expect("Failed to create ProjectNode");
        let parent_node = PlanNodeEnum::Project(parent_proj);

        // Apply the rules.
        let rule = CollapseProjectRule::new();
        let mut ctx = RewriteContext::new();
        let result = rule
            .apply(&mut ctx, &parent_node)
            .expect("Failed to apply rule");

        assert!(
            result.is_some(),
            "The folding of the two Project nodes should succeed."
        );
    }
}
