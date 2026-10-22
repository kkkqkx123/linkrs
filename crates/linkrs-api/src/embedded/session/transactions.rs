//! Explicit transaction handles and the transaction / session command
//! execution path.
//!
//! `begin_transaction*` / `with_transaction` expose the `Transaction` handle
//! API. `execute_transaction_command` / `execute_command_plan` /
//! `execute_in_transaction` are the internal routing called by `execution`
//! (kept `pub(crate)` so the sibling module can reach them).

use super::Session;
use crate::api_core::error::CoreError;
use crate::api_core::types::{SavepointId, TransactionHandle};
use crate::api_core::CoreResult;
use crate::embedded::result::QueryResult;
use crate::embedded::transaction::Transaction;
use crate::embedded::transaction::TransactionConfig;
use linkrs_core::Value;
use linkrs_query::parser::ast::stmt::Ast;
use linkrs_query::parser::ast::Stmt;
use linkrs_storage::StorageClient;
use linkrs_transaction::TransactionId;
use linkrs_transaction::TransactionOptions;
use std::collections::HashMap;
use std::sync::Arc;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Start a transaction
    ///
    /// # Return
    /// - Returns the transaction handle on success
    /// - Return error on failure
    pub fn begin_transaction(&self) -> CoreResult<Transaction<'_, S>> {
        if self.db.read_only {
            return Err(CoreError::StorageError(
                "database is read-only; use a read-only transaction".to_string(),
            ));
        }
        let options = TransactionOptions::default();
        let txn_id = self
            .db
            .txn_manager
            .begin_transaction(options)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = TransactionHandle(txn_id);

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
    /// use linkrs_api::embedded::{GraphDatabase, TransactionConfig};
    /// use std::time::Duration;
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = GraphDatabase::open("my_db")?;
    /// let session = db.session()?;
    ///
    /// // Create read-only transactions
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
        if self.db.read_only && !config.read_only {
            return Err(CoreError::StorageError(
                "database is read-only; use a read-only transaction".to_string(),
            ));
        }
        let options = config.into_options();
        let txn_id = self
            .db
            .txn_manager
            .begin_transaction(options)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = TransactionHandle(txn_id);

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

    /// Execute a transaction command: TransactionManager side effect +
    /// state-machine plan execution (the `TxnOperator` validates the
    /// session controller and produces the structured result).
    pub(crate) fn execute_transaction_command(
        &self,
        query: &str,
        stmt: &Stmt,
        parsed_ast: Arc<Ast>,
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
                if self.db.read_only && !options.read_only {
                    return Err(CoreError::StorageError(
                        "database is read-only; use a read-only transaction".to_string(),
                    ));
                }
                let txn_id = txn_manager
                    .begin_transaction_with_owner(options, "embedded".to_string())
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                *self.current_transaction.write() = Some(txn_id);
                let result = self.execute_command_plan(query, parsed_ast, Some(txn_id), true);
                if result.is_err() {
                    // A failed BEGIN must not leave a bound transaction
                    // behind: abort it and release the session binding.
                    let _ = txn_manager.abort_transaction(txn_id);
                    *self.current_transaction.write() = None;
                    self.session_variables.rollback_variables();
                }
                result
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
                    // Drop the storage write guard before running the command
                    // plan: plan execution re-locks storage internally, and
                    // holding the guard across it self-deadlocks.
                    {
                        let storage = self.storage_mut();
                        txn_manager
                            .rollback_to_savepoint(txn_id, savepoint_info.id, &*storage)
                            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                    }
                    self.session_variables.rollback_variables_to(savepoint_name);
                    self.execute_command_plan(query, parsed_ast, Some(txn_id), true)
                } else {
                    let txn_id = require_transaction("rollback")?;
                    // The embedded database has no commit sink, so the undo
                    // log must be applied explicitly (mirroring
                    // `rollback_to_savepoint`). The write guard is scoped to
                    // the abort: holding it across `execute_command_plan`
                    // below self-deadlocks (plan execution re-locks storage).
                    {
                        let mut storage = self.storage_mut();
                        txn_manager
                            .abort_transaction_with_undo(txn_id, &mut *storage)
                            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                    }
                    *self.current_transaction.write() = None;
                    self.session_variables.rollback_variables();
                    self.execute_command_plan(query, parsed_ast, Some(txn_id), false)
                }
            }
            Stmt::Savepoint(savepoint_stmt) => {
                let txn_id = require_transaction("create savepoint")?;
                // Capture the storage-staged write boundary before the
                // savepoint so ROLLBACK TO can rewind staged rows.
                let staged_mark = {
                    let storage = self.storage_mut();
                    linkrs_storage::UndoTarget::staged_write_mark(&*storage, txn_id)
                };
                txn_manager
                    .create_savepoint(txn_id, Some(savepoint_stmt.name.clone()), staged_mark)
                    .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
                // Record the variable-overlay boundary only once the
                // command plan succeeded, so a failed SAVEPOINT leaves no
                // half-installed marker behind.
                let result = self.execute_command_plan(query, parsed_ast, Some(txn_id), true);
                if result.is_ok() {
                    self.session_variables
                        .push_variable_savepoint(&savepoint_stmt.name);
                }
                result
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
                let result = self.execute_command_plan(query, parsed_ast, Some(txn_id), true);
                if result.is_ok() {
                    self.session_variables
                        .release_variable_savepoint(&release_stmt.name);
                }
                result
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
    pub(crate) fn execute_command_plan(
        &self,
        query: &str,
        parsed_ast: Arc<Ast>,
        transaction_id: Option<TransactionId>,
        active: bool,
    ) -> CoreResult<QueryResult> {
        let ctx = self
            .session_query_request()
            .transaction_id(transaction_id)
            .parsed_statement(Some(parsed_ast))
            .build();
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
    pub(crate) fn execute_in_transaction(
        &self,
        query: &str,
        txn_id: TransactionId,
        parameters: Option<HashMap<String, Value>>,
    ) -> CoreResult<QueryResult> {
        self.ensure_writable(query)?;
        let txn_manager = self.txn_manager();
        let (ctx, statement_start) = txn_manager
            .begin_statement(txn_id)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        let execution = txn_manager
            .create_execution(txn_id, false)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;

        let query_ctx = self
            .session_query_request()
            .auto_commit(false)
            .transaction_id(Some(txn_id))
            .parameters(parameters)
            .build();

        let result = {
            let mut query_api = self.db.query_api.write();
            query_api.execute_with_execution(query, query_ctx, &execution)?
        };
        Self::reject_server_only_config_intent(&result)?;
        txn_manager
            .finish_statement(&ctx, statement_start)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        self.notify_dml(query, result.metadata.rows_returned);
        Ok(QueryResult::from_core(result))
    }

    /// Commit a transaction by handle (for C API use)
    ///
    /// # Parameters
    /// - `txn_handle` - transaction handle
    ///
    /// # Return
    /// - Returns () on success
    /// - Return error on failure
    pub fn commit_transaction(&self, txn_handle: TransactionHandle) -> CoreResult<()> {
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
    pub fn rollback_transaction(&self, txn_handle: TransactionHandle) -> CoreResult<()> {
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
        txn_handle: &TransactionHandle,
        name: &str,
    ) -> CoreResult<SavepointId> {
        let staged_mark = {
            let storage = self.storage_mut();
            linkrs_storage::UndoTarget::staged_write_mark(&*storage, txn_handle.0)
        };
        self.txn_manager()
            .create_savepoint(txn_handle.0, Some(name.to_string()), staged_mark)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
            .map(SavepointId)
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
        txn_handle: &TransactionHandle,
        savepoint: SavepointId,
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
        txn_handle: &TransactionHandle,
        savepoint: SavepointId,
    ) -> CoreResult<()> {
        let txn_manager = self.txn_manager();
        let storage = self.storage_mut();
        txn_manager
            .rollback_to_savepoint(txn_handle.0, savepoint.0, &*storage)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
    }
}
