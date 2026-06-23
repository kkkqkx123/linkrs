//! Rules for pushing projection operations down to JOIN
//!
//! This rule pushes the projection operation down to both sides of the JOIN, thereby reducing the amount of intermediate data.
//!
//! # Conversion example
//!
//! Before:
//! ```text
//!   Project(col1, col2, col3)
//!           |
//!   HashInnerJoin
//!   /          \
//! Left        Right
//! (col1-col5) (col2-col6)
//! ```
//!
//! After:
//! ```text
//!   HashInnerJoin
//!   /          \
//! Project     Project
//! (col1)      (col2, col3)
//!   |            |
//! Left        Right
//! ```
//!
//! # Applicable Conditions
//!
//! The Project node is located above the JOIN node.
//! The projection columns can be separated into left and right sides.
//! The JOIN keys are retained in the projection.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::visitor::ExpressionVisitor;
use crate::core::types::expr::visitor_collectors::VariableCollector;
use crate::core::types::YieldColumn;
use crate::core::Expression;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::result::{RewriteError, RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::{PushDownRule, RewriteRule};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::join::join_node::{
    HashInnerJoinNode, HashLeftJoinNode, InnerJoinNode, LeftJoinNode,
};
use crate::query::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::query::planning::plan::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Rules for pushing projection operations down to JOIN
///
/// Push the projection operation down to both sides of the JOIN to reduce intermediate results.
#[derive(Debug)]
pub struct PushProjectDownJoinRule;

impl PushProjectDownJoinRule {
    pub fn new() -> Self {
        Self
    }

    fn collect_variables(&self, expr: &Expression) -> Vec<String> {
        let mut collector = VariableCollector::new();
        collector.visit(expr);
        collector.variables
    }

    fn get_columns_from_expr(&self, expr: &ContextualExpression) -> Vec<String> {
        if let Some(expr_meta) = expr.expression() {
            self.collect_variables(expr_meta.inner())
        } else {
            Vec::new()
        }
    }

    fn get_columns_from_yield_column(&self, col: &YieldColumn) -> Vec<String> {
        let mut vars = self.get_columns_from_expr(&col.expression);
        vars.push(col.alias.clone());
        vars
    }

    fn create_project_node(
        &self,
        input: PlanNodeEnum,
        columns: Vec<String>,
        ctx: Arc<ExpressionAnalysisContext>,
    ) -> Result<PlanNodeEnum, RewriteError> {
        let yield_columns: Vec<YieldColumn> = columns
            .into_iter()
            .map(|col| {
                let expr = Expression::variable(&col);
                let meta = crate::core::types::expr::ExpressionMeta::new(expr);
                let id = ctx.register_expression(meta);
                let ctx_expr = ContextualExpression::new(id, ctx.clone());
                YieldColumn {
                    expression: ctx_expr,
                    alias: col,
                    is_matched: false,
                }
            })
            .collect();

        if yield_columns.is_empty() {
            return Ok(input);
        }

        ProjectNode::new(input, yield_columns)
            .map(PlanNodeEnum::Project)
            .map_err(|e| {
                RewriteError::rewrite_failed(format!("Failed to create ProjectNode: {:?}", e))
            })
    }

    fn split_columns_for_join(
        &self,
        project_columns: &[YieldColumn],
        left_col_names: &[String],
        right_col_names: &[String],
        hash_keys: &[ContextualExpression],
        probe_keys: &[ContextualExpression],
    ) -> (Vec<String>, Vec<String>) {
        let mut left_needed: Vec<String> = Vec::new();
        let mut right_needed: Vec<String> = Vec::new();

        for col in project_columns {
            let vars = self.get_columns_from_yield_column(col);
            for var in vars {
                if left_col_names.contains(&var) && !left_needed.contains(&var) {
                    left_needed.push(var.clone());
                }
                if right_col_names.contains(&var) && !right_needed.contains(&var) {
                    right_needed.push(var);
                }
            }
        }

        for key in hash_keys {
            let vars = self.get_columns_from_expr(key);
            for var in vars {
                if left_col_names.contains(&var) && !left_needed.contains(&var) {
                    left_needed.push(var);
                }
            }
        }

        for key in probe_keys {
            let vars = self.get_columns_from_expr(key);
            for var in vars {
                if right_col_names.contains(&var) && !right_needed.contains(&var) {
                    right_needed.push(var);
                }
            }
        }

        (left_needed, right_needed)
    }

    fn apply_to_hash_inner_join(
        &self,
        project: &ProjectNode,
        join: &HashInnerJoinNode,
        ctx: &RewriteContext,
    ) -> RewriteResult<Option<TransformResult>> {
        let left_col_names = join.left_input().col_names().to_vec();
        let right_col_names = join.right_input().col_names().to_vec();

        let (left_needed, right_needed) = self.split_columns_for_join(
            project.columns(),
            &left_col_names,
            &right_col_names,
            join.hash_keys(),
            join.probe_keys(),
        );

        if left_needed.is_empty() && right_needed.is_empty() {
            return Ok(None);
        }

        if left_needed.len() == left_col_names.len() && right_needed.len() == right_col_names.len()
        {
            return Ok(None);
        }

        let expr_ctx = ctx.expr_context();
        let mut new_left = join.left_input().clone();
        let mut new_right = join.right_input().clone();

        if !left_needed.is_empty() && left_needed.len() < left_col_names.len() {
            new_left = self.create_project_node(new_left, left_needed, expr_ctx.clone())?;
        }

        if !right_needed.is_empty() && right_needed.len() < right_col_names.len() {
            new_right = self.create_project_node(new_right, right_needed, expr_ctx)?;
        }

        let new_join = HashInnerJoinNode::new(
            new_left,
            new_right,
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create HashInnerJoinNode: {:?}", e))
        })?;

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::HashInnerJoin(new_join));

        Ok(Some(result))
    }

    fn apply_to_hash_left_join(
        &self,
        project: &ProjectNode,
        join: &HashLeftJoinNode,
        ctx: &RewriteContext,
    ) -> RewriteResult<Option<TransformResult>> {
        let left_col_names = join.left_input().col_names().to_vec();
        let right_col_names = join.right_input().col_names().to_vec();

        let (left_needed, right_needed) = self.split_columns_for_join(
            project.columns(),
            &left_col_names,
            &right_col_names,
            join.hash_keys(),
            join.probe_keys(),
        );

        if left_needed.is_empty() && right_needed.is_empty() {
            return Ok(None);
        }

        if left_needed.len() == left_col_names.len() && right_needed.len() == right_col_names.len()
        {
            return Ok(None);
        }

        let expr_ctx = ctx.expr_context();
        let mut new_left = join.left_input().clone();
        let mut new_right = join.right_input().clone();

        if !left_needed.is_empty() && left_needed.len() < left_col_names.len() {
            new_left = self.create_project_node(new_left, left_needed, expr_ctx.clone())?;
        }

        if !right_needed.is_empty() && right_needed.len() < right_col_names.len() {
            new_right = self.create_project_node(new_right, right_needed, expr_ctx)?;
        }

        let new_join = HashLeftJoinNode::new(
            new_left,
            new_right,
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create HashLeftJoinNode: {:?}", e))
        })?;

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::HashLeftJoin(new_join));

        Ok(Some(result))
    }

    fn apply_to_inner_join(
        &self,
        project: &ProjectNode,
        join: &InnerJoinNode,
        ctx: &RewriteContext,
    ) -> RewriteResult<Option<TransformResult>> {
        let left_col_names = join.left_input().col_names().to_vec();
        let right_col_names = join.right_input().col_names().to_vec();

        let (left_needed, right_needed) = self.split_columns_for_join(
            project.columns(),
            &left_col_names,
            &right_col_names,
            join.hash_keys(),
            join.probe_keys(),
        );

        if left_needed.is_empty() && right_needed.is_empty() {
            return Ok(None);
        }

        if left_needed.len() == left_col_names.len() && right_needed.len() == right_col_names.len()
        {
            return Ok(None);
        }

        let expr_ctx = ctx.expr_context();
        let mut new_left = join.left_input().clone();
        let mut new_right = join.right_input().clone();

        if !left_needed.is_empty() && left_needed.len() < left_col_names.len() {
            new_left = self.create_project_node(new_left, left_needed, expr_ctx.clone())?;
        }

        if !right_needed.is_empty() && right_needed.len() < right_col_names.len() {
            new_right = self.create_project_node(new_right, right_needed, expr_ctx)?;
        }

        let new_join = InnerJoinNode::new(
            new_left,
            new_right,
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create InnerJoinNode: {:?}", e))
        })?;

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::InnerJoin(new_join));

        Ok(Some(result))
    }

    fn apply_to_left_join(
        &self,
        project: &ProjectNode,
        join: &LeftJoinNode,
        ctx: &RewriteContext,
    ) -> RewriteResult<Option<TransformResult>> {
        let left_col_names = join.left_input().col_names().to_vec();
        let right_col_names = join.right_input().col_names().to_vec();

        let (left_needed, right_needed) = self.split_columns_for_join(
            project.columns(),
            &left_col_names,
            &right_col_names,
            join.hash_keys(),
            join.probe_keys(),
        );

        if left_needed.is_empty() && right_needed.is_empty() {
            return Ok(None);
        }

        if left_needed.len() == left_col_names.len() && right_needed.len() == right_col_names.len()
        {
            return Ok(None);
        }

        let expr_ctx = ctx.expr_context();
        let mut new_left = join.left_input().clone();
        let mut new_right = join.right_input().clone();

        if !left_needed.is_empty() && left_needed.len() < left_col_names.len() {
            new_left = self.create_project_node(new_left, left_needed, expr_ctx.clone())?;
        }

        if !right_needed.is_empty() && right_needed.len() < right_col_names.len() {
            new_right = self.create_project_node(new_right, right_needed, expr_ctx)?;
        }

        let new_join = LeftJoinNode::new(
            new_left,
            new_right,
            join.hash_keys().to_vec(),
            join.probe_keys().to_vec(),
        )
        .map_err(|e| {
            RewriteError::rewrite_failed(format!("Failed to create LeftJoinNode: {:?}", e))
        })?;

        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(PlanNodeEnum::LeftJoin(new_join));

        Ok(Some(result))
    }
}

impl Default for PushProjectDownJoinRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for PushProjectDownJoinRule {
    fn name(&self) -> &'static str {
        "PushProjectDownJoinRule"
    }

    fn pattern(&self) -> Pattern {
        Pattern::new_with_name("Project")
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        let project = match node {
            PlanNodeEnum::Project(n) => n,
            _ => return Ok(None),
        };

        let input = project.input();

        match input {
            PlanNodeEnum::HashInnerJoin(join) => self.apply_to_hash_inner_join(project, join, ctx),
            PlanNodeEnum::HashLeftJoin(join) => self.apply_to_hash_left_join(project, join, ctx),
            PlanNodeEnum::InnerJoin(join) => self.apply_to_inner_join(project, join, ctx),
            PlanNodeEnum::LeftJoin(join) => self.apply_to_left_join(project, join, ctx),
            _ => Ok(None),
        }
    }
}

impl PushDownRule for PushProjectDownJoinRule {
    fn can_push_down(&self, node: &PlanNodeEnum, target: &PlanNodeEnum) -> bool {
        matches!(node, PlanNodeEnum::Project(_))
            && matches!(
                target,
                PlanNodeEnum::HashInnerJoin(_)
                    | PlanNodeEnum::HashLeftJoin(_)
                    | PlanNodeEnum::InnerJoin(_)
                    | PlanNodeEnum::LeftJoin(_)
            )
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

    #[test]
    fn test_rule_name() {
        let rule = PushProjectDownJoinRule::new();
        assert_eq!(rule.name(), "PushProjectDownJoinRule");
    }

    #[test]
    fn test_rule_pattern() {
        let rule = PushProjectDownJoinRule::new();
        let pattern = rule.pattern();
        assert!(pattern.node.is_some());
    }
}
