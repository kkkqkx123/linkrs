//! Assignment Statement Planner
//!
//! Query planning for handling variable assignment statements.
//! Supports syntax like: $var = GO FROM 1 OVER friend

use crate::parser::ast::stmt::{AssignmentStmt, Stmt};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::QueryContext;
use std::sync::Arc;

/// Assignment Statement Planner
/// Responsible for converting variable assignment statements into execution plans.
/// The assignment statement executes the right-hand side query and binds the result
/// to the specified variable name in the query context.
#[derive(Debug, Clone)]
pub struct AssignmentPlanner;

impl AssignmentPlanner {
    pub fn new() -> Self {
        Self
    }

    fn extract_assignment_stmt(&self, stmt: &Stmt) -> Result<AssignmentStmt, PlannerError> {
        match stmt {
            Stmt::Assignment(assignment_stmt) => Ok(assignment_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the Assignment".to_string(),
            )),
        }
    }
}

impl Planner for AssignmentPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        match bound {
            crate::binder::BoundStatement::Other(stmt) => {
                let ast = crate::parser::ast::stmt::Ast::new(
                    (**stmt).clone(),
                    std::sync::Arc::new(
                        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                    ),
                );
                let fallback_validated = ValidatedStatement::new(
                    std::sync::Arc::new(ast),
                    validated.validation_info.clone(),
                );
                self.transform(&fallback_validated, qctx)
            }
            _ => self.transform(validated, qctx),
        }
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let assignment_stmt = self.extract_assignment_stmt(validated.stmt())?;

        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::parser::ast::stmt::Ast::new(
                (*assignment_stmt.statement).clone(),
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let mut inner_planner = PlannerEnum::from_stmt(&Arc::new(
            (*assignment_stmt.statement).clone(),
        ))
        .ok_or_else(|| {
            PlannerError::NoSuitablePlanner(format!(
                "assignment inner statement: {}",
                assignment_stmt.statement.kind()
            ))
        })?;

        let inner_plan = inner_planner.transform(&inner_validated, qctx)?;

        log::debug!(
            "AssignmentPlanner: variable '{}' bound to inner plan",
            assignment_stmt.variable
        );

        Ok(inner_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Assignment(_))
    }
}

impl Default for AssignmentPlanner {
    fn default() -> Self {
        Self::new()
    }
}
