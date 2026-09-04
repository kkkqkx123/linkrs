//! Explain/Profile Statement Planner
//!
//! Query planning for handling EXPLAIN and PROFILE statements.
//! These statements plan the inner query and mark the plan for
//! explain/profile execution mode at the executor layer.

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerEnum, PlannerError, ValidatedStatement};
use crate::QueryContext;
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

    fn extract_inner_stmt(&self, stmt: &Stmt) -> Result<Box<Stmt>, PlannerError> {
        match stmt {
            Stmt::Explain(explain_stmt) => Ok(explain_stmt.statement.clone()),
            Stmt::Profile(profile_stmt) => Ok(profile_stmt.statement.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain EXPLAIN or PROFILE".to_string(),
            )),
        }
    }

    /// Plan the inner statement via the AST-based path (for backward compatibility).
    fn plan_inner_ast(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        _metadata_context: Option<&crate::metadata::MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let inner_stmt = self.extract_inner_stmt(validated.stmt())?;

        let inner_validated = ValidatedStatement::new(
            Arc::new(crate::parser::ast::stmt::Ast::new(
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

        log::debug!("ExplainPlanner: inner plan generated via AST path",);

        Ok(inner_plan)
    }
}

impl Planner for ExplainPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        self.plan_inner_ast(validated, qctx, None)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let (inner_bound, is_profile) = match bound {
            BoundStatement::Explain(e) => (e.statement.as_ref(), false),
            BoundStatement::Profile(p) => (p.statement.as_ref(), true),
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "ExplainPlanner requires BoundStatement::Explain or Profile".to_string(),
                ));
            }
        };

        let mut inner_planner =
            PlannerEnum::from_bound_statement(inner_bound).ok_or_else(|| {
                PlannerError::NoSuitablePlanner(format!(
                    "explain inner statement: {}",
                    inner_bound.kind()
                ))
            })?;

        let inner_validated = ctx.derive_validated(inner_bound);
        let inner_ctx = crate::planning::context::PlanContext {
            bound: inner_bound,
            qctx: qctx.clone(),
            metadata,
            validated: inner_validated.as_ref().unwrap_or(validated),
        };
        let inner_plan = inner_planner.plan_bound(&inner_ctx)?;

        let mode = if is_profile { "PROFILE" } else { "EXPLAIN" };
        log::debug!(
            "ExplainPlanner: {} mode, inner plan generated via bound path",
            mode,
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
