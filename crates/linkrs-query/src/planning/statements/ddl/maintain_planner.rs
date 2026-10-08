//! Maintenance Operation Planner
//! Handling query planning related to maintenance tasks (such as SUBMIT JOB, etc.)
//!
//! ## Module Structure
//!
//! - `show` - SHOW / SHOW CREATE / DESC planning
//! - `maintenance` - schema CREATE / ALTER / DROP planning
//! - `system` - transactions, maintenance and LOAD/CALL system planning

use crate::binder::BoundStatement;
use crate::parser::ast::{CreateTarget, Stmt};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
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

    fn current_space(&self, validated: &ValidatedStatement) -> String {
        validated
            .validation_info
            .semantic_info
            .space_name
            .clone()
            .unwrap_or_default()
    }
}

impl Planner for MaintainPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let validated = ctx.validated;
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
            _ => return self.transform(validated, qctx),
        };

        Ok(SubPlan::from_single_node(final_node))
    }

    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        let current_space = self.current_space(validated);

        if let Stmt::Create(create_stmt) = stmt {
            match &create_stmt.target {
                CreateTarget::Node { .. }
                | CreateTarget::Edge { .. }
                | CreateTarget::Path { .. } => {
                    let mut data_planner =
                        crate::planning::statements::dml::create_planner::CreatePlanner::new();
                    return data_planner.transform(validated, qctx);
                }
                _ => {}
            }
        }

        let final_node = match stmt {
            Stmt::Show(show_stmt) => self.plan_show(&show_stmt.target, &current_space),

            Stmt::ShowCreate(show_create_stmt) => {
                self.plan_show_create(&show_create_stmt.target, &current_space)
            }

            Stmt::Create(create_stmt) => {
                if let Some(node) = self.plan_create(
                    &create_stmt.target,
                    create_stmt.if_not_exists,
                    &current_space,
                )? {
                    return Ok(SubPlan::from_single_node(node));
                }
                return Err(PlannerError::UnsupportedOperation(
                    "Create target is not supported by MaintainPlanner".to_string(),
                ));
            }

            Stmt::CreateMacro(create_macro_stmt) => {
                let node = self.plan_create_macro(create_macro_stmt)?;
                return Ok(SubPlan::from_single_node(node));
            }

            Stmt::DropMacro(drop_macro_stmt) => self.plan_drop_macro(drop_macro_stmt),

            Stmt::CreateType(create_type_stmt) => {
                let node = self.plan_create_type(create_type_stmt)?;
                return Ok(SubPlan::from_single_node(node));
            }

            Stmt::DropType(drop_type_stmt) => self.plan_drop_type(drop_type_stmt),

            Stmt::Alter(alter_stmt) => self.plan_alter(&alter_stmt.target, &current_space)?,

            Stmt::ClearSpace(clear_stmt) => system::plan_clear_space(&clear_stmt.space_name),

            Stmt::Desc(desc_stmt) => self.plan_desc(&desc_stmt.target, &current_space),

            Stmt::ShowConfigs(show_configs_stmt) => system::plan_show_configs(show_configs_stmt),

            Stmt::ShowQueries(_) => system::plan_show_queries(),

            Stmt::ShowSessions(_) => system::plan_show_sessions(),

            Stmt::BeginTransaction(_) => system::plan_begin_transaction(),

            Stmt::CommitTransaction(_) => system::plan_commit(),

            Stmt::RollbackTransaction(rollback_stmt) => {
                system::plan_rollback(rollback_stmt.savepoint_name.as_ref())
            }

            Stmt::Savepoint(savepoint_stmt) => system::plan_savepoint(&savepoint_stmt.name),

            Stmt::ReleaseSavepoint(release_stmt) => {
                system::plan_release_savepoint(&release_stmt.name)
            }

            Stmt::Drop(drop_stmt) => {
                self.plan_drop(&drop_stmt.target, drop_stmt.if_exists, &current_space)?
            }

            Stmt::Migrate(_) => system::plan_migrate(),

            Stmt::CommentOn(comment_stmt) => system::plan_comment_on(comment_stmt),

            Stmt::Checkpoint(_) => system::plan_checkpoint(),

            Stmt::ExportDatabase(export_stmt) => system::plan_export_database(export_stmt),

            Stmt::ImportDatabase(import_stmt) => system::plan_import_database(import_stmt),

            Stmt::AttachDatabase(attach_stmt) => system::plan_attach_database(attach_stmt),

            Stmt::DetachDatabase(detach_stmt) => system::plan_detach_database(detach_stmt),

            Stmt::LoadFrom(load_stmt) => {
                return system::plan_load_from(load_stmt, validated, qctx);
            }

            Stmt::InQueryCall(call_stmt) => system::plan_in_query_call(call_stmt),

            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "Statement {:?} is not supported by MaintainPlanner",
                    stmt
                )));
            }
        };

        let sub_plan = SubPlan::from_single_node(final_node);
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(
            stmt,
            Stmt::Show(_)
                | Stmt::ShowCreate(_)
                | Stmt::Create(_)
                | Stmt::Alter(_)
                | Stmt::ClearSpace(_)
                | Stmt::Desc(_)
                | Stmt::Drop(_)
                | Stmt::ShowConfigs(_)
                | Stmt::ShowQueries(_)
                | Stmt::ShowSessions(_)
                | Stmt::BeginTransaction(_)
                | Stmt::CommitTransaction(_)
                | Stmt::RollbackTransaction(_)
                | Stmt::Savepoint(_)
                | Stmt::ReleaseSavepoint(_)
                | Stmt::Migrate(_)
                | Stmt::CommentOn(_)
                | Stmt::Checkpoint(_)
                | Stmt::ExportDatabase(_)
                | Stmt::ImportDatabase(_)
                | Stmt::AttachDatabase(_)
                | Stmt::DetachDatabase(_)
                | Stmt::LoadFrom(_)
                | Stmt::InQueryCall(_)
        )
    }
}

impl Default for MaintainPlanner {
    fn default() -> Self {
        Self::new()
    }
}
