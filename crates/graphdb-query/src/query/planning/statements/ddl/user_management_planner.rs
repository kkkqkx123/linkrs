//! User Management Planner
//! Handling query planning related to user management (CREATE USER, ALTER USER, DROP USER, CHANGE PASSWORD)

use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode;
use crate::query::planning::plan::core::{ArgumentNode, PlanNodeEnum};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;
#[derive(Debug, Clone)]
pub struct UserManagementPlanner;

impl UserManagementPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for UserManagementPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let arg_node = ArgumentNode::new(1, "user_management_args");

        let final_node = match validated.stmt() {
            Stmt::CreateUser(create_stmt) => {
                let mut node = crate::query::planning::plan::core::nodes::CreateUserNode::new(
                    1,
                    create_stmt.username.clone(),
                    create_stmt.password.clone(),
                );
                if let Some(ref role) = create_stmt.role {
                    node = node.with_role(role.clone());
                }
                node = node.with_if_not_exists(create_stmt.if_not_exists);
                PlanNodeEnum::UserManage(UserManageNode::Create(node))
            }
            Stmt::AlterUser(alter_stmt) => {
                let mut node = crate::query::planning::plan::core::nodes::AlterUserNode::new(
                    2,
                    alter_stmt.username.clone(),
                );
                if let Some(ref role) = alter_stmt.new_role {
                    node = node.with_role(role.clone());
                }
                if let Some(locked) = alter_stmt.is_locked {
                    node = node.with_locked(locked);
                }
                PlanNodeEnum::UserManage(UserManageNode::Alter(node))
            }
            Stmt::DropUser(drop_stmt) => {
                let node = crate::query::planning::plan::core::nodes::DropUserNode::new(
                    3,
                    drop_stmt.username.clone(),
                )
                .with_if_exists(drop_stmt.if_exists);
                PlanNodeEnum::UserManage(UserManageNode::Drop(node))
            }
            Stmt::ChangePassword(change_stmt) => {
                let password_info = crate::core::types::PasswordInfo {
                    username: change_stmt.username.clone(),
                    old_password: change_stmt.old_password.clone(),
                    new_password: change_stmt.new_password.clone(),
                };

                let node = crate::query::planning::plan::core::nodes::ChangePasswordNode::new(
                    4,
                    password_info,
                );
                PlanNodeEnum::UserManage(UserManageNode::ChangePassword(node))
            }
            Stmt::Grant(grant_stmt) => {
                let node = crate::query::planning::plan::core::nodes::GrantRoleNode::new(
                    5,
                    grant_stmt.username.clone(),
                    grant_stmt.space_name.clone(),
                    format!("{:?}", grant_stmt.role).to_lowercase(),
                );
                PlanNodeEnum::UserManage(UserManageNode::GrantRole(node))
            }
            Stmt::Revoke(revoke_stmt) => {
                let node = crate::query::planning::plan::core::nodes::RevokeRoleNode::new(
                    6,
                    revoke_stmt.username.clone(),
                    revoke_stmt.space_name.clone(),
                );
                PlanNodeEnum::UserManage(UserManageNode::RevokeRole(node))
            }
            Stmt::ShowUsers(_) => {
                let node = crate::query::planning::plan::core::nodes::ShowUsersNode::new(7);
                PlanNodeEnum::UserManage(UserManageNode::ShowUsers(node))
            }
            Stmt::ShowRoles(show_roles_stmt) => {
                let space_name = show_roles_stmt.space_name.clone().unwrap_or_default();
                let node =
                    crate::query::planning::plan::core::nodes::ShowRolesNode::new(8, space_name);
                PlanNodeEnum::UserManage(UserManageNode::ShowRoles(node))
            }
            Stmt::DescribeUser(desc_user_stmt) => {
                let node = crate::query::planning::plan::core::nodes::DescribeUserNode::new(
                    9,
                    desc_user_stmt.username.clone(),
                );
                PlanNodeEnum::UserManage(UserManageNode::DescribeUser(node))
            }
            _ => {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "Unsupported user management operation: {:?}",
                    validated.stmt()
                )));
            }
        };

        let sub_plan = SubPlan::new(Some(final_node), Some(PlanNodeEnum::Argument(arg_node)));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(
            stmt,
            Stmt::CreateUser(_)
                | Stmt::AlterUser(_)
                | Stmt::DropUser(_)
                | Stmt::ChangePassword(_)
                | Stmt::Grant(_)
                | Stmt::Revoke(_)
                | Stmt::ShowUsers(_)
                | Stmt::ShowRoles(_)
                | Stmt::DescribeUser(_)
        )
    }
}

impl Default for UserManagementPlanner {
    fn default() -> Self {
        Self::new()
    }
}
