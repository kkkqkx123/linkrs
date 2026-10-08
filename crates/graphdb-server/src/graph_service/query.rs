use std::collections::HashMap;
use std::sync::Arc;

use log::warn;

use graphdb_api::api_core::types::ConsistencyLevel;
use graphdb_api::api_core::QueryResult;
use graphdb_transaction::types::TransactionExecution;
use graphdb_transaction::TransactionId;

use super::context::QueryExecutionContext;
use super::error::GraphServiceError;
use super::GraphService;
use crate::session::ClientSession;
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
    pub async fn execute(
        &self,
        session_id: i64,
        stmt: &str,
    ) -> Result<QueryResult, GraphServiceError> {
        self.execute_inner(session_id, stmt, None, None, ConsistencyLevel::default())
            .await
    }

    /// Execute a query with client-supplied query parameters (`@name`
    /// references) and/or session variables (`$name` references).
    pub async fn execute_with_params(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, GraphServiceError> {
        self.execute_inner(
            session_id,
            stmt,
            parameters,
            session_variables,
            ConsistencyLevel::default(),
        )
        .await
    }

    /// Execute a query with explicit consistency requirement.
    pub async fn execute_with_consistency(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        self.execute_inner(session_id, stmt, parameters, session_variables, consistency)
            .await
    }

    /// Unified materialized execution path. All public entry points delegate
    /// here with either the default or an explicit consistency level, so
    /// command classification, USE handling, and auto-commit stay in one place.
    async fn execute_inner(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| GraphServiceError::new(format!("Invalid session ID: {}", session_id)))?;

        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);

        if let Some(ref txn_manager) = self.transaction_manager {
            txn_manager.cleanup_expired_transactions();
        }

        if let Some(parsed) = Self::parse_command(stmt)? {
            let parsed_ast = parsed.ast;
            let stmt_ast = parsed_ast.stmt();
            let context = QueryExecutionContext {
                stmt,
                parsed_ast: Some(parsed_ast.clone()),
                space_id,
                parameters,
                session_variables,
            };
            match stmt_ast {
                crate::query::parser::ast::Stmt::AssignVariable(assign) => {
                    return self.execute_variable_assignment(
                        &session,
                        parsed_ast.clone(),
                        assign,
                        &context,
                        consistency,
                    );
                }
                _ => {
                    return self.execute_transaction_command(
                        &session,
                        stmt_ast,
                        &context,
                        consistency,
                    );
                }
            }
        }

        let context = QueryExecutionContext {
            stmt,
            parsed_ast: None,
            space_id,
            parameters,
            session_variables,
        };
        let mut result =
            self.execute_query_with_permission(session_id, &context, consistency.clone());

        if stmt.trim().to_uppercase().starts_with("USE ") {
            if let Ok(ref exec_result) = result {
                if let Some(space_summary) = Self::extract_space_summary_from_result(exec_result) {
                    session.set_space(space_summary);
                }
            }
        }

        self.finish_auto_commit(&session, &mut result);
        if result.is_ok() && Self::is_user_management_statement(stmt) {
            self.reconcile_user_state(Some(session_id)).await;
            self.evict_stale_user_sessions(&session, session_id, stmt)
                .await;
        }
        result
    }

    fn is_user_management_statement(stmt: &str) -> bool {
        let upper = stmt.trim().to_uppercase();
        upper.starts_with("CREATE USER")
            || upper.starts_with("DROP USER")
            || upper.starts_with("ALTER USER")
            || upper.starts_with("CHANGE PASSWORD")
            || upper.starts_with("GRANT ")
            || upper.starts_with("REVOKE ")
    }

    /// Enforce the password policy on query-channel password writes.
    ///
    /// Single hook on the execution path covering user creation, admin
    /// alteration carrying a new password, and self-service password
    /// changes. History reuse is checked against stored hashes.
    pub(crate) fn enforce_new_password_policy(
        &self,
        caller: &str,
        stmt: &str,
    ) -> Result<(), GraphServiceError> {
        let Some(parsed) = Self::parse_stmt(stmt) else {
            return Ok(());
        };
        use crate::query::parser::ast::Stmt as Parsed;
        let invalid =
            |reason: String| GraphServiceError::new(format!("invalid password: {}", reason));
        match parsed {
            Parsed::CreateUser(create) => self
                .validate_new_password(&create.username, &create.password)
                .map_err(invalid),
            Parsed::AlterUser(alter) => match alter.password {
                Some(password) => self
                    .validate_new_password(&alter.username, &password)
                    .map_err(invalid),
                None => Ok(()),
            },
            Parsed::ChangePassword(change) => {
                let target = change.username.as_deref().unwrap_or(caller);
                self.validate_new_password(target, &change.new_password)
                    .map_err(invalid)
            }
            _ => Ok(()),
        }
    }

    /// Target account of a query-channel password write, if any.
    ///
    /// After a successful rotation or drop, every other session of the
    /// target account is invalidated, matching the admin channel.
    async fn evict_stale_user_sessions(
        &self,
        session: &Arc<ClientSession>,
        session_id: i64,
        stmt: &str,
    ) {
        let Some(parsed) = Self::parse_stmt(stmt) else {
            return;
        };
        use crate::query::parser::ast::Stmt as Parsed;
        let target = match parsed {
            Parsed::AlterUser(alter) if alter.password.is_some() => Some(alter.username),
            Parsed::ChangePassword(change) => {
                Some(change.username.unwrap_or_else(|| session.user()))
            }
            Parsed::DropUser(drop) => Some(drop.username),
            _ => None,
        };
        if let Some(target) = target {
            self.session_manager
                .remove_sessions_by_username(&target, Some(session_id))
                .await;
        }
    }

    /// Keep permission state and sessions consistent after query-channel
    /// user writes. Storage is the source of truth.
    async fn reconcile_user_state(&self, except_session_id: Option<i64>) {
        let stored_users = self.storage.list_users();
        let tracked: Vec<String> = self
            .permission_manager
            .list_all_users()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for name in tracked {
            if !stored_users.contains(&name) {
                self.permission_manager.remove_user(&name);
            }
        }
        for username in &stored_users {
            self.sync_role_from_storage(username);
            let locked = self
                .storage
                .get_user(username)
                .map(|user| user.is_locked)
                .unwrap_or(false);
            if locked {
                self.session_manager
                    .remove_sessions_by_username(username, except_session_id)
                    .await;
            }
        }
    }

    /// Commit the session-bound transaction when the session runs in
    /// auto-commit mode. A commit failure replaces the original result.
    pub(crate) fn finish_auto_commit(
        &self,
        session: &Arc<ClientSession>,
        result: &mut Result<QueryResult, GraphServiceError>,
    ) {
        if result.is_ok() && session.is_auto_commit() {
            if let Some(txn_id) = session.current_transaction() {
                if let Some(ref txn_manager) = self.transaction_manager {
                    match txn_manager.commit_transaction(txn_id) {
                        Ok(()) => {
                            session.unbind_transaction();
                        }
                        Err(e) => {
                            warn!("Auto-commit failed for transaction {}: {}", txn_id, e);
                            session.unbind_transaction();
                            *result =
                                Err(GraphServiceError::new(format!("Auto-commit failed: {}", e)));
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn execute_query_with_permission(
        &self,
        session_id: i64,
        context: &QueryExecutionContext<'_>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| GraphServiceError::new(format!("Invalid session ID: {}", session_id)))?;

        session.charge();
        let username = session.user();
        self.check_query_permission(&username, context)?;

        let mut statement_guard = None;
        let execution = if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                match txn_manager.begin_statement(txn_id) {
                    Ok((ctx, statement_start)) => {
                        statement_guard = Some((txn_manager.clone(), ctx.clone(), statement_start));
                        Some(
                            txn_manager
                                .create_execution(ctx.id, false)
                                .map_err(|error| GraphServiceError::new(error.to_string()))?,
                        )
                    }
                    Err(e) => {
                        if e.is_timeout() {
                            warn!(
                                "Transaction {} exceeded a timeout before statement execution",
                                txn_id
                            );
                        }
                        return Err(GraphServiceError::new(e.to_string()));
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        let mut result = self.run_query_plan(&session, context, None, execution, consistency);

        if let Some((txn_manager, context, statement_start)) = statement_guard {
            if let Err(error) = txn_manager.finish_statement(&context, statement_start) {
                result = Err(GraphServiceError::new(error.to_string()));
            }
        }

        if result.is_err() {
            self.cleanup_invalid_transaction(&session);
        }

        result
    }

    /// Drop a session transaction that can no longer execute after a failed query.
    pub(crate) fn cleanup_invalid_transaction(&self, session: &Arc<ClientSession>) {
        if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                if let Ok(ctx) = txn_manager.get_context(txn_id) {
                    if !ctx.state().can_execute() {
                        warn!(
                            "Transaction {} is in invalid state {} after failed query, cleaning up",
                            txn_id,
                            ctx.state()
                        );
                        if let Err(e) = txn_manager.abort_transaction(txn_id) {
                            warn!("Failed to rollback invalid transaction {}: {}", txn_id, e);
                        }
                        session.unbind_transaction();
                        session.set_auto_commit(true);
                        session.rollback_variables();
                    }
                }
            }
        }
    }

    /// Shared query execution segment for regular statements and transaction
    /// commands: builds the `QueryRequest` and invokes the query API with the
    /// resolved execution binding, converting the result.
    pub(crate) fn run_query_plan(
        &self,
        session: &Arc<ClientSession>,
        context: &QueryExecutionContext<'_>,
        transaction_id: Option<TransactionId>,
        execution: Option<TransactionExecution>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        let merged_session_variables = match context.session_variables {
            Some(ref client_variables) => {
                let mut merged = session.variables_snapshot();
                merged.extend(client_variables.clone());
                Some(merged)
            }
            None => Some(session.variables_snapshot()),
        };
        let query_request = graphdb_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: transaction_id.or_else(|| session.current_transaction()),
            parameters: context.parameters.clone(),
            session_variables: merged_session_variables,
            query_id: None,
            parsed_statement: context.parsed_ast.clone(),
            consistency,
        };

        let mut query_api = self.query_api.write();
        if let Some(execution) = execution.as_ref() {
            query_api
                .execute_with_execution(context.stmt, query_request, execution)
                .map_err(|e| GraphServiceError::from_core_error_with_query(e, context.stmt))
        } else {
            let execution_storage = self
                .storage
                .bind_auto_commit_context()
                .map_err(|error| GraphServiceError::new(error.to_string()))?;
            query_api
                .execute_with_operation_storage(context.stmt, query_request, execution_storage)
                .map_err(|e| GraphServiceError::from_core_error_with_query(e, context.stmt))
        }
    }
}
