//! UNWIND Sentence Planner
//!
//! Responsible for planning the execution of the UNWIND clause, which expands the list into multiple lines.

use crate::core::types::ContextualExpression;
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode;
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::PlannerError;
use crate::query::planning::statements::statement_planner::ClausePlanner;
use crate::query::validator::structs::CypherClauseKind;
use crate::query::QueryContext;
use std::sync::Arc;

/// UNWIND Sentence Planner
///
/// Responsible for converting UNWIND clauses into execution plan nodes.
/// UNWIND 语法：UNWIND [expression] AS [variable]
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

        let unwind_node = UnwindNode::new(input_node.clone(), &variable, expression)?;
        Ok(SubPlan::new(Some(unwind_node.into_enum()), input_plan.tail))
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
mod tests {
    use super::*;

    #[test]
    fn test_unwind_clause_planner_creation() {
        let planner = UnwindClausePlanner::new();
        assert_eq!(planner.clause_kind(), CypherClauseKind::Unwind);
    }

    #[test]
    fn test_extract_unwind_info() {
        use crate::core::Expression;
        use crate::query::parser::ast::Span;
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr = Expression::List(vec![]);
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);

        let unwind_stmt = Stmt::Unwind(crate::query::parser::ast::stmt::UnwindStmt {
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
        use crate::query::parser::ast::Span;

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

        let result = extract_unwind_info(&match_stmt);
        assert!(result.is_err());
    }
}
