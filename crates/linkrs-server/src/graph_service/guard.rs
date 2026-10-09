use linkrs_core::types::SpaceSummary;
use linkrs_core::Permission;

use super::context::QueryExecutionContext;
use super::error::GraphServiceError;
use super::GraphService;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    /// Whether the statement is a server-resolved configuration statement.
    ///
    /// Shared by permission classification and the streaming entry guard so
    /// both agree on the set: configuration intents resolve on the
    /// materialized path only.
    pub(crate) fn is_config_statement(stmt: &str) -> bool {
        let upper = stmt.trim().to_uppercase();
        upper.starts_with("UPDATE CONFIGS") || upper.starts_with("SHOW CONFIGS")
    }

    pub(crate) fn extract_permission_from_statement(&self, stmt: &str) -> Permission {
        let stmt_upper = stmt.trim().to_uppercase();

        // Configuration statements touch global server state outside any
        // space: only administrators may issue them. This matches the
        // statement category mapping where both forms require admin rights.
        if Self::is_config_statement(stmt) {
            Permission::Admin
        } else if stmt_upper.starts_with("SELECT") || stmt_upper.starts_with("MATCH") {
            Permission::Read
        } else if stmt_upper.starts_with("INSERT") || stmt_upper.starts_with("CREATE") {
            Permission::Write
        } else if stmt_upper.starts_with("DELETE") || stmt_upper.starts_with("DROP") {
            Permission::Delete
        } else if stmt_upper.starts_with("ALTER") || stmt_upper.starts_with("ADD") {
            Permission::Schema
        } else {
            Permission::Read
        }
    }

    /// Shared permission gate for regular statements and transaction commands.
    /// Session-only statements (`USE`, `LET`) skip the space permission check.
    /// User management statements take a dedicated caller-auth path instead of
    /// the coarse read/write/delete mapping.
    pub(crate) fn check_query_permission(
        &self,
        username: &str,
        context: &QueryExecutionContext<'_>,
    ) -> Result<(), GraphServiceError> {
        if self.is_user_locked(username) {
            return Err(GraphServiceError::new(
                "Permission check failed: account is locked".to_string(),
            ));
        }
        if self.must_change_password(username) && !Self::is_change_password_intent(context.stmt) {
            return Err(GraphServiceError::new(
                "Permission check failed: password change required before other operations"
                    .to_string(),
            ));
        }
        if let Err(reason) = self.enforce_new_password_policy(username, context.stmt) {
            return Err(reason);
        }
        if let Some(result) = self.check_user_management_permission(username, context) {
            return result;
        }
        if self.permission_manager.is_admin(username) {
            return Ok(());
        }
        let stmt_upper = context.stmt.trim().to_uppercase();
        let session_only = stmt_upper.starts_with("USE ") || stmt_upper.starts_with("LET ");
        if session_only {
            return Ok(());
        }
        let permission = self.extract_permission_from_statement(context.stmt);
        self.permission_manager
            .check_permission(username, context.space_id, permission)
            .map_err(|e| GraphServiceError::new(format!("Permission check failed: {}", e)))
    }

    fn is_change_password_intent(stmt: &str) -> bool {
        if let Some(parsed) = Self::parse_stmt(stmt) {
            return matches!(parsed, crate::query::parser::ast::Stmt::ChangePassword(_));
        }
        stmt.trim().to_uppercase().starts_with("CHANGE PASSWORD")
    }

    pub(crate) fn parse_stmt(stmt: &str) -> Option<crate::query::parser::ast::Stmt> {
        let mut parser = crate::query::parser::Parser::new(stmt);
        match parser.parse() {
            Ok(result) if !parser.has_errors() => Some(result.ast.stmt().clone()),
            _ => None,
        }
    }

    fn check_user_management_permission(
        &self,
        username: &str,
        context: &QueryExecutionContext<'_>,
    ) -> Option<Result<(), GraphServiceError>> {
        let upper = context.stmt.trim().to_uppercase();
        let maybe_user_mgmt = upper.starts_with("CREATE USER")
            || upper.starts_with("DROP USER")
            || upper.starts_with("ALTER USER")
            || upper.starts_with("GRANT ")
            || upper.starts_with("REVOKE ")
            || upper.starts_with("CHANGE PASSWORD")
            || upper.starts_with("SHOW USERS")
            || upper.starts_with("SHOW ROLES")
            || upper.starts_with("DESCRIBE USER");
        if !maybe_user_mgmt {
            return None;
        }
        let Some(stmt) = Self::parse_stmt(context.stmt) else {
            return Some(Err(GraphServiceError::new(
                "Permission check failed: unable to classify user management statement".to_string(),
            )));
        };
        Some(self.enforce_user_management(username, &stmt, context.space_id))
    }

    fn enforce_user_management(
        &self,
        username: &str,
        stmt: &crate::query::parser::ast::Stmt,
        space_id: i64,
    ) -> Result<(), GraphServiceError> {
        use crate::query::parser::ast::Stmt as S;
        let denied =
            |msg: String| GraphServiceError::new(format!("Permission check failed: {}", msg));
        match stmt {
            S::CreateUser(_) | S::AlterUser(_) | S::DropUser(_) => {
                if self.permission_manager.is_admin(username) {
                    Ok(())
                } else {
                    Err(denied("only Admin or God can manage users".to_string()))
                }
            }
            S::Grant(grant) => self.enforce_role_change(
                username,
                &grant.username,
                &grant.space_name,
                grant.role.as_str(),
            ),
            S::Revoke(revoke) => self.enforce_role_change(
                username,
                &revoke.username,
                &revoke.space_name,
                revoke.role.as_str(),
            ),
            S::ChangePassword(change) => {
                let target = change.username.as_deref().unwrap_or(username);
                if target == username || self.permission_manager.is_god(username) {
                    Ok(())
                } else {
                    Err(denied("can only change own password".to_string()))
                }
            }
            S::ShowUsers(_) | S::ShowRoles(_) | S::DescribeUser(_) => self
                .permission_manager
                .check_permission(username, space_id, Permission::Read)
                .map_err(|e| GraphServiceError::new(format!("Permission check failed: {}", e))),
            _ => {
                unreachable!("user management pre-filter must only match DCL statements")
            }
        }
    }

    fn enforce_role_change(
        &self,
        caller: &str,
        target_user: &str,
        space_name: &str,
        role_name: &str,
    ) -> Result<(), GraphServiceError> {
        let denied =
            |msg: String| GraphServiceError::new(format!("Permission check failed: {}", msg));
        if caller == target_user {
            return Err(denied("cannot modify own role".to_string()));
        }
        let target_role: linkrs_core::RoleType =
            role_name.parse().map_err(|e: String| denied(e))?;
        let space_id = self
            .storage
            .get_space_id(space_name)
            .map(|id| id as i64)
            .map_err(|e| denied(format!("space '{}' not found: {}", space_name, e)))?;
        let operator_role = self
            .permission_manager
            .get_role(caller, space_id)
            .or_else(|| {
                self.permission_manager
                    .get_role(caller, crate::permission::GOD_SPACE_ID)
            });
        let Some(op_role) = operator_role else {
            return Err(denied("only Admin or God can manage roles".to_string()));
        };
        if !matches!(
            op_role,
            linkrs_core::RoleType::God | linkrs_core::RoleType::Admin | linkrs_core::RoleType::Dba
        ) {
            return Err(denied("only Admin or God can manage roles".to_string()));
        }
        if !op_role.can_grant(target_role) {
            return Err(denied(format!("cannot grant role {}", target_role)));
        }
        Ok(())
    }

    /// Extract the SpaceSummary from a USE-statement result.
    ///
    /// The engine executes USE as a DataSet with `space_name`/`space_id`/
    /// `vid_type` columns; `QueryResult::space_summary` recognizes both
    /// representations.
    pub(crate) fn extract_space_summary_from_result(
        result: &linkrs_api::api_core::QueryResult,
    ) -> Option<SpaceSummary> {
        result.space_summary()
    }
}
