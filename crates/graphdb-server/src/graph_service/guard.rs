use graphdb_core::types::SpaceSummary;
use graphdb_core::Permission;

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
    pub(crate) fn check_query_permission(
        &self,
        username: &str,
        context: &QueryExecutionContext<'_>,
    ) -> Result<(), GraphServiceError> {
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

    /// Extract the SpaceSummary from a USE-statement result.
    ///
    /// The engine executes USE as a DataSet with `space_name`/`space_id`/
    /// `vid_type` columns; `QueryResult::space_summary` recognizes both
    /// representations.
    pub(crate) fn extract_space_summary_from_result(
        result: &graphdb_api::api_core::QueryResult,
    ) -> Option<SpaceSummary> {
        result.space_summary()
    }
}
