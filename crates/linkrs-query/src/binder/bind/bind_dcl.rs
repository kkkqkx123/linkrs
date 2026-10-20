//! Binding for access-control statements.
//!
//! Covers the user lifecycle (`CREATE USER` / `ALTER USER` / `DROP USER`),
//! credential rotation (`CHANGE PASSWORD`), role assignment (`GRANT` /
//! `REVOKE`) and the role/user introspection statements (`SHOW USERS`,
//! `SHOW ROLES`, `DESCRIBE USER`).

use linkrs_core::DBResult;

use crate::binder::bound::*;

use super::Binder;

impl Binder {
    pub(crate) fn bind_create_user(
        &mut self,
        stmt: &crate::parser::ast::CreateUserStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::CreateUser(BoundCreateUser {
            username: stmt.username.clone(),
            password: stmt.password.clone(),
            if_not_exists: stmt.if_not_exists,
        }))
    }

    pub(crate) fn bind_drop_user(
        &mut self,
        stmt: &crate::parser::ast::DropUserStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::DropUser(BoundDropUser {
            username: stmt.username.clone(),
            if_exists: stmt.if_exists,
        }))
    }

    pub(crate) fn bind_alter_user(
        &mut self,
        stmt: &crate::parser::ast::AlterUserStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::AlterUser(BoundAlterUser {
            username: stmt.username.clone(),
            password: stmt.password.clone(),
            is_locked: stmt.is_locked,
        }))
    }

    pub(crate) fn bind_change_password(
        &mut self,
        stmt: &crate::parser::ast::ChangePasswordStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ChangePassword(BoundChangePassword {
            username: stmt.username.clone(),
            old_password: stmt.old_password.clone(),
            new_password: stmt.new_password.clone(),
        }))
    }

    pub(crate) fn bind_grant(
        &mut self,
        stmt: &crate::parser::ast::GrantStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Grant(BoundGrant {
            username: stmt.username.clone(),
            space_name: stmt.space_name.clone(),
            role: stmt.role,
        }))
    }

    pub(crate) fn bind_revoke(
        &mut self,
        stmt: &crate::parser::ast::RevokeStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Revoke(BoundRevoke {
            username: stmt.username.clone(),
            space_name: stmt.space_name.clone(),
            role: stmt.role,
        }))
    }

    pub(crate) fn bind_describe_user(
        &mut self,
        stmt: &crate::parser::ast::DescribeUserStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::DescribeUser(BoundDescribeUser {
            username: stmt.username.clone(),
        }))
    }

    pub(crate) fn bind_show_users(
        &mut self,
        _stmt: &crate::parser::ast::ShowUsersStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ShowUsers(BoundShowUsers))
    }

    pub(crate) fn bind_show_roles(
        &mut self,
        stmt: &crate::parser::ast::ShowRolesStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ShowRoles(BoundShowRoles {
            space_name: stmt.space_name.clone(),
        }))
    }
}
