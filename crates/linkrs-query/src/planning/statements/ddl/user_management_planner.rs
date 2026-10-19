//! User Management Planner
//! Handling query planning related to user management (CREATE USER, ALTER USER, DROP USER, CHANGE PASSWORD)

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode;
use crate::planning::plan::core::{ArgumentNode, PlanNodeEnum};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use std::sync::Arc;
#[derive(Debug, Clone)]
pub struct UserManagementPlanner;

impl UserManagementPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for UserManagementPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let arg_node = ArgumentNode::new(1, "user_management_args");

        let final_node = match bound {
            BoundStatement::CreateUser(create) => {
                let node = crate::planning::plan::core::nodes::CreateUserNode::new(
                    1,
                    create.username.clone(),
                    create.password.clone(),
                )
                .with_if_not_exists(create.if_not_exists);
                PlanNodeEnum::UserManage(UserManageNode::Create(node))
            }
            BoundStatement::AlterUser(alter) => {
                let mut node = crate::planning::plan::core::nodes::AlterUserNode::new(
                    2,
                    alter.username.clone(),
                )
                .with_password_history_depth(qctx.password_history_depth());
                if let Some(ref password) = alter.password {
                    node = node.with_password(password.clone());
                }
                if let Some(locked) = alter.is_locked {
                    node = node.with_locked(locked);
                }
                PlanNodeEnum::UserManage(UserManageNode::Alter(node))
            }
            BoundStatement::DropUser(drop) => {
                let node =
                    crate::planning::plan::core::nodes::DropUserNode::new(3, drop.username.clone())
                        .with_if_exists(drop.if_exists);
                PlanNodeEnum::UserManage(UserManageNode::Drop(node))
            }
            _ => {
                let stmt = match bound {
                    crate::binder::BoundStatement::Other(s) => s.as_ref(),
                    _ => {
                        return Err(PlannerError::PlanGenerationFailed(
                            "UserManagementPlanner requires a bound AST statement for fallback"
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
                return self.transform(&validated, qctx.clone());
            }
        };

        let sub_plan = SubPlan::new(Some(final_node), Some(PlanNodeEnum::Argument(arg_node)));
        Ok(sub_plan)
    }


}

impl Default for UserManagementPlanner {
    fn default() -> Self {
        Self::new()
    }
}
