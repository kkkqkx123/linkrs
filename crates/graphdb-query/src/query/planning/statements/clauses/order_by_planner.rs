//! ORDER BY Clause Planner
//!
//! Responsible for planning the execution of the ORDER BY clause and sorting the results.
//! Supports both simple column references and complex expressions (e.g., function calls).

use crate::core::types::ContextualExpression;
use crate::query::parser::ast::Stmt;
use crate::query::parser::OrderByItem;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::operation::sort_node::{SortItem, SortNode};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
use std::sync::Arc;

/// The ORDER BY clause planner
///
/// Responsible for planning the execution of the ORDER BY clause and sorting the results.
#[derive(Debug, Clone)]
pub struct OrderByClausePlanner {}

impl Default for OrderByClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderByClausePlanner {
    pub fn new() -> Self {
        Self {}
    }
}

fn extract_order_by_items(stmt: &Stmt) -> Vec<OrderByItem> {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(order_by_clause) = &match_stmt.order_by {
            return order_by_clause.items.clone();
        }
    }
    Vec::new()
}

/// Extract the expression from a ContextualExpression.
///
/// Returns a clone of the inner Expression, or a Variable expression as fallback.
fn extract_expression(expr: &ContextualExpression) -> crate::core::Expression {
    if let Some(expr_meta) = expr.expression() {
        expr_meta.inner().clone()
    } else {
        // Fallback: create a variable expression from the string representation
        let expr_string = expr
            .expression()
            .map(|e| e.inner().to_expression_string())
            .unwrap_or_default();
        crate::core::Expression::Variable(expr_string)
    }
}

impl ClausePlanner for OrderByClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::OrderBy
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let order_by_items = extract_order_by_items(stmt);

        if order_by_items.is_empty() {
            return Ok(input_plan);
        }

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The ORDER BY clause requires an input plan".to_string(),
            )
        })?;

        let sort_items: Vec<SortItem> = order_by_items
            .into_iter()
            .map(|item| {
                let expression = extract_expression(&item.expression);
                SortItem::new(expression, item.direction)
            })
            .collect();

        let sort_node = SortNode::new(input_node.clone(), sort_items)?;
        Ok(SubPlan::new(Some(sort_node.into_enum()), input_plan.tail))
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::OrderDirection;
    use crate::core::Expression;
    use crate::query::parser::ast::{OrderByItem, Span};
    use crate::query::planning::plan::core::nodes::StartNode;
    use crate::query::planning::plan::core::PlanNodeEnum;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use std::sync::Arc;

    #[test]
    fn test_order_by_clause_planner_creation() {
        let planner = OrderByClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::OrderBy);
    }

    #[test]
    fn test_extract_order_by_items() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: Some(crate::query::parser::ast::stmt::OrderByClause {
                span: Span::default(),
                items: vec![OrderByItem {
                    expression: ctx_expr.clone(),
                    direction: OrderDirection::Asc,
                }],
            }),
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let items = extract_order_by_items(&match_stmt);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].direction, OrderDirection::Asc);
    }

    #[test]
    fn test_extract_order_by_items_empty() {
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

        let items = extract_order_by_items(&match_stmt);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn test_extract_expression() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let result = extract_expression(&ctx_expr);
        assert_eq!(result, Expression::Variable("age".to_string()));
    }

    #[test]
    fn test_extract_expression_complex() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "name".to_string(),
        };
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr.clone());
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let result = extract_expression(&ctx_expr);
        assert_eq!(result, expr);
    }

    #[test]
    fn test_extract_expression_function_call() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Function {
            name: "cosine_similarity".to_string(),
            args: vec![
                Expression::Variable("a".to_string()),
                Expression::Variable("b".to_string()),
            ],
        };
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr.clone());
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let result = extract_expression(&ctx_expr);
        assert_eq!(result, expr);
    }

    #[test]
    fn test_transform_clause() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: Some(crate::query::parser::ast::stmt::OrderByClause {
                span: Span::default(),
                items: vec![OrderByItem {
                    expression: ctx_expr.clone(),
                    direction: OrderDirection::Asc,
                }],
            }),
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

        let planner = OrderByClausePlanner::new();
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

        if let Some(PlanNodeEnum::Sort(_)) = sub_plan.root {
        } else {
            panic!("Expected SortNode");
        }
    }

    #[test]
    fn test_transform_clause_empty_order_by() {
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

        let planner = OrderByClausePlanner::new();
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
    }

    #[test]
    fn test_transform_clause_empty_input_plan() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::Variable("age".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(id, ctx);

        let match_stmt = Stmt::Match(crate::query::parser::ast::stmt::MatchStmt {
            span: Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: Some(crate::query::parser::ast::stmt::OrderByClause {
                span: Span::default(),
                items: vec![OrderByItem {
                    expression: ctx_expr.clone(),
                    direction: OrderDirection::Asc,
                }],
            }),
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let input_plan = SubPlan {
            root: None,
            tail: None,
        };

        let planner = OrderByClausePlanner::new();
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
