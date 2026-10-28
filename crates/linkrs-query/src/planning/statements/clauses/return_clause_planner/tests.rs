use super::*;
use crate::parser::ast::Span;
use crate::planning::plan::core::nodes::StartNode;
use crate::planning::plan::core::PlanNodeEnum;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
use linkrs_core::Expression;
use std::sync::Arc;

#[test]
fn test_return_clause_planner_creation() {
    let planner = ReturnClausePlanner::new();
    assert_eq!(planner.clause_kind(), CypherClauseKind::Return);
}

#[test]
fn test_return_clause_planner_with_distinct() {
    let planner = ReturnClausePlanner::with_distinct(true);
    assert!(planner.distinct);
}

#[test]
fn test_extract_distinct_flag() {
    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![],
            distinct: true,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let distinct = extract_distinct_flag(&match_stmt);
    assert!(distinct);
}

#[test]
fn test_extract_return_columns() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Variable("n".to_string());
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = linkrs_core::types::ContextualExpression::new(id, ctx);

    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![crate::parser::ast::stmt::ReturnItem::Expression {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            distinct: false,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let columns = extract_return_columns(&match_stmt).expect("failed to extract");
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].alias, "n");
}

#[test]
fn test_extract_return_columns_empty() {
    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![],
            distinct: false,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let result = extract_return_columns(&match_stmt);
    assert!(result.is_err());
}

#[test]
fn test_generate_default_alias() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Variable("n".to_string());
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let contextual = ContextualExpression::new(id, ctx.clone());
    let alias = linkrs_core::types::expr::expression_utils::generate_default_alias_from_contextual(
        &contextual,
    );
    assert_eq!(alias, "n");

    let expr = Expression::Property {
        object: Box::new(Expression::Variable("n".to_string())),
        property: "name".to_string(),
    };
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let contextual = ContextualExpression::new(id, ctx.clone());
    let alias = linkrs_core::types::expr::expression_utils::generate_default_alias_from_contextual(
        &contextual,
    );
    assert_eq!(alias, "n.name");

    let expr = Expression::Function {
        name: "count".to_string(),
        args: vec![],
    };
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let contextual = ContextualExpression::new(id, ctx.clone());
    let alias = linkrs_core::types::expr::expression_utils::generate_default_alias_from_contextual(
        &contextual,
    );
    assert_eq!(alias, "count");
}

#[test]
fn test_transform_clause() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Variable("n".to_string());
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = linkrs_core::types::ContextualExpression::new(id, ctx);

    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![crate::parser::ast::stmt::ReturnItem::Expression {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            distinct: false,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let start_node = StartNode::new();
    let start_node_enum = PlanNodeEnum::Start(start_node.clone());
    let input_plan = SubPlan::from_dual(Some(start_node_enum.clone()), Some(start_node_enum), None);

    let planner = ReturnClausePlanner::new();
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

    match sub_plan.root {
        Some(PlanNodeEnum::Project(_)) => {}
        Some(PlanNodeEnum::Dedup(_)) => {}
        _ => panic!("Expected ProjectNode or DedupNode"),
    }
}

#[test]
fn test_transform_clause_with_distinct() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Variable("n".to_string());
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = linkrs_core::types::ContextualExpression::new(id, ctx);

    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![crate::parser::ast::stmt::ReturnItem::Expression {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            distinct: true,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let start_node = StartNode::new();
    let start_node_enum = PlanNodeEnum::Start(start_node.clone());
    let input_plan = SubPlan::from_dual(Some(start_node_enum.clone()), Some(start_node_enum), None);

    let planner = ReturnClausePlanner::with_distinct(true);
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

    if let Some(PlanNodeEnum::Dedup(_)) = sub_plan.root {
    } else {
        panic!("Expected DedupNode with distinct=true");
    }
}

#[test]
fn test_transform_clause_empty_input_plan() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr = Expression::Variable("n".to_string());
    let expr_meta = linkrs_core::types::expr::ExpressionMeta::new(expr);
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = linkrs_core::types::ContextualExpression::new(id, ctx);

    let match_stmt = Stmt::Match(crate::parser::ast::stmt::MatchStmt {
        span: Span::default(),
        patterns: vec![],
        where_clause: None,
        return_clause: Some(crate::parser::ast::stmt::ReturnClause {
            span: Span::default(),
            items: vec![crate::parser::ast::stmt::ReturnItem::Expression {
                expression: ctx_expr.clone(),
                alias: None,
            }],
            distinct: false,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        }),
        order_by: None,
        limit: None,
        skip: None,
        join_hint: None,
        optional: false,
        delete_clause: None,
    });

    let input_plan = SubPlan::from_dual(None, None, None);

    let planner = ReturnClausePlanner::new();
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
