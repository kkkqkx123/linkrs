//! The WHERE clause planner
//!
//! Responsible for planning the execution of the WHERE clause and filtering the input data.
//! The ClausePlanner interface has been implemented, providing comprehensive filtering capabilities.

use crate::binder::validation::CypherClauseKind;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::plan_combiner::wrap_logical;
use crate::planning::statements::statement_planner::ClausePlanner;
use crate::QueryContext;
use graphdb_core::types::ContextualExpression;
use std::sync::Arc;

/// The WHERE clause planner
///
/// Responsible for planning the execution of the WHERE clause and filtering the input data.
#[derive(Debug, Clone)]
pub struct WhereClausePlanner;

impl Default for WhereClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl WhereClausePlanner {
    pub fn new() -> Self {
        Self
    }
}

impl ClausePlanner for WhereClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Where
    }

    fn transform_clause(
        &self,
        qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let condition = extract_where_condition(stmt)?;

        // Extract conjunctive EXISTS / IN subqueries. When none are present
        // the classic filter-only path is unchanged.
        let condition_expr = condition.expression().map(|e| e.inner().clone());
        let mut specs = Vec::new();
        let residual_expr =
            condition_expr.map(|e| exists_planner::extract_conjunctive_exists(&e, &mut specs));

        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names = input_plan
            .root()
            .as_ref()
            .map(|root| root.col_names().to_vec())
            .unwrap_or_default();

        // Unified entry for expression-level EXISTS / IN: any subquery left
        // in the residual condition (OR positions, containers, ...) is
        // compiled here and attached to the residual filter node. Compilation
        // failure returns a precise PlannerError instead of leaking to the
        // runtime "not supported" path.
        let mut id_alloc = exists_planner::SubqueryIdAllocator::new();
        let mut residual_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let residual_expr = match &residual_expr {
            Some(residual) => {
                let (planned_expr, subqueries) = exists_planner::plan_expression_subqueries(
                    residual.clone(),
                    &qctx,
                    space_id,
                    &space_name,
                    &outer_col_names,
                    &mut id_alloc,
                )?;
                residual_subqueries = subqueries;
                Some(planned_expr)
            }
            None => None,
        };

        if specs.is_empty() {
            // No conjunctive subqueries: the classic filter path is unchanged
            // when the residual is also subquery-free.
            if residual_subqueries.is_empty() {
                return plan_simple_filter(condition, input_plan);
            }
            let Some(residual_expr) = residual_expr else {
                return plan_simple_filter(condition, input_plan);
            };
            let context = condition.context().clone();
            let residual_ctx = exists_planner::to_contextual(residual_expr, &context);
            return plan_simple_filter_with(residual_ctx, input_plan, residual_subqueries);
        }

        let residual_expr = residual_expr.expect("residual exists alongside specs");

        let mut plan = input_plan;
        for spec in &specs {
            // The outer columns become the Argument layout of a correlated
            // right subtree; for the key-based path they are unused.
            let outer_col_names = plan
                .root()
                .as_ref()
                .map(|root| root.col_names().to_vec())
                .unwrap_or_default();
            let planned = exists_planner::plan_subquery(
                spec,
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
            )?;
            plan = if let Some(condition) = &planned.mark_join_condition {
                exists_planner::wrap_mark_join(plan, &planned, condition, spec.negated)?
            } else if planned.correlated {
                exists_planner::wrap_correlated_apply(plan, &planned, spec.negated)?
            } else {
                exists_planner::wrap_pattern_apply(plan, &planned, spec.negated)?
            };
        }

        if !exists_planner::is_trivially_true(&residual_expr) {
            let context = condition.context().clone();
            let residual_ctx = exists_planner::to_contextual(residual_expr, &context);
            plan = plan_simple_filter_with(residual_ctx, plan, residual_subqueries)?;
        }

        Ok(plan)
    }
}

/// Classic path: a plain filter node over the input.
fn plan_simple_filter(
    condition: ContextualExpression,
    input_plan: SubPlan,
) -> Result<SubPlan, PlannerError> {
    let input_node = input_plan.root().as_ref().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The WHERE clause requires an input plan".to_string())
    })?;

    let filter_node = FilterNode::new(input_node.clone(), condition.clone())?;
    let logical_root = wrap_logical(&input_plan, |input| {
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    });
    Ok(SubPlan {
        root: Some(filter_node.into_enum()),
        tail: input_plan.tail,
        logical_root,
    })
}

/// Apply a prepared filter condition on top of a plan, carrying any
/// expression-level subqueries compiled for the condition.
fn plan_simple_filter_with(
    condition: ContextualExpression,
    input_plan: SubPlan,
    subqueries: Vec<exists_planner::PlannedSubquery>,
) -> Result<SubPlan, PlannerError> {
    let input_node = input_plan.root().as_ref().ok_or_else(|| {
        PlannerError::PlanGenerationFailed("The WHERE clause requires an input plan".to_string())
    })?;

    let filter_node =
        FilterNode::new(input_node.clone(), condition.clone())?.with_subqueries(subqueries);
    let logical_root = wrap_logical(&input_plan, |input| {
        LogicalNodeEnum::Filter(LogicalFilterNode {
            id: next_node_id(),
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            condition,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        })
    });
    Ok(SubPlan {
        root: Some(filter_node.into_enum()),
        tail: input_plan.tail,
        logical_root,
    })
}

fn extract_where_condition(stmt: &Stmt) -> Result<ContextualExpression, PlannerError> {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(ref where_expr) = match_stmt.where_clause {
            return Ok(where_expr.clone());
        }
    }
    Err(PlannerError::PlanGenerationFailed(
        "The WHERE clause should create a default expression at the Parser level".to_string(),
    ))
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::parser::ast::Span;
    use crate::planning::plan::core::nodes::StartNode;
    use crate::planning::plan::core::PlanNodeEnum;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::Expression;
    use std::sync::Arc;

    #[test]
    fn test_where_clause_planner_creation() {
        let planner = WhereClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Where);
    }

    #[test]
    fn test_extract_where_condition() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: Some(ctx_expr.clone()),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            join_hint: None,
            optional: false,
            delete_clause: None,
        });

        let condition = extract_where_condition(&match_stmt).expect("failed to extract");
        assert_eq!(condition.id(), ctx_expr.id());
    }

    #[test]
    fn test_extract_where_condition_none() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            join_hint: None,
            optional: false,
            delete_clause: None,
        });

        let result = extract_where_condition(&match_stmt);
        assert!(result.is_err());
    }

    #[test]
    fn test_transform_clause() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: Some(ctx_expr),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            join_hint: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
            logical_root: None,
        };

        let planner = WhereClausePlanner::new();
        let qctx = Arc::new(crate::QueryContext::new(Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_ok());

        let sub_plan = result.expect("transform_clause should succeed");
        assert!(sub_plan.root.is_some());

        if let Some(PlanNodeEnum::Filter(_)) = sub_plan.root {
        } else {
            panic!("Expected FilterNode");
        }
    }

    #[test]
    fn test_transform_clause_invalid_stmt() {
        let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            join_hint: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
            logical_root: None,
        };

        let planner = WhereClausePlanner::new();
        let qctx = Arc::new(crate::QueryContext::new(Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_err());
    }
}
