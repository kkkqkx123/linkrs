//! Query execution entry points for an embedded session.
//!
//! `execute*` routes transaction / session commands, `CREATE ... AS`, and
//! text-begun transactions to their dedicated handlers in `transactions` /
//! `create_as`, then drives the shared `QueryApi` for ordinary statements.

use super::Session;
use crate::api_core::session_command::parse_session_command;
use crate::api_core::types::QueryResult as CoreQueryResult;
use crate::api_core::{CoreError, CoreResult};
use crate::embedded::result::QueryResult;
use linkrs_core::types::DataSet;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::ast::stmt::AssignVariableStmt;
use linkrs_query::parser::ast::stmt::Ast;
use linkrs_query::parser::ast::Stmt;
use linkrs_query::parser::ParserResult;
use linkrs_storage::StorageClient;
use linkrs_transaction::TransactionId;
use std::collections::HashMap;
use std::sync::Arc;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Execute the query statement.
    ///
    /// # Parameters
    /// `query` – A string representing the query statement.
    ///
    /// # Back
    /// Return the query results when successful.
    /// - Return error on failure
    pub fn execute(&self, query: &str) -> CoreResult<QueryResult> {
        self.ensure_not_interrupted()?;
        self.ensure_writable(query)?;
        // Reset the previous change history
        self.statistics.reset_last();

        // Transaction / session commands are classified through the unified
        // parser entry: the six transaction commands perform the
        // Transaction / session commands are classified through the shared
        // API-layer gate: the six transaction commands perform the
        // TransactionManager side effect and execute the state-machine plan;
        // `LET` assigns a session variable.
        match Self::parse_command(query) {
            Err(error) => return Err(error),
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
                    Stmt::Extension(extension) => {
                        return self.execute_extension(extension);
                    }
                    stmt => {
                        return self.execute_transaction_command(query, stmt, parsed_ast.clone());
                    }
                }
            }
            Ok(None) => {}
        }

        // `CREATE TAG/EDGE ... AS (query)` materializes the inner query
        // result as a new table through the session pipeline.
        if let Some(create_stmt) = Self::parse_create_as(query) {
            return self.execute_create_as(&create_stmt);
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

        let ctx = self.session_query_request().build();

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
        Self::reject_server_only_config_intent(&result)?;

        // Update statistical information
        self.statistics
            .record_changes(result.metadata.rows_returned);

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        self.notify_dml(query, result.metadata.rows_returned);

        Ok(QueryResult::from_core(result))
    }

    /// Unified classification entry shared with the network service layer:
    /// the keyword gate, parse, and AST match live in
    /// [`crate::api_core::session_command`], so both transports classify
    /// identically. Returns the parsed statement when it is one of the
    /// transaction / session commands.
    fn parse_command(query: &str) -> CoreResult<Option<ParserResult>> {
        parse_session_command(query).map_err(CoreError::from)
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
        self.ensure_not_interrupted()?;
        self.ensure_writable(query)?;
        // Transaction / session commands do not consume query parameters;
        // classify and route them through the unified command path.
        match Self::parse_command(query) {
            Err(error) => return Err(error),
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
                    Stmt::Extension(extension) => {
                        return self.execute_extension(extension);
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

        let ctx = self
            .session_query_request()
            .parameters(Some(params))
            .build();

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
        Self::reject_server_only_config_intent(&result)?;

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        self.notify_dml(query, result.metadata.rows_returned);

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
        self.ensure_not_interrupted()?;
        self.ensure_writable(query)?;
        // Transaction / session commands do not consume parameters or
        // session variables; classify and route them through the unified
        // command path.
        match Self::parse_command(query) {
            Err(error) => return Err(error),
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
                    Stmt::Extension(extension) => {
                        return self.execute_extension(extension);
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

        let ctx = self
            .session_query_request()
            .parameters(Some(params))
            .session_variables(Some(session_variables))
            .build();

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
        Self::reject_server_only_config_intent(&result)?;

        // Detect USE <space> results and persist space context
        self.update_space_from_result(&result);

        self.notify_dml(query, result.metadata.rows_returned);

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
    pub(crate) fn execute_variable_assignment(
        &self,
        query: &str,
        parsed_ast: Arc<Ast>,
        assign: &AssignVariableStmt,
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

        let ctx = self
            .session_query_request()
            .transaction_id(*self.current_transaction.read())
            .parameters(parameters)
            .session_variables(merged_variables)
            .parsed_statement(Some(parsed_ast))
            .build();

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
        Ok(QueryResult::from_core(CoreQueryResult::new(
            ExecutionResult::from_data_set(DataSet::from_rows(rows, columns)),
            result.metadata,
        )))
    }
}
