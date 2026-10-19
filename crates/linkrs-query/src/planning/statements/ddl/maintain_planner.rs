//! Maintenance Operation Planner
//! Handling query planning related to maintenance tasks (such as SUBMIT JOB, etc.)
//!
//! ## Module Structure
//!
//! - `show` - SHOW / SHOW CREATE / DESC planning
//! - `maintenance` - schema CREATE / ALTER / DROP planning
//! - `system` - transactions, maintenance and LOAD/CALL system planning

use crate::binder::BoundStatement;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use std::sync::Arc;

mod maintenance;
mod show;
mod system;

#[derive(Debug, Clone)]
pub struct MaintainPlanner;

impl MaintainPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for MaintainPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let space = qctx
            .space_name()
            .or_else(|| qctx.request_context().space_name.clone())
            .unwrap_or_default();

        let final_node = match bound {
            BoundStatement::Show(s) => self.plan_show(&s.target, &space),
            BoundStatement::ShowCreate(s) => self.plan_show_create(&s.target, &space),
            BoundStatement::Drop(s) => self.plan_drop(&s.target, s.if_exists, &space)?,
            BoundStatement::Alter(s) => self.plan_alter(&s.target, &space)?,
            BoundStatement::Desc(s) => self.plan_desc(&s.target, &space),
            BoundStatement::ClearSpace(s) => system::plan_clear_space(&s.space_name),
            BoundStatement::BeginTransaction(_) => system::plan_begin_transaction(),
            BoundStatement::Commit(_) => system::plan_commit(),
            BoundStatement::Rollback(r) => system::plan_rollback(r.savepoint_name.as_ref()),
            BoundStatement::Savepoint(s) => system::plan_savepoint(&s.name),
            BoundStatement::ReleaseSavepoint(s) => system::plan_release_savepoint(&s.name),
            _ => {
                let stmt = match bound {
                    crate::binder::BoundStatement::Other(s) => s.as_ref(),
                    _ => {
                        return Err(PlannerError::PlanGenerationFailed(
                            "MaintainPlanner requires a bound AST statement for fallback"
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

        Ok(SubPlan::from_single_node(final_node))
    }


}

impl Default for MaintainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
