//! Assignment Statement Planner
//!
//! Query planning for handling variable assignment statements.
//! Supports syntax like: $var = GO FROM 1 OVER friend

use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerEnum, PlannerError};
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
}

impl Planner for AssignmentPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let assignment = match bound {
            crate::binder::BoundStatement::Assignment(a) => a,
            _ => {
                let stmt = match bound {
                    crate::binder::BoundStatement::Other(s) => s.as_ref(),
                    _ => {
                        return Err(PlannerError::PlanGenerationFailed(
                            "AssignmentPlanner requires a bound AST statement for fallback"
                                .to_string(),
                        ));
                    }
                };
                let ast = Arc::new(crate::parser::ast::stmt::Ast::new(
                    stmt.clone(),
                    Arc::new(
                        linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                    ),
                ));
                let validated = crate::binder::validation::ValidatedStatement::new(
                    ast,
                    crate::binder::validation::ValidationInfo::new(),
                );
                return self.transform(&validated, qctx);
            }
        };

        let mut inner_planner = PlannerEnum::from_bound_statement(&assignment.statement)
            .ok_or_else(|| {
                PlannerError::NoSuitablePlanner(format!(
                    "assignment inner statement: {}",
                    assignment.statement.kind()
                ))
            })?;

        let inner_ctx = crate::planning::context::PlanContext {
            bound: &assignment.statement,
            qctx: qctx.clone(),
            metadata,
        };
        let inner_plan = inner_planner.plan_bound(&inner_ctx)?;

        log::debug!(
            "AssignmentPlanner: variable '{}' bound to inner plan",
            assignment.variable
        );

        Ok(inner_plan)
    }


}

impl Default for AssignmentPlanner {
    fn default() -> Self {
        Self::new()
    }
}
