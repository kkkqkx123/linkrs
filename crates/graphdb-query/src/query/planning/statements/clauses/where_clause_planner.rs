//! The WHERE clause planner
//!
//! Responsible for planning the execution of the WHERE clause and filtering the input data.
//! The ClausePlanner interface has been implemented, providing comprehensive filtering capabilities.

use crate::core::types::ContextualExpression;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
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
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let condition = extract_where_condition(stmt)?;

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The WHERE clause requires an input plan".to_string(),
            )
        })?;

        let filter_node = FilterNode::new(input_node.clone(), condition)?;
        Ok(SubPlan::new(Some(filter_node.into_enum()), input_plan.tail))
    }
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
    use crate::core::Expression;
    use crate::query::parser::ast::Span;
    use crate::query::planning::plan::core::nodes::StartNode;
    use crate::query::planning::plan::core::PlanNodeEnum;
    use crate::query::validator::context::ExpressionAnalysisContext;
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
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: Some(ctx_expr.clone()),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let condition = extract_where_condition(&match_stmt).expect("failed to extract");
        assert_eq!(condition.id(), ctx_expr.id());
    }

    #[test]
    fn test_extract_where_condition_none() {
        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
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
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: Some(ctx_expr),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let planner = WhereClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
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
        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let start_node = StartNode::new();
        let start_node_enum = PlanNodeEnum::Start(start_node.clone());
        let input_plan = SubPlan {
            root: Some(start_node_enum.clone()),
            tail: Some(start_node_enum),
        };

        let planner = WhereClausePlanner::new();
        let qctx = Arc::new(crate::query::QueryContext::new(Arc::new(
            crate::query::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
            },
        )));

        let result = planner.transform_clause(qctx, &match_stmt, input_plan);
        assert!(result.is_err());
    }
}
