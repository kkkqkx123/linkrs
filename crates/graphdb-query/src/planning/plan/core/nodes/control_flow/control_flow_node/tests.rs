use std::sync::Arc;

use super::*;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::ContextualExpression;

#[test]
fn test_argument_node_creation() {
    let node = ArgumentNode::new(1, "var_name");
    assert_eq!(node.type_name(), "ArgumentNode");
    assert_eq!(node.id(), 1);
    assert_eq!(node.var(), "var_name");
}

#[test]
fn test_select_node_creation() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
        graphdb_core::Expression::Variable("condition".to_string()),
    );
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = ContextualExpression::new(id, ctx);
    let node = SelectNode::new(1, ctx_expr);
    assert_eq!(node.type_name(), "Select");
    assert_eq!(node.id(), 1);
    assert!(node.if_branch().is_none());
    assert!(node.else_branch().is_none());
}

#[test]
fn test_loop_node_creation() {
    let ctx = Arc::new(ExpressionAnalysisContext::new());
    let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(
        graphdb_core::Expression::Variable("condition".to_string()),
    );
    let id = ctx.register_expression(expr_meta);
    let ctx_expr = ContextualExpression::new(id, ctx);
    let node = LoopNode::new(1, ctx_expr);
    assert_eq!(node.type_name(), "Loop");
    assert_eq!(node.id(), 1);
    assert!(node.body().is_none());
}

#[test]
fn test_pass_through_node_creation() {
    let node = PassThroughNode::new(1);
    assert_eq!(node.type_name(), "PassThroughNode");
    assert_eq!(node.id(), 1);
}
