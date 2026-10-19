//! Explain/Profile Statement Planner
//!
//! Query planning for handling EXPLAIN and PROFILE statements.
//! These statements plan the inner query and mark the plan for
//! explain/profile execution mode at the executor layer.

use crate::binder::BoundStatement;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerEnum, PlannerError};

/// Explain/Profile Statement Planner
/// Responsible for converting EXPLAIN/PROFILE statements into execution plans.
/// The planner delegates to the inner statement's planner and marks the result
/// for explain/profile execution at the executor layer.
#[derive(Debug, Clone)]
pub struct ExplainPlanner;

impl ExplainPlanner {
    pub fn new() -> Self {
        Self
    }

    pub fn new_profile() -> Self {
        Self
    }
}

impl Planner for ExplainPlanner {

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
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

        let inner_ctx = crate::planning::context::PlanContext {
            bound: inner_bound,
            qctx: qctx.clone(),
            metadata,
        };
        let inner_plan = inner_planner.plan_bound(&inner_ctx)?;

        let mode = if is_profile { "PROFILE" } else { "EXPLAIN" };
        log::debug!(
            "ExplainPlanner: {} mode, inner plan generated via bound path",
            mode,
        );

        Ok(inner_plan)
    }

}

impl Default for ExplainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
