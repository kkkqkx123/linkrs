//! Attribute/Tag Remover Planner
//!
//! Query planning for handling the REMOVE statement
//!
//! REMOVE has no storage write path yet, so both planning entries fail
//! explicitly instead of producing a plan that only strips row columns.

use crate::binder::BoundStatement;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};

/// Attribute/Tag Remover Planner
/// Responsible for converting the REMOVE statement into an execution plan.
#[derive(Debug, Clone)]
pub struct RemovePlanner;

impl RemovePlanner {
    /// Create a new deletion planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for RemovePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        match bound {
            BoundStatement::Remove(_) => {}
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain a REMOVE".to_string(),
                ));
            }
        };

        // Standalone REMOVE has no storage write path yet: fail explicitly
        // instead of producing a plan that only strips row columns.
        Err(PlannerError::PlanGenerationFailed(
            "REMOVE is not wired to storage yet".to_string(),
        ))
    }


}

impl Default for RemovePlanner {
    fn default() -> Self {
        Self::new()
    }
}
