//! Attribute/Tag Remover Planner
//!
//! Query planning for handling the REMOVE statement
//!
//! Standalone REMOVE is rejected with a user-facing explanation that names
//! the supported alternative, instead of producing a plan that only strips
//! row columns.

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

        // Standalone REMOVE has no storage write path: fail with a user-facing
        // explanation that names the supported alternative.
        Err(PlannerError::PlanGenerationFailed(
            "standalone REMOVE is not supported; use DELETE to remove vertices and edges instead"
                .to_string(),
        ))
    }
}

impl Default for RemovePlanner {
    fn default() -> Self {
        Self::new()
    }
}
