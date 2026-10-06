use std::sync::Arc;

use log::info;

use graphdb_api::api_core::types::ConsistencyLevel;
use graphdb_api::api_core::QueryResult;
use graphdb_transaction::TransactionId;

use super::context::QueryExecutionContext;
use super::error::{offset_to_position, GraphServiceError};
use super::GraphService;
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::Stmt;
use crate::query::parser::{Parser, ParserResult};
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
    /// Whether the statement text begins with a transaction / session
    /// command keyword (used to surface the first specific parse error for
    /// malformed commands instead of the generic recovery abort).
    pub(crate) fn is_command_like(stmt: &str) -> bool {
        let upper = stmt.trim().to_uppercase();
        upper == "BEGIN"
            || upper.starts_with("BEGIN ")
            || upper.starts_with("START TRANSACTION")
            || upper.starts_with("COMMIT")
            || upper.starts_with("ROLLBACK")
            || upper.starts_with("SAVEPOINT")
            || upper.starts_with("RELEASE SAVEPOINT")
            || upper == "LET"
            || upper.starts_with("LET ")
    }

    /// Whether the text holds more than one statement, reusing the parser's
    /// statement-boundary rule: one optional trailing semicolon is allowed,
    /// anything after that is a second statement.
    pub(crate) fn has_multiple_statements(text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let mut parser = Parser::new(text);
        let _ = parser.parse();
        parser
            .take_errors()
            .iter()
            .any(|error| error.message.contains("after end of statement"))
    }

    /// Unified classification entry: parse the statement and return it when
    /// it is one of the transaction / session commands.
    pub(crate) fn parse_command(stmt: &str) -> Result<Option<ParserResult>, GraphServiceError> {
        if !Self::is_command_like(stmt) {
            return Ok(None);
        }
        let mut parser = Parser::new(stmt);
        match parser.parse() {
            Ok(result) if !parser.has_errors() => {
                let stmt_ast = result.ast.stmt();
                match stmt_ast {
                    Stmt::BeginTransaction(_)
                    | Stmt::CommitTransaction(_)
                    | Stmt::RollbackTransaction(_)
                    | Stmt::Savepoint(_)
                    | Stmt::ReleaseSavepoint(_)
                    | Stmt::AssignVariable(_) => Ok(Some(result)),
                    _ => Ok(None),
                }
            }
            Ok(_) => Ok(None),
            Err(_) => {
                if let Some(first) = parser.errors().iter().next() {
                    let position = first.position.is_valid().then_some(first.position);
                    let position = position.or_else(|| {
                        first
                            .offset
                            .and_then(|offset| offset_to_position(stmt, offset))
                    });
                    return Err(GraphServiceError::with_position(
                        format!("Parse error: {}", first.message),
                        position,
                    ));
                }
                Ok(None)
            }
        }
    }

    /// Execute a transaction / session command through the unified
    /// AST plan operator state machine pipeline.
    pub(crate) fn execute_transaction_command(
        &self,
        session: &Arc<ClientSession>,
        stmt: &Stmt,
        context: &QueryExecutionContext<'_>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        self.validate_session_transaction_state(session)?;
        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        match stmt {
            Stmt::BeginTransaction(begin_stmt) => {
                if session.has_active_transaction() {
                    return Err(GraphServiceError::new(
                        "Session already has an active transaction",
                    ));
                }

                let mut options = session.transaction_options();
                if let Some(read_only) = begin_stmt.read_only {
                    options.read_only = read_only;
                }
                let txn_id = self.begin_owned_transaction(session, options)?;

                session.bind_transaction(txn_id);
                session.set_auto_commit(false);
                info!(
                    "Session {} started {} transaction {}",
                    session.id(),
                    if begin_stmt.read_only == Some(true) {
                        "read-only"
                    } else {
                        "read-write"
                    },
                    txn_id
                );
                let result = self.run_transaction_command_plan(
                    session.id(),
                    context,
                    Some(txn_id),
                    consistency,
                );
                if result.is_err() {
                    let _ = txn_manager.abort_transaction(txn_id);
                    session.unbind_transaction();
                    session.set_auto_commit(true);
                    session.rollback_variables();
                }
                result
            }

            Stmt::CommitTransaction(_) => {
                let txn_id = session
                    .current_transaction()
                    .ok_or("No active transaction to commit")?;

                txn_manager
                    .commit_transaction(txn_id)
                    .map_err(|e| format!("Failed to commit transaction: {}", e))?;

                session.unbind_transaction();
                session.set_auto_commit(true);
                session.commit_variables();
                info!("Session {} committed transaction {}", session.id(), txn_id);
                self.run_transaction_command_plan(session.id(), context, Some(txn_id), consistency)
            }

            Stmt::RollbackTransaction(rollback_stmt) => {
                if let Some(savepoint_name) = &rollback_stmt.savepoint_name {
                    let txn_id = session
                        .current_transaction()
                        .ok_or("No active transaction to rollback")?;
                    let savepoint_info = txn_manager
                        .get_context(txn_id)
                        .map_err(|e| format!("Failed to get transaction context: {}", e))?
                        .find_savepoint_by_name(savepoint_name)
                        .ok_or_else(|| format!("Savepoint '{}' does not exist", savepoint_name))?;

                    let storage = &*self.storage;
                    txn_manager
                        .rollback_to_savepoint(txn_id, savepoint_info.id, storage)
                        .map_err(|e| format!("Failed to rollback to savepoint: {}", e))?;
                    session.rollback_variables_to(savepoint_name);
                    info!(
                        "Session {} rolled back transaction {} to savepoint {}",
                        session.id(),
                        txn_id,
                        savepoint_name
                    );
                    self.run_transaction_command_plan(
                        session.id(),
                        context,
                        Some(txn_id),
                        consistency,
                    )
                } else {
                    let txn_id = session
                        .current_transaction()
                        .ok_or("No active transaction to rollback")?;
                    txn_manager
                        .abort_transaction(txn_id)
                        .map_err(|e| format!("Failed to rollback transaction: {}", e))?;
                    session.unbind_transaction();
                    session.set_auto_commit(true);
                    session.rollback_variables();
                    info!(
                        "Session {} rolled back transaction {}",
                        session.id(),
                        txn_id
                    );
                    self.run_transaction_command_plan(
                        session.id(),
                        context,
                        Some(txn_id),
                        consistency,
                    )
                }
            }

            Stmt::Savepoint(savepoint_stmt) => {
                let txn_id = session
                    .current_transaction()
                    .ok_or("No active transaction, cannot create savepoint")?;
                let staged_mark =
                    crate::storage::UndoTarget::staged_write_mark(self.storage.as_ref(), txn_id);
                let savepoint_id = txn_manager
                    .create_savepoint(txn_id, Some(savepoint_stmt.name.clone()), staged_mark)
                    .map_err(|e| format!("Failed to create savepoint: {}", e))?;
                info!(
                    "Session {} created savepoint {} in transaction {} (ID: {})",
                    session.id(),
                    savepoint_stmt.name,
                    txn_id,
                    savepoint_id
                );
                let result = self.run_transaction_command_plan(
                    session.id(),
                    context,
                    Some(txn_id),
                    consistency,
                );
                if result.is_ok() {
                    session.push_variable_savepoint(&savepoint_stmt.name);
                }
                result
            }

            Stmt::ReleaseSavepoint(release_stmt) => {
                let txn_id = session
                    .current_transaction()
                    .ok_or("No active transaction, cannot release savepoint")?;
                let txn_context = txn_manager
                    .get_context(txn_id)
                    .map_err(|e| format!("Failed to get transaction context: {}", e))?;
                let savepoint_info = txn_context
                    .find_savepoint_by_name(&release_stmt.name)
                    .ok_or_else(|| format!("Savepoint '{}' does not exist", release_stmt.name))?;
                txn_manager
                    .release_savepoint(txn_id, savepoint_info.id)
                    .map_err(|e| format!("Failed to release savepoint: {}", e))?;
                info!(
                    "Session {} released savepoint {} in transaction {}",
                    session.id(),
                    release_stmt.name,
                    txn_id
                );
                let result = self.run_transaction_command_plan(
                    session.id(),
                    context,
                    Some(txn_id),
                    consistency,
                );
                if result.is_ok() {
                    session.release_variable_savepoint(&release_stmt.name);
                }
                result
            }

            _ => Err(GraphServiceError::new(
                "Statement is not a transaction command",
            )),
        }
    }

    /// Start a session-owned transaction, retrying once after expiring stale
    /// transactions on write-conflict.
    fn begin_owned_transaction(
        &self,
        session: &Arc<ClientSession>,
        options: graphdb_transaction::TransactionOptions,
    ) -> Result<TransactionId, GraphServiceError> {
        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;
        match txn_manager.begin_transaction_with_owner(options.clone(), session.id().to_string()) {
            Ok(txn_id) => Ok(txn_id),
            Err(e) => {
                if matches!(
                    e.kind(),
                    graphdb_transaction::TransactionErrorKind::WriteTransactionConflict
                ) {
                    txn_manager.cleanup_expired_transactions();
                    match txn_manager
                        .begin_transaction_with_owner(options, session.id().to_string())
                    {
                        Ok(txn_id) => Ok(txn_id),
                        Err(retry_err) => Err(GraphServiceError::new(format!(
                            "Failed to start transaction: {}",
                            retry_err
                        ))),
                    }
                } else {
                    Err(GraphServiceError::new(format!(
                        "Failed to start transaction: {}",
                        e
                    )))
                }
            }
        }
    }

    /// Execute the transaction-command plan: permission check plus query API
    /// invocation with the explicit transaction id.
    pub(crate) fn run_transaction_command_plan(
        &self,
        session_id: i64,
        context: &QueryExecutionContext<'_>,
        transaction_id: Option<TransactionId>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| GraphServiceError::new(format!("Invalid session ID: {}", session_id)))?;

        session.charge();
        let username = session.user();
        self.check_query_permission(&username, context)?;

        let execution = transaction_id.and_then(|id| {
            let manager = self.transaction_manager.as_ref()?;
            if manager.is_transaction_active(id) {
                manager.create_execution(id, false).ok()
            } else {
                None
            }
        });

        self.run_query_plan(&session, context, transaction_id, execution, consistency)
    }

    /// Execute a `LET $name = expr` session-variable assignment.
    pub(crate) fn execute_variable_assignment(
        &self,
        session: &Arc<ClientSession>,
        parsed_ast: Arc<Ast>,
        assign: &crate::query::parser::ast::stmt::AssignVariableStmt,
        context: &QueryExecutionContext<'_>,
        consistency: ConsistencyLevel,
    ) -> Result<QueryResult, GraphServiceError> {
        let result = self.execute_query_with_permission(
            session.id(),
            &QueryExecutionContext {
                stmt: context.stmt,
                parsed_ast: Some(parsed_ast),
                space_id: context.space_id,
                parameters: context.parameters.clone(),
                session_variables: context.session_variables.clone(),
            },
            consistency,
        )?;
        let value = Self::extract_let_value(&result)?;
        session.set_variable(assign.name.clone(), value);
        info!("Session {} set session variable", session.id());
        Ok(graphdb_api::api_core::QueryResult::empty())
    }

    /// The LET plan evaluates to exactly one row with one value column.
    fn extract_let_value(result: &QueryResult) -> Result<graphdb_core::Value, GraphServiceError> {
        if result.columns().len() != 1 {
            return Err(GraphServiceError::new(format!(
                "LET expression must evaluate to a single value, got {} columns",
                result.columns().len()
            )));
        }
        if result.rows().len() != 1 {
            return Err(GraphServiceError::new(format!(
                "LET expression must evaluate to a single row, got {} rows",
                result.rows().len()
            )));
        }
        result
            .first_value()
            .cloned()
            .ok_or_else(|| GraphServiceError::new("LET expression returned no value"))
    }

    /// Validate that session's transaction binding is consistent with transaction manager state.
    pub(crate) fn validate_session_transaction_state(
        &self,
        session: &Arc<ClientSession>,
    ) -> Result<(), GraphServiceError> {
        if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                if !txn_manager.is_transaction_active(txn_id) {
                    log::warn!(
                        "Session {} has stale transaction binding to {}, cleaning up",
                        session.id(),
                        txn_id
                    );
                    session.unbind_transaction();
                    return Err(GraphServiceError::new(format!(
                        "Transaction {} is no longer active, please retry the operation",
                        txn_id
                    )));
                }
            }
        }
        Ok(())
    }
}
