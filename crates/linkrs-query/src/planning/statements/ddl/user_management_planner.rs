//! User Management Planner
//! Handling query planning related to user management (CREATE USER, ALTER USER, DROP USER, CHANGE PASSWORD)
//! and to role assignment (GRANT, REVOKE) plus the role/user introspection
//! statements (SHOW USERS, SHOW ROLES, DESCRIBE USER).

use crate::binder::BoundStatement;
use crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode;
use crate::planning::plan::core::{ArgumentNode, PlanNodeEnum};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
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
            BoundStatement::ChangePassword(change) => {
                // A statement without a username rotates the password of the
                // authenticated caller.
                let username = change
                    .username
                    .clone()
                    .or_else(|| qctx.authenticated_user().map(|user| user.to_string()));
                let password_info = linkrs_core::types::PasswordInfo {
                    username,
                    old_password: change.old_password.clone(),
                    new_password: change.new_password.clone(),
                    history_limit: qctx.password_history_depth(),
                };

                let node =
                    crate::planning::plan::core::nodes::ChangePasswordNode::new(4, password_info);
                PlanNodeEnum::UserManage(UserManageNode::ChangePassword(node))
            }
            BoundStatement::Grant(grant) => {
                let node = crate::planning::plan::core::nodes::GrantRoleNode::new(
                    5,
                    grant.username.clone(),
                    grant.space_name.clone(),
                    grant.role,
                );
                PlanNodeEnum::UserManage(UserManageNode::GrantRole(node))
            }
            BoundStatement::Revoke(revoke) => {
                let node = crate::planning::plan::core::nodes::RevokeRoleNode::new(
                    6,
                    revoke.username.clone(),
                    revoke.space_name.clone(),
                    revoke.role,
                );
                PlanNodeEnum::UserManage(UserManageNode::RevokeRole(node))
            }
            BoundStatement::DescribeUser(describe) => {
                let node = crate::planning::plan::core::nodes::DescribeUserNode::new(
                    7,
                    describe.username.clone(),
                );
                PlanNodeEnum::UserManage(UserManageNode::DescribeUser(node))
            }
            BoundStatement::ShowUsers(_) => {
                let node = crate::planning::plan::core::nodes::ShowUsersNode::new(8);
                PlanNodeEnum::UserManage(UserManageNode::ShowUsers(node))
            }
            BoundStatement::ShowRoles(show_roles) => {
                let space_name = show_roles.space_name.clone().unwrap_or_default();
                let node = crate::planning::plan::core::nodes::ShowRolesNode::new(9, space_name);
                PlanNodeEnum::UserManage(UserManageNode::ShowRoles(node))
            }
            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "UserManagementPlanner has no plan for bound statement: {}",
                    bound.kind()
                )));
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
