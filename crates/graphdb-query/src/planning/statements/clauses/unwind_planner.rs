//! UNWIND Sentence Planner
//!
//! Responsible for planning the execution of the UNWIND clause, which expands the list into multiple lines.

use crate::binder::validation::CypherClauseKind;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
use crate::planning::plan::logical::logical_nodes::graph_ops::LogicalUnwindNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::plan_combiner::wrap_logical;
use crate::planning::statements::statement_planner::ClausePlanner;
use crate::QueryContext;
use graphdb_core::types::ContextualExpression;
use std::sync::Arc;

/// UNWIND Sentence Planner
///
/// Responsible for converting UNWIND clauses into execution plan nodes.
/// UNWIND syntax: UNWIND [expression] AS [variable]
#[derive(Debug)]
pub struct UnwindClausePlanner;

impl UnwindClausePlanner {
    pub fn new() -> Self {
        Self
    }
}

impl ClausePlanner for UnwindClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Unwind
    }

    fn transform_clause(
        &self,
        _qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let (expression, variable) = extract_unwind_info(stmt)?;

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The UNWIND clause requires a plan entry".to_string(),
            )
        })?;

        let unwind_node = UnwindNode::new(input_node.clone(), &variable, expression.clone())?;
        let logical_root = wrap_logical(&input_plan, |input| {
            LogicalNodeEnum::Unwind(LogicalUnwindNode {
                id: next_node_id(),
                input: Some(Box::new(input.clone())),
                deps: vec![input],
                alias: variable.clone(),
                list_expression: expression.clone(),
                output_var: None,
                col_names: unwind_node.col_names().to_vec(),
                column_types: vec![],
            })
        });
        Ok(SubPlan {
            root: Some(unwind_node.into_enum()),
            tail: input_plan.tail.clone(),
            logical_root,
        })
    }
}

/// Extract the information about the UNWIND clause from the sentence.
fn extract_unwind_info(stmt: &Stmt) -> Result<(ContextualExpression, String), PlannerError> {
    if let Stmt::Unwind(unwind_stmt) = stmt {
        return Ok((unwind_stmt.expression.clone(), unwind_stmt.variable.clone()));
    }
    Err(PlannerError::PlanGenerationFailed(
        "Expecting UNWIND statements, but getting other types of statements".to_string(),
    ))
}

impl Default for UnwindClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;

    #[test]
    fn test_unwind_clause_planner_creation() {
        let planner = UnwindClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Unwind);
    }

    #[test]
    fn test_extract_unwind_info() {
        use crate::parser::ast::Span;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::Expression;
        use std::sync::Arc;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::List(vec![]);
        let expr_meta = graphdb_core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let unwind_stmt = Stmt::Unwind(crate::parser::ast::stmt::UnwindStmt {
            span: Span::default(),
            expression: ctx_expr.clone(),
            variable: "x".to_string(),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
        });

        let (_expr, var) = extract_unwind_info(&unwind_stmt).expect("failed to extract");
        assert_eq!(var, "x");
    }

    #[test]
    fn test_extract_unwind_info_invalid_stmt() {
        use crate::parser::ast::Span;

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

        let result = extract_unwind_info(&match_stmt);
        assert!(result.is_err());
    }

    #[test]
    fn unwind_clause_preserves_logical_mirror() {
        use crate::parser::ast::Span;
        use crate::planning::plan::core::nodes::StartNode;
        use crate::planning::plan::core::PlanNodeEnum;
        use crate::planning::plan::logical::logical_nodes::access::LogicalStartNode;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use graphdb_core::Expression;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(Expression::List(vec![])));
        let list_expr = ContextualExpression::new(id, ctx);
        let stmt = Stmt::Unwind(crate::parser::ast::stmt::UnwindStmt {
            span: Span::default(),
            expression: list_expr,
            variable: "x".to_string(),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
        });
        let start = PlanNodeEnum::Start(StartNode::new());
        let input = SubPlan {
            root: Some(start.clone()),
            tail: Some(start),
            logical_root: Some(LogicalNodeEnum::Start(LogicalStartNode::new())),
        };
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
        let out = UnwindClausePlanner::new()
            .transform_clause(qctx, &stmt, input)
            .expect("unwind planning should succeed");
        assert!(matches!(out.root, Some(PlanNodeEnum::Unwind(_))));
        assert!(
            matches!(out.logical_root, Some(LogicalNodeEnum::Unwind(_))),
            "unwind must wrap the upstream logical tree"
        );
    }

    #[test]
    fn unwind_clause_stays_physical_without_upstream_logical() {
        use crate::parser::ast::Span;
        use crate::planning::plan::core::nodes::StartNode;
        use crate::planning::plan::core::PlanNodeEnum;
        use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
        use graphdb_core::types::expr::ExpressionMeta;
        use graphdb_core::Expression;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(Expression::List(vec![])));
        let list_expr = ContextualExpression::new(id, ctx);
        let stmt = Stmt::Unwind(crate::parser::ast::stmt::UnwindStmt {
            span: Span::default(),
            expression: list_expr,
            variable: "x".to_string(),
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
        });
        let start = PlanNodeEnum::Start(StartNode::new());
        let input = SubPlan {
            root: Some(start.clone()),
            tail: Some(start),
            logical_root: None,
        };
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
        let out = UnwindClausePlanner::new()
            .transform_clause(qctx, &stmt, input)
            .expect("unwind planning should succeed");
        assert!(matches!(out.root, Some(PlanNodeEnum::Unwind(_))));
        assert!(out.logical_root.is_none());
    }
}
