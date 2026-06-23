//! Explain/Profile Statement Planner
//!
//! Query planning for handling EXPLAIN and PROFILE statements.
//! These statements plan the inner query and mark the plan for
//! explain/profile execution mode at the executor layer.

use crate::query::parser::ast::stmt::{ExplainFormat, Stmt};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

/// Explain/Profile Statement Planner
/// Responsible for converting EXPLAIN/PROFILE statements into execution plans.
/// The planner delegates to the inner statement's planner and marks the result
/// for explain/profile execution at the executor layer.
#[derive(Debug, Clone)]
pub struct ExplainPlanner {
    is_profile: bool,
}

impl ExplainPlanner {
    pub fn new() -> Self {
        Self { is_profile: false }
    }

    pub fn new_profile() -> Self {
        Self { is_profile: true }
    }

    fn extract_inner_stmt(&self, stmt: &Stmt) -> Result<(Box<Stmt>, ExplainFormat), PlannerError> {
        match stmt {
            Stmt::Explain(explain_stmt) => {
                Ok((explain_stmt.statement.clone(), explain_stmt.format.clone()))
            }
            Stmt::Profile(profile_stmt) => {
                Ok((profile_stmt.statement.clone(), profile_stmt.format.clone()))
            }
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain EXPLAIN or PROFILE".to_string(),
            )),
        }
    }
}

impl Planner for ExplainPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let (inner_stmt, format) = self.extract_inner_stmt(validated.stmt())?;

        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::query::parser::ast::stmt::Ast::new(
                (*inner_stmt).clone(),
                validated.ast.expr_context().clone(),
            )),
            validated.validation_info.clone(),
        );

        let mut inner_planner = PlannerEnum::from_stmt(&Arc::new((*inner_stmt).clone()))
            .ok_or_else(|| {
                PlannerError::NoSuitablePlanner(format!(
                    "explain inner statement: {}",
                    inner_stmt.kind()
                ))
            })?;

        let inner_plan = inner_planner.transform(&inner_validated, qctx)?;

        let mode = if self.is_profile {
            "PROFILE"
        } else {
            "EXPLAIN"
        };
        log::debug!(
            "ExplainPlanner: {} mode with format {:?}, inner plan generated",
            mode,
            format
        );

        Ok(inner_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        if self.is_profile {
            matches!(stmt, Stmt::Profile(_))
        } else {
            matches!(stmt, Stmt::Explain(_))
        }
    }
}

impl Default for ExplainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
