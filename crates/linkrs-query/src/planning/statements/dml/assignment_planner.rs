//! Assignment Statement Planner
//!
//! Query planning for handling variable assignment statements.
//! Supports syntax like: $var = GO FROM 1 OVER friend

use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerEnum, PlannerError};

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
        let assignment = match bound {
            crate::binder::BoundStatement::Assignment(a) => a,
            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "AssignmentPlanner has no plan for bound statement: {}",
                    bound.kind()
                )));
            }
        };

        let inner_plan = PlannerEnum::plan_substatement(ctx, &assignment.statement)?;

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
