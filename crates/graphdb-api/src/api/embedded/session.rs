//! Session Management Module
//!
//! Provide the concept of a "session" as the context in which queries are executed.

use crate::api::core::{CoreError, CoreResult, QueryApi, QueryRequest, SchemaApi};
use crate::api::embedded::batch::BatchInserter;
use crate::api::embedded::result::QueryResult;
use crate::api::embedded::transaction::{Transaction, TransactionConfig};
use crate::core::Value;
use crate::core::{SessionStatistics, StatsManager};
use crate::query::executor::expression::functions::{CustomFunction, FunctionRegistry};
use crate::query::parser::ast::Stmt;
use crate::query::parser::{Parser, ParserResult};
#[cfg(feature = "fulltext-search")]
use crate::search::FulltextIndexManager;
use crate::storage::StorageClient;
#[cfg(feature = "vector")]
use crate::sync::vector_sync::SearchOptions;
use crate::sync::SyncManager;
use crate::transaction::TransactionId;
use crate::transaction::TransactionManager;
use crate::transaction::TransactionOptions;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Session – Execution Context
///
/// A session is the basic unit for the execution of queries, and it maintains contextual information such as the current graph space and the transaction status.
///
/// # Examples
///
/// ```rust
/// use graphdb_api::api::embedded::{GraphDatabase, DatabaseConfig};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = GraphDatabase::open("my_db")?;
/// let mut session = db.session()?;
///
// Switch to the image space
/// session.use_space("test_space")?;
///
// Execute the query
/// let result = session.execute("MATCH (n) RETURN n")?;
///
// Using a transaction
/// let txn = session.begin_transaction()?;
/// txn.execute("CREATE TAG user(name string)")?;
/// txn.commit()?;
/// # Ok(())
/// # }
/// ```
pub struct Session<S: StorageClient + Clone + 'static> {
    db: Arc<GraphDatabaseInner<S>>,
    space_id: Arc<RwLock<Option<u64>>>,
    space_name: Arc<RwLock<Option<String>>>,
    auto_commit: bool,
    /// Session-level transaction binding started through a text
    /// `BEGIN` statement (the explicit `Transaction` handle API is
    /// independent of this).
    current_transaction: Arc<RwLock<Option<TransactionId>>>,
    /// Session-level change statistics
    statistics: SessionStatistics,
    /// Session-level function registry
    function_registry: Arc<RwLock<FunctionRegistry>>,
    /// Session-scoped user variables (`$name`) with transaction overlay.
    session_variables: Arc<crate::api::session_variables::SessionVariables>,
}

/// Internal structure of the database, used for sharing data between Session and GraphDatabase
#[repr(C)]
pub(crate) struct GraphDatabaseInner<S: StorageClient + Clone + 'static> {
    pub(crate) query_api: Arc<RwLock<QueryApi<S>>>,
    pub(crate) schema_api: SchemaApi<S>,
    pub(crate) txn_manager: Arc<TransactionManager>,
    pub(crate) storage: Arc<RwLock<S>>,
    #[cfg(feature = "fulltext-search")]
    pub(crate) fulltext_manager: Option<Arc<FulltextIndexManager>>,
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) stats_manager: Arc<StatsManager>,
    /// Tokio runtime for vector operations in embedded mode.
    /// Stored here to ensure the runtime lives as long as the database.
    #[cfg(feature = "vector")]
    pub(crate) vector_runtime: Arc<tokio::runtime::Runtime>,
}

impl<S: StorageClient + Clone + 'static + graphdb_storage::storage::UndoTarget> Session<S> {
    /// Create a new session.
    pub(crate) fn new(db: Arc<GraphDatabaseInner<S>>) -> Self {
        Self {
            db,
            space_id: Arc::new(RwLock::new(None)),
            space_name: Arc::new(RwLock::new(None)),
            auto_commit: true,
            current_transaction: Arc::new(RwLock::new(None)),
            statistics: SessionStatistics::new(),
            function_registry: Arc::new(RwLock::new(FunctionRegistry::new())),
            session_variables: Arc::new(crate::api::session_variables::SessionVariables::new()),
        }
    }

    /// Register a custom function
    pub fn register_custom_function(&self, function: CustomFunction) -> CoreResult<()> {
        let mut registry = self.function_registry.write();
        registry.register_custom_full(function);
        Ok(())
    }

    /// Obtain a reference to the function registry.
    pub fn function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        Arc::clone(&self.function_registry)
    }

    /// Get the number of rows affected by the last operation.
    pub fn changes(&self) -> u64 {
        self.statistics.last_changes()
    }

    /// Obtain the total number of session changes
    pub fn total_changes(&self) -> u64 {
        self.statistics.total_changes()
    }

    /// Obtain the ID of the last vertex that was inserted.
    pub fn last_insert_vertex_id(&self) -> Option<u64> {
        self.statistics.last_insert_vertex_id()
    }

    /// Obtain the ID of the last inserted edge.
    pub fn last_insert_edge_id(&self) -> Option<u64> {
        self.statistics.last_insert_edge_id()
    }

    /// Obtain statistical information references
    pub fn statistics(&self) -> &SessionStatistics {
        &self.statistics
    }

    /// Switch to the image space
    ///
    /// # Parameters
    /// `space_name` – Name of the graph space
    ///
    /// # Back
    /// - Returns on success ()
    /// - Return an error when something goes wrong (for example, if the required space does not exist).
    pub fn use_space(&mut self, space_name: &str) -> CoreResult<()> {
        let space_id = self.db.schema_api.use_space(space_name)?;
        *self.space_id.write() = Some(space_id);
        *self.space_name.write() = Some(space_name.to_string());
        Ok(())
    }

    /// Obtain the name of the current image space.
    pub fn current_space(&self) -> Option<String> {
        self.space_name.read().clone()
    }

    /// Obtain the current image space ID.
    pub fn current_space_id(&self) -> Option<u64> {
        *self.space_id.read()
    }

    /// After executing a query, check if the result represents a space switch
    /// (from USE <space>), and persist the new space context on this session.
    ///
    /// The core QueryApi converts SpaceSwitched to a QueryResult with
    /// "space_name", "space_id", "vid_type" columns. This method detects
    /// that pattern and updates the session's space state accordingly.
    fn update_space_from_result(&self, result: &crate::api::core::QueryResult) {
        let columns = result.columns();
        if !columns.iter().any(|c| c == "space_name") {
            return;
        }
        let row = match result.rows().first() {
            Some(r) => r,
            None => return,
        };
        let name = columns
            .iter()
            .position(|c| c == "space_name")
            .and_then(|idx| row.get(idx))
            .and_then(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            });
        let name = match name {
            Some(n) => n,
            None => return,
        };
        let id = columns
            .iter()
            .position(|c| c == "space_id")
            .and_then(|idx| row.get(idx))
            .and_then(|v| match v {
                Value::BigInt(i) => Some(*i as u64),
                _ => None,
            });
        let id = match id {
            Some(i) => i,
            None => return,
        };
        *self.space_id.write() = Some(id);
        *self.space_name.write() = Some(name);
    }

    /// Enable the automatic submission mode.
    ///
    /// When `auto_commit` is set to `true`, each query is automatically committed.
    /// When `auto_commit` is set to `false`, transactions must be explicitly used.
    pub fn set_auto_commit(&mut self, auto_commit: bool) {
        self.auto_commit = auto_commit;
    }

    /// Enable the automatic submission mode.
    pub fn auto_commit(&self) -> bool {
        self.auto_commit
    }

    // ── Session variables (`$name`) ─────────────────────────────────────

    /// Assign a session variable. Inside a text-begun transaction the
    /// assignment is recorded on the overlay so ROLLBACK / ROLLBACK TO
    /// SAVEPOINT restore the previous value.
    pub fn set_variable(&self, name: String, value: Value) {
        self.session_variables
            .set_variable(name, value, self.current_transaction.read().is_some());
    }

    /// Snapshot of all session variables (base + overlay) for injection as
    /// query inputs.
    pub fn variables_snapshot(&self) -> HashMap<String, Value> {
        self.session_variables.variables_snapshot()
    }

    /// Execute the query statement.
    ///
    /// # Parameters
    /// `query` – A string representing the query statement.
    ///
    /// # Back
    /// Return the query results when successful.
    /// - Return error on failure
    pub fn execute(&self, query: &str) -> CoreResult<QueryResult> {
        // Reset the previous change history
        self.statistics.reset_last();

        // Transaction / session commands are classified through the unified
        // parser entry: the six transaction commands perform the
        // TransactionManager side effect and execute the state-machine plan;
        // `LET` is not supported in embedded sessions (no session-variable
        // store).
        match Self::parse_command(query) {
            Err(parse_error) => {
                return Err(CoreError::InvalidParameter(parse_error));
            }
            Ok(Some(parsed)) => {
                let parsed_ast = parsed.ast;
                match parsed_ast.stmt() {
                    Stmt::AssignVariable(assign) => {
                        return self.execute_variable_assignment(
                            query,
                            parsed_ast.clone(),
                            assign,
                            None,
                            None,
                        );
                    }
                    stmt => {
                        return self.execute_transaction_command(query, stmt, parsed_ast.clone());
                    }
                }
            }
            Ok(None) => {}
        }

        // Statements inside a text-begun transaction run against the
        // transaction binding (mirroring the `Transaction` handle API).
        if let Some(txn_id) = *self.current_transaction.read() {
            let result = self.execute_in_transaction(query, txn_id, None);
            self.statistics.record_changes(
                result
                    .as_ref()
                    .map(|r| r.metadata().rows_returned as u64)
                    .unwrap_or(0),
            );
            return result;
        }

        let ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: self.auto_commit,
            transaction_id: None,
            parameters: None,
            session_variables: Some(self.variables_snapshot()),
            query_id: None,
            isolation_level: None,
            parsed_statement: None,
         consistency: Default::default(), minimum_lsn: None, };

        let mut query_api = self.db.query_api.write();
        let result = if self.auto_commit {
            let storage = self
                .db
                .storage
                .read()
                .bind_auto_commit_context()
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            query_api.execute_with_operation_storage(query, ctx, storage)?
        } else {
            query_api.execute(query, ctx)?
        };

        // Update statistical information
        self.statistics
            .record_changes(result.metadata.rows_returned);

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        Ok(QueryResult::from_core(result))
    }

    // ==================== Unified transaction / session commands ====================

    /// Unified classification entry (same policy as the server
    /// `GraphService::parse_command`): returns the parsed statement when it
    /// is one of the transaction / session commands, `Err` with the first
    /// specific parse error for malformed command-like statements.
    ///
    /// The parse is gated behind the zero-cost command-keyword text check:
    /// regular statements skip the API-layer parse entirely (single-parse
    /// pipeline — the query engine parses them once on the regular path).
    fn parse_command(query: &str) -> Result<Option<ParserResult>, String> {
        let upper = query.trim().to_uppercase();
        let command_like = upper == "BEGIN"
            || upper.starts_with("BEGIN ")
            || upper.starts_with("START TRANSACTION")
            || upper.starts_with("COMMIT")
            || upper.starts_with("ROLLBACK")
            || upper.starts_with("SAVEPOINT")
            || upper.starts_with("RELEASE SAVEPOINT")
            || upper == "LET"
            || upper.starts_with("LET ");
        if !command_like {
            return Ok(None);
        }
        let mut parser = Parser::new(query);
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
                    return Err(format!("Parse error: {}", first.message));
                }
                Ok(None)
            }
        }
    }

    /// Execute a transaction command: TransactionManager side effect +
    /// state-machine plan execution (the `TxnOperator` validates the
    /// session controller and produces the structured result).
    fn execute_transaction_command(
        &self,
        query: &str,
        stmt: &Stmt,
        parsed_ast: Arc<crate::query::parser::ast::stmt::Ast>,
    ) -> CoreResult<QueryResult> {
        let txn_manager = self.txn_manager();
        let require_transaction = |what: &str| {
            self.current_transaction_id().ok_or_else(|| {
                CoreError::TransactionFailed(format!("No active transaction to {}", what))
            })
        };
        match stmt {
            Stmt::BeginTransaction(begin_stmt) => {
                if self.current_transaction.read().is_some() {
                    return Err(CoreError::InvalidParameter(
                        "Session already has an active transaction".to_string(),
                    ));
                }
                let mut options = TransactionOptions::default();
                if let Some(read_only) = begin_stmt.read_only {
                    options.read_only = read_only;
                }
                let txn_id = txn_manager
                    .begin_transaction_with_owner(options, "embedded".to_string())
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                *self.current_transaction.write() = Some(txn_id);
                self.execute_command_plan(query, parsed_ast, Some(txn_id), true)
            }
            Stmt::CommitTransaction(_) => {
                let txn_id = require_transaction("commit")?;
                txn_manager
                    .commit_transaction(txn_id)
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                *self.current_transaction.write() = None;
                self.session_variables.commit_variables();
                self.execute_command_plan(query, parsed_ast, Some(txn_id), false)
            }
            Stmt::RollbackTransaction(rollback_stmt) => {
                if let Some(savepoint_name) = &rollback_stmt.savepoint_name {
                    let txn_id = require_transaction("rollback")?;
                    let savepoint_info = txn_manager
                        .get_context(txn_id)
                        .map_err(|e| CoreError::TransactionFailed(e.to_string()))?
                        .find_savepoint_by_name(savepoint_name)
                        .ok_or_else(|| {
                            CoreError::TransactionFailed(format!(
                                "Savepoint '{}' does not exist",
                                savepoint_name
                            ))
                        })?;
                    let storage = self.storage_mut();
                    txn_manager
                        .rollback_to_savepoint(txn_id, savepoint_info.id, &*storage)
                        .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                    self.session_variables.rollback_variables_to(savepoint_name);
                    self.execute_command_plan(query, parsed_ast, Some(txn_id), true)
                } else {
                    let txn_id = require_transaction("rollback")?;
                    // The embedded database has no commit sink, so the undo
                    // log must be applied explicitly (mirroring
                    // `rollback_to_savepoint`).
                    let mut storage = self.storage_mut();
                    txn_manager
                        .abort_transaction_with_undo(txn_id, &mut *storage)
                        .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                    *self.current_transaction.write() = None;
                    self.session_variables.rollback_variables();
                    self.execute_command_plan(query, parsed_ast, Some(txn_id), false)
                }
            }
            Stmt::Savepoint(savepoint_stmt) => {
                let txn_id = require_transaction("create savepoint")?;
                txn_manager
                    .create_savepoint(txn_id, Some(savepoint_stmt.name.clone()))
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                self.session_variables
                    .push_variable_savepoint(&savepoint_stmt.name);
                self.execute_command_plan(query, parsed_ast, Some(txn_id), true)
            }
            Stmt::ReleaseSavepoint(release_stmt) => {
                let txn_id = require_transaction("release savepoint")?;
                let context = txn_manager
                    .get_context(txn_id)
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                let savepoint_info = context
                    .find_savepoint_by_name(&release_stmt.name)
                    .ok_or_else(|| {
                        CoreError::TransactionFailed(format!(
                            "Savepoint '{}' does not exist",
                            release_stmt.name
                        ))
                    })?;
                txn_manager
                    .release_savepoint(txn_id, savepoint_info.id)
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                self.session_variables
                    .release_variable_savepoint(&release_stmt.name);
                self.execute_command_plan(query, parsed_ast, Some(txn_id), true)
            }
            _ => Err(CoreError::InvalidParameter(
                "Statement is not a transaction command".to_string(),
            )),
        }
    }

    /// Execute the transaction-command plan: the command runs in
    /// `TransactionScope::CommandScope` and the `TxnOperator` validates the
    /// session controller. An active transaction binds through
    /// `create_execution`; finished transactions (COMMIT / ROLLBACK) run
    /// without a storage binding.
    fn execute_command_plan(
        &self,
        query: &str,
        parsed_ast: Arc<crate::query::parser::ast::stmt::Ast>,
        transaction_id: Option<TransactionId>,
        active: bool,
    ) -> CoreResult<QueryResult> {
        let ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: self.auto_commit,
            transaction_id,
            parameters: None,
            session_variables: Some(self.variables_snapshot()),
            query_id: None,
            isolation_level: None,
            parsed_statement: Some(parsed_ast),
         consistency: Default::default(), minimum_lsn: None, };
        let mut query_api = self.db.query_api.write();
        if active {
            let txn_manager = self.txn_manager();
            let execution = txn_manager
                .create_execution(
                    transaction_id.ok_or_else(|| {
                        CoreError::TransactionFailed(
                            "No transaction id for command plan".to_string(),
                        )
                    })?,
                    false,
                )
                .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
            query_api
                .execute_with_execution(query, ctx, &execution)
                .map(QueryResult::from_core)
        } else {
            query_api.execute(query, ctx).map(QueryResult::from_core)
        }
    }

    /// Execute a statement inside a text-begun transaction.
    fn execute_in_transaction(
        &self,
        query: &str,
        txn_id: TransactionId,
        parameters: Option<HashMap<String, Value>>,
    ) -> CoreResult<QueryResult> {
        let txn_manager = self.txn_manager();
        let (ctx, statement_start) = txn_manager
            .begin_statement(txn_id)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        let execution = txn_manager
            .create_execution(txn_id, false)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;

        let query_ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: false,
            transaction_id: Some(txn_id),
            parameters,
            session_variables: Some(self.variables_snapshot()),
            query_id: None,
            isolation_level: None,
            parsed_statement: None,
         consistency: Default::default(), minimum_lsn: None, };

        let result = {
            let mut query_api = self.db.query_api.write();
            query_api.execute_with_execution(query, query_ctx, &execution)?
        };
        txn_manager
            .finish_statement(&ctx, statement_start)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        Ok(QueryResult::from_core(result))
    }

    /// Execute a `LET $name = expr` session-variable assignment.
    ///
    /// The right-hand side is evaluated through the query engine (the LET
    /// statement plans to a single-row, single-column expression evaluation);
    /// the value is stored on the session variable store. Inside a text-begun
    /// transaction the assignment is recorded on the overlay so ROLLBACK /
    /// ROLLBACK TO SAVEPOINT restore the previous value. Client-supplied
    /// parameters and session variables are passed through to the evaluation.
    fn execute_variable_assignment(
        &self,
        query: &str,
        parsed_ast: Arc<crate::query::parser::ast::stmt::Ast>,
        assign: &crate::query::parser::ast::stmt::AssignVariableStmt,
        parameters: Option<HashMap<String, Value>>,
        session_variables: Option<HashMap<String, Value>>,
    ) -> CoreResult<QueryResult> {
        // Client-supplied session variables override only the keys they name;
        // all other keys keep the session snapshot.
        let merged_variables = match session_variables {
            Some(client_variables) => {
                let mut merged = self.variables_snapshot();
                merged.extend(client_variables);
                Some(merged)
            }
            None => Some(self.variables_snapshot()),
        };

        let ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: self.auto_commit,
            transaction_id: *self.current_transaction.read(),
            parameters,
            session_variables: merged_variables,
            query_id: None,
            isolation_level: None,
            parsed_statement: Some(parsed_ast),
         consistency: Default::default(), minimum_lsn: None, };

        let mut query_api = self.db.query_api.write();
        let result = if let Some(txn_id) = *self.current_transaction.read() {
            let txn_manager = self.txn_manager();
            let (statement_ctx, statement_start) = txn_manager
                .begin_statement(txn_id)
                .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
            let execution = txn_manager
                .create_execution(txn_id, false)
                .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
            let result = query_api.execute_with_execution(query, ctx, &execution)?;
            txn_manager
                .finish_statement(&statement_ctx, statement_start)
                .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
            result
        } else if self.auto_commit {
            let storage = self
                .db
                .storage
                .read()
                .bind_auto_commit_context()
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            query_api.execute_with_operation_storage(query, ctx, storage)?
        } else {
            query_api.execute(query, ctx)?
        };

        self.statistics
            .record_changes(result.metadata.rows_returned);

        // Contract: the LET plan evaluates to exactly one row with one value
        // column. Guard instead of silently taking the first value if a
        // planner regression changes the shape.
        if result.rows().len() != 1 {
            return Err(CoreError::InvalidParameter(format!(
                "LET expression must evaluate to a single row, got {} rows",
                result.rows().len()
            )));
        }
        if result.columns().len() != 1 {
            return Err(CoreError::InvalidParameter(format!(
                "LET expression must evaluate to a single value, got {} columns",
                result.columns().len()
            )));
        }
        let columns = result.columns().to_vec();
        let rows = result.rows().to_vec();
        let row = rows.first().ok_or_else(|| {
            CoreError::InvalidParameter("LET expression returned no value".to_string())
        })?;
        let value = row.first().cloned().ok_or_else(|| {
            CoreError::InvalidParameter("LET expression returned no value".to_string())
        })?;
        self.set_variable(assign.name.clone(), value);
        Ok(QueryResult::from_core(crate::api::core::QueryResult::new(
            crate::query::executor::base::ExecutionResult::from_data_set(
                crate::core::types::DataSet::from_rows(rows, columns),
            ),
            result.metadata,
        )))
    }

    /// Execute a parameterized query
    ///
    /// # Parameters
    /// - `query` - query statement string
    /// - `params` – Query parameters
    ///
    /// # Return
    /// - Returns query results on success
    /// - Return error on failure
    pub fn execute_with_params(
        &self,
        query: &str,
        params: HashMap<String, Value>,
    ) -> CoreResult<QueryResult> {
        // Transaction / session commands do not consume query parameters;
        // classify and route them through the unified command path.
        match Self::parse_command(query) {
            Err(parse_error) => return Err(CoreError::InvalidParameter(parse_error)),
            Ok(Some(parsed)) => {
                let parsed_ast = parsed.ast;
                match parsed_ast.stmt() {
                    Stmt::AssignVariable(assign) => {
                        return self.execute_variable_assignment(
                            query,
                            parsed_ast.clone(),
                            assign,
                            Some(params),
                            None,
                        );
                    }
                    stmt => {
                        return self.execute_transaction_command(query, stmt, parsed_ast.clone());
                    }
                }
            }
            Ok(None) => {}
        }

        // Statements inside a text-begun transaction run against the
        // transaction binding.
        if let Some(txn_id) = *self.current_transaction.read() {
            return self.execute_in_transaction(query, txn_id, Some(params));
        }

        let ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: self.auto_commit,
            transaction_id: None,
            parameters: Some(params),
            session_variables: Some(self.variables_snapshot()),
            query_id: None,
            isolation_level: None,
            parsed_statement: None,
         consistency: Default::default(), minimum_lsn: None, };

        let mut query_api = self.db.query_api.write();
        let result = if self.auto_commit {
            let storage = self
                .db
                .storage
                .read()
                .bind_auto_commit_context()
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            query_api.execute_with_operation_storage(query, ctx, storage)?
        } else {
            query_api.execute(query, ctx)?
        };

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        Ok(QueryResult::from_core(result))
    }

    /// Execute a query with both query parameters (`@name` references) and
    /// session variables (`$name` references).
    ///
    /// The two channels are fully independent: a parameter and a session
    /// variable with the same name coexist without conflict.
    pub fn execute_with_params_and_variables(
        &self,
        query: &str,
        params: HashMap<String, Value>,
        session_variables: HashMap<String, Value>,
    ) -> CoreResult<QueryResult> {
        // Transaction / session commands do not consume parameters or
        // session variables; classify and route them through the unified
        // command path.
        match Self::parse_command(query) {
            Err(parse_error) => return Err(CoreError::InvalidParameter(parse_error)),
            Ok(Some(parsed)) => {
                let parsed_ast = parsed.ast;
                match parsed_ast.stmt() {
                    Stmt::AssignVariable(assign) => {
                        return self.execute_variable_assignment(
                            query,
                            parsed_ast.clone(),
                            assign,
                            Some(params),
                            None,
                        );
                    }
                    stmt => {
                        return self.execute_transaction_command(query, stmt, parsed_ast.clone());
                    }
                }
            }
            Ok(None) => {}
        }

        // Statements inside a text-begun transaction run against the
        // transaction binding.
        if let Some(txn_id) = *self.current_transaction.read() {
            return self.execute_in_transaction(query, txn_id, Some(params));
        }

        let ctx = QueryRequest {
            space_id: *self.space_id.read(),
            space_name: self.space_name.read().clone(),
            auto_commit: self.auto_commit,
            transaction_id: None,
            parameters: Some(params),
            session_variables: Some(session_variables),
            query_id: None,
            isolation_level: None,
            parsed_statement: None,
         consistency: Default::default(), minimum_lsn: None, };

        let mut query_api = self.db.query_api.write();
        let result = if self.auto_commit {
            let storage = self
                .db
                .storage
                .read()
                .bind_auto_commit_context()
                .map_err(|error| CoreError::StorageError(error.to_string()))?;
            query_api.execute_with_operation_storage(query, ctx, storage)?
        } else {
            query_api.execute(query, ctx)?
        };

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        Ok(QueryResult::from_core(result))
    }

    /// Start a transaction
    ///
    /// # Return
    /// - Returns the transaction handle on success
    /// - Return error on failure
    pub fn begin_transaction(&self) -> CoreResult<Transaction<'_, S>> {
        let options = TransactionOptions::default();
        let txn_id = self
            .db
            .txn_manager
            .begin_transaction(options)
            .map_err(|e| crate::api::core::CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = crate::api::core::TransactionHandle(txn_id);

        Ok(Transaction::new(self, txn_handle))
    }

    /// Starting a Transaction with Configuration
    ///
    /// # Parameters
    /// - `config` - transaction configuration options
    ///
    /// # Return
    /// - Returns the transaction handle on success
    /// - Return error on failure
    ///
    /// # Examples
    ///
    /// ```rust
    /// use graphdb_api::api::embedded::{GraphDatabase, TransactionConfig};
    /// use std::time::Duration;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = GraphDatabase::open("my_db")?;
    /// let session = db.session()?;
    ///
    // Create read-only transactions
    /// let config = TransactionConfig::new()
    ///     .read_only()
    ///     .with_timeout(Duration::from_secs(60));
    ///
    /// let txn = session.begin_transaction_with_config(config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn begin_transaction_with_config(
        &self,
        config: TransactionConfig,
    ) -> CoreResult<Transaction<'_, S>> {
        let options = config.into_options();
        let txn_id = self
            .db
            .txn_manager
            .begin_transaction(options)
            .map_err(|e| crate::api::core::CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = crate::api::core::TransactionHandle(txn_id);

        Ok(Transaction::new(self, txn_handle))
    }

    /// Performing operations in a transaction (autocommit/rollback)
    ///
    /// # Parameters
    /// - `f` - closure executed in a transaction
    ///
    /// # Return
    /// - Returns the closure's return value on success
    /// - Return error on failure
    pub fn with_transaction<F, T>(&self, f: F) -> CoreResult<T>
    where
        F: FnOnce(&Transaction<'_, S>) -> CoreResult<T>,
    {
        let txn = self.begin_transaction()?;

        match f(&txn) {
            Ok(result) => {
                txn.commit()?;
                Ok(result)
            }
            Err(e) => {
                let _ = txn.rollback();
                Err(e)
            }
        }
    }

    /// Creating a graph space
    ///
    /// # Parameters
    /// - `name' - space name
    /// - `config' - space configuration
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn create_space(
        &self,
        name: &str,
        config: crate::api::core::SpaceConfig,
    ) -> CoreResult<()> {
        self.db.schema_api.create_space(name, config)
    }

    /// Deletion of map space
    ///
    /// # Parameters
    /// - `name' - space name
    ///
    /// # Return
    /// - Returns on success ()
    /// - Return error on failure
    pub fn drop_space(&self, name: &str) -> CoreResult<()> {
        self.db.schema_api.drop_space(name)
    }

    /// List all graph spaces
    pub fn list_spaces(&self) -> CoreResult<Vec<String>> {
        // Getting all the space through the storage layer
        let storage = self.db.storage.write();
        let spaces = storage
            .list_spaces()
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        Ok(spaces.into_iter().map(|s| s.space_name).collect())
    }

    /// Getting a mutable lock on the query API (internal use)
    pub(crate) fn query_api_mut(&self) -> parking_lot::RwLockWriteGuard<'_, QueryApi<S>> {
        self.db.query_api.as_ref().write()
    }

    /// Get space ID (internal use)
    pub(crate) fn space_id(&self) -> Option<u64> {
        *self.space_id.read()
    }

    /// Getting the transaction manager (internal use)
    pub(crate) fn txn_manager(&self) -> Arc<TransactionManager> {
        self.db.txn_manager.clone()
    }

    /// Acquiring stored write locks (for internal use)
    pub(crate) fn storage_mut(&self) -> parking_lot::RwLockWriteGuard<'_, S> {
        self.db.storage.write()
    }

    /// Get current space name (for internal use)
    pub(crate) fn space_name(&self) -> Option<String> {
        self.space_name.read().clone()
    }

    /// Get the text-begun transaction binding, if any (internal use).
    pub(crate) fn current_transaction_id(&self) -> Option<TransactionId> {
        *self.current_transaction.read()
    }

    /// Creating a Batch Inserter
    ///
    /// # Parameters
    /// - `batch_size` - batch size, automatically refreshes when this amount is reached
    ///
    /// # Return
    /// - Returns an instance of BatchInserter
    ///
    /// # Examples
    ///
    /// ```rust
    /// use graphdb_api::api::embedded::GraphDatabase;
    /// use graphdb_api::core::Vertex;
    /// use graphdb_api::core::types::VertexId;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = GraphDatabase::open("my_db")?;
    /// let session = db.session()?;
    ///
    // Create a batch inserter that automatically refreshes every 100 entries
    /// let mut inserter = session.batch_inserter(100);
    ///
    // Add vertices
    /// for i in 0..1000 {
    ///     let vertex = Vertex::with_vid(VertexId::from_int64(i));
    ///     inserter.add_vertex(vertex);
    /// }
    ///
    // Perform batch insertion
    /// let result = inserter.execute()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn batch_inserter(&self, batch_size: usize) -> BatchInserter<'_, S> {
        BatchInserter::new(self, batch_size)
    }

    /// Batch insert vertices
    ///
    /// # Parameters
    /// - `vertices` - list of vertices to insert
    ///
    /// # Return
    /// - Returns the number of vertices inserted on success
    /// - Return error on failure
    pub fn batch_insert_vertices(&self, vertices: Vec<crate::core::Vertex>) -> CoreResult<usize> {
        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let count = vertices.len();
        let mut storage = self.storage_mut();
        storage
            .batch_insert_vertices(&space_name, vertices)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        Ok(count)
    }

    /// Batch insert edges
    ///
    /// # Parameters
    /// - `edges` - list of edges to insert
    ///
    /// # Return
    /// - Returns the number of edges inserted on success
    /// - Return error on failure
    pub fn batch_insert_edges(&self, edges: Vec<crate::core::Edge>) -> CoreResult<usize> {
        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let count = edges.len();
        let mut storage = self.storage_mut();
        storage
            .batch_insert_edges(&space_name, edges)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        Ok(count)
    }

    /// Commit a transaction by handle (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub fn commit_transaction(
        &self,
        txn_handle: crate::api::core::TransactionHandle,
    ) -> CoreResult<()> {
        self.txn_manager()
            .commit_transaction(txn_handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Rollback a transaction by handle (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub fn rollback_transaction(
        &self,
        txn_handle: crate::api::core::TransactionHandle,
    ) -> CoreResult<()> {
        self.txn_manager()
            .abort_transaction(txn_handle.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Create a savepoint for a transaction (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    /// - `name` - savepoint name
    ///
    /// # Return
    /// - Returns savepoint ID on success
    /// - Return error on failure
    pub fn create_savepoint(
        &self,
        txn_handle: &crate::api::core::TransactionHandle,
        name: &str,
    ) -> CoreResult<crate::api::core::SavepointId> {
        self.txn_manager()
            .create_savepoint(txn_handle.0, Some(name.to_string()))
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
            .map(crate::api::core::SavepointId)
    }

    /// Release a savepoint (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    /// - `savepoint` - savepoint ID
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub fn release_savepoint(
        &self,
        txn_handle: &crate::api::core::TransactionHandle,
        savepoint: crate::api::core::SavepointId,
    ) -> CoreResult<()> {
        self.txn_manager()
            .release_savepoint(txn_handle.0, savepoint.0)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Rollback to a savepoint (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    /// - `savepoint` - savepoint ID
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub fn rollback_to_savepoint(
        &self,
        txn_handle: &crate::api::core::TransactionHandle,
        savepoint: crate::api::core::SavepointId,
    ) -> CoreResult<()> {
        let txn_manager = self.txn_manager();
        let storage = self.storage_mut();
        txn_manager
            .rollback_to_savepoint(txn_handle.0, savepoint.0, &*storage)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }

    /// Vector search - search for similar vectors
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `query_vector` - query vector
    /// - `limit` - maximum number of results to return
    ///
    /// # Return
    /// - Returns vector search results on success
    /// - Return error on failure
    #[cfg(feature = "vector")]
    pub async fn vector_search(
        &self,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> CoreResult<Vec<crate::api::core::VectorSearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        let options = SearchOptions::new(space_id, tag_name, field_name, query_vector, limit);
        let results = coordinator
            .search_with_options(options)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|r| crate::api::core::VectorSearchResult {
                id: r.id,
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect())
    }

    /// Vector search with threshold
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `query_vector` - query vector
    /// - `limit` - maximum number of results to return
    /// - `threshold` - minimum similarity threshold
    ///
    /// # Return
    /// - Returns vector search results on success
    /// - Return error on failure
    #[cfg(feature = "vector")]
    pub async fn vector_search_with_threshold(
        &self,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> CoreResult<Vec<crate::api::core::VectorSearchResult>> {
        let space_id = (*self.space_id.read())
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        let options = SearchOptions::new(space_id, tag_name, field_name, query_vector, limit)
            .with_threshold(threshold);
        let results = coordinator
            .search_with_options(options)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))?;

        Ok(results
            .into_iter()
            .map(|r| crate::api::core::VectorSearchResult {
                id: r.id,
                score: r.score,
                vector: r.vector.map(|v| v.to_vec()),
                payload: r.payload.map(|p| p.into_iter().collect()),
            })
            .collect())
    }

    /// Create a vector index
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    /// - `vector_size` - dimension of the vector
    /// - `distance` - distance metric
    ///
    /// # Return
    /// - Returns collection name on success
    /// - Return error on failure
    #[cfg(feature = "vector")]
    pub async fn create_vector_index(
        &self,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_search::DistanceMetric,
    ) -> CoreResult<String> {
        let space_id = {
            let guard = self.space_id.read();
            guard
                .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?
        };

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        coordinator
            .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }

    /// Drop a vector index
    ///
    /// # Parameters
    /// - `tag_name` - tag name
    /// - `field_name` - vector field name
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    #[cfg(feature = "vector")]
    pub async fn drop_vector_index(&self, tag_name: &str, field_name: &str) -> CoreResult<()> {
        let space_id = {
            let guard = self.space_id.read();
            guard
                .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?
        };

        let sync_manager =
            self.db.sync_manager.as_ref().ok_or_else(|| {
                CoreError::InvalidParameter("Sync manager not available".to_string())
            })?;

        let coordinator = sync_manager.vector_coordinator().ok_or_else(|| {
            CoreError::InvalidParameter("Vector coordinator not available".to_string())
        })?;

        coordinator
            .drop_vector_index(space_id, tag_name, field_name)
            .await
            .map_err(|e| CoreError::VectorError(e.to_string()))
    }
}

impl<S: StorageClient + Clone + 'static> Drop for Session<S> {
    fn drop(&mut self) {
        // No special cleanup is required when the session is discarded.
        // Because all transactions are managed through the Transaction object, and Transactions have their own Drop implementation
        // Just logging here for debugging purposes
        log::debug!(
            "Session released, current graph space: {:?}",
            self.space_name.read()
        );
    }
}

// In order to support Send + Sync, we need to ensure that S satisfies these constraints
// Safety Notes:
// 1. Session uses Arc<GraphDatabaseInner<S>> to share data internally, Arc itself is Send + Sync.
// 2. QueryApi in GraphDatabaseInner is Mutex-protected for thread-safety.
// 3. The StorageClient class must implement the Clone method and be marked as ‘static’. This is to ensure that objects can be safely passed between different threads.
// 4. All internal states (space_id, space_name, auto_commit) are of simple, replicable types.
// Therefore, the Session can securely implement both the Send and Sync functions.
unsafe impl<S: StorageClient + Clone + 'static> Send for Session<S> {}
unsafe impl<S: StorageClient + Clone + 'static> Sync for Session<S> {}
