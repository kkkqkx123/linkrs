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
            BoundStatement::Other(stmt) => match stmt.as_ref() {
                crate::parser::ast::Stmt::Create(create) => self
                    .plan_create(&create.target, create.if_not_exists, &space)?
                    .ok_or_else(|| {
                        PlannerError::UnsupportedOperation(
                            "MaintainPlanner has no plan for this CREATE target".to_string(),
                        )
                    })?,
                crate::parser::ast::Stmt::CreateMacro(s) => self.plan_create_macro(s)?,
                crate::parser::ast::Stmt::CreateType(s) => self.plan_create_type(s)?,
                crate::parser::ast::Stmt::DropMacro(s) => self.plan_drop_macro(s),
                crate::parser::ast::Stmt::DropType(s) => self.plan_drop_type(s),
                crate::parser::ast::Stmt::AttachDatabase(s) => self.plan_attach_database(s),
                crate::parser::ast::Stmt::DetachDatabase(s) => self.plan_detach_database(s),
                crate::parser::ast::Stmt::PurgeExpired(s) => self.plan_purge_expired(s),
                crate::parser::ast::Stmt::Checkpoint(_) => self.plan_checkpoint(),
                crate::parser::ast::Stmt::KillQuery(_) => {
                    return Err(PlannerError::UnsupportedOperation(
                        "KILL QUERY executes in the network service against live query tracking and is not plannable here; use the unary query endpoint with a query id from SHOW QUERIES".to_string(),
                    ));
                }
                crate::parser::ast::Stmt::ExportDatabase(s) => self.plan_export_database(s),
                crate::parser::ast::Stmt::ImportDatabase(s) => self.plan_import_database(s),
                _ => {
                    return Err(PlannerError::UnsupportedOperation(format!(
                        "MaintainPlanner has no plan for bound statement: {}",
                        bound.kind()
                    )));
                }
            },
            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "MaintainPlanner has no plan for bound statement: {}",
                    bound.kind()
                )));
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
