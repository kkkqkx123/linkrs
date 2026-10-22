//! Session state, accessors, and shared helpers.
//!
//! This module owns every method that is pure state access / bookkeeping plus
//! the small associated helpers (`single_message_result`, `session_query_request`,
//! `reject_server_only_config_intent`, `update_space_from_result`, `notify_dml*`)
//! that the execution / transaction / create-as paths call across module
//! boundaries. Cross-module helpers are `pub(crate)` so sibling submodules can
//! reach them; everything else keeps its original visibility.

use super::Session;
use crate::api_core::error::{CoreError, CoreResult};
use crate::api_core::query_api::QueryApi;
use crate::api_core::types::{
    ExecutionMetadata, QueryRequestBuilder, QueryResult as CoreQueryResult, SpaceConfig,
};
use crate::embedded::result::QueryResult;
use linkrs_core::types::DataSet;
use linkrs_core::Edge;
use linkrs_core::Value;
use linkrs_core::Vertex;
use linkrs_query::dml_op_from_stmt;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::DmlOp;
use linkrs_storage::StorageClient;
use parking_lot::RwLock;
use parking_lot::RwLockWriteGuard;
use std::collections::HashMap;
use std::sync::Arc;

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
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
    pub fn statistics(&self) -> &linkrs_core::SessionStatistics {
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

    /// Classify `query` and emit a statement-level DML notification when it
    /// is a write. No-op for read-only text, DDL, and unparseable text, and
    /// when nobody subscribes (`HookBus::subscribe_dml`). Returns the
    /// classified operation so callers sharing the classification (e.g. the
    /// C-API update hook) do not parse the text twice.
    ///
    /// The prepared-statement classifier (`dml_op_from_stmt` over the parsed
    /// `Stmt`) is the single source of truth; unparseable text stays silent
    /// because no operation can be determined for it.
    pub(crate) fn notify_dml(&self, query: &str, rows: u64) -> Option<DmlOp> {
        let op = self.classify_prepared(query)?;
        let space = self
            .current_space()
            .unwrap_or_else(|| "default".to_string());
        self.notify_dml_op(op, &space, rows);
        Some(op)
    }

    /// Classification from the parsed statement. Returns `Some(op)` for
    /// DML and `None` for read-only/DDL/diagnostics or unparseable text.
    fn classify_prepared(&self, query: &str) -> Option<DmlOp> {
        let mut parser = linkrs_query::parser::Parser::new(query);
        let parsed = parser.parse().ok()?;
        if parser.has_errors() {
            return None;
        }
        dml_op_from_stmt(parsed.ast.stmt())
    }

    /// Emit one statement-level DML notification for an operation the
    /// caller already classified (or that never had statement text).
    pub(crate) fn notify_dml_op(&self, op: DmlOp, space: &str, rows: u64) {
        self.db.hooks.emit_dml(op, space, rows);
    }

    /// Whether any statement-level DML observer is registered
    /// (consulted by the C-API update hook fast path).
    #[cfg(feature = "c_api")]
    pub(crate) fn has_dml_observers(&self) -> bool {
        self.db.hooks.has_dml_observers()
    }

    /// After executing a query, check if the result represents a space switch
    /// (from USE <space>), and persist the new space context on this session.
    ///
    /// The core QueryApi converts SpaceSwitched to a QueryResult with
    /// "space_name", "space_id", "vid_type" columns. This method detects
    /// that pattern and updates the session's space state accordingly.
    pub(crate) fn update_space_from_result(&self, result: &CoreQueryResult) {
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

    // ── Cooperative interrupt ─────────────────────────────────────────

    /// Request interruption of the next query on this session.
    ///
    /// Cooperative cancellation at the entry gate: `execute*` entry points
    /// fail fast while the flag is set. A query already running is not
    /// touched; stop those through the executor kill path and call
    /// `clear_interrupt` to resume normal execution.
    pub fn interrupt(&self) {
        self.interrupted
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether an interrupt has been requested and not yet cleared.
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Clear a previously requested interrupt.
    pub fn clear_interrupt(&self) {
        self.interrupted
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn ensure_not_interrupted(&self) -> CoreResult<()> {
        if self.is_interrupted() {
            return Err(CoreError::QueryExecutionFailed(
                "query interrupted".to_string(),
            ));
        }
        Ok(())
    }

    /// Reject mutating operations on read-only databases (flag-only check
    /// for schema/batch paths that carry no query text).
    pub(crate) fn ensure_writable_op(&self) -> CoreResult<()> {
        if self.db.read_only {
            return Err(CoreError::StorageError(
                "database is read-only; write operations are not allowed".to_string(),
            ));
        }
        Ok(())
    }

    /// Reject mutating statements on read-only databases.
    ///
    /// The prepared-statement classification (`StatementClass` over the
    /// parsed `Stmt`) is the single source of truth; unparseable text falls
    /// through to the mutating-opener list below and fails at parse time
    /// during execution when it is not a real write.
    pub(crate) fn ensure_writable(&self, query: &str) -> CoreResult<()> {
        if !self.db.read_only {
            return Ok(());
        }
        if let Some(stmt) = self.parse_stmt(query) {
            let class = linkrs_query::pipeline::prepared::classify_statement(&stmt);
            match class {
                linkrs_query::StatementClass::Dml
                | linkrs_query::StatementClass::Ddl
                | linkrs_query::StatementClass::Analyze => {
                    return self.ensure_writable_op();
                }
                linkrs_query::StatementClass::ReadOnly
                | linkrs_query::StatementClass::Transaction
                | linkrs_query::StatementClass::Diagnostic => {}
            }
            // Bulk / admin mutators that are not DML/DDL by class still
            // mutate storage; check them by kind.
            match &stmt {
                linkrs_query::parser::ast::Stmt::Copy(_)
                | linkrs_query::parser::ast::Stmt::ExportDatabase(_)
                | linkrs_query::parser::ast::Stmt::ImportDatabase(_)
                | linkrs_query::parser::ast::Stmt::AttachDatabase(_)
                | linkrs_query::parser::ast::Stmt::DetachDatabase(_)
                | linkrs_query::parser::ast::Stmt::LoadFrom(_)
                | linkrs_query::parser::ast::Stmt::Checkpoint(_)
                | linkrs_query::parser::ast::Stmt::PurgeExpired(_) => {
                    return self.ensure_writable_op();
                }
                _ => {}
            }
            return Ok(());
        }
        let keyword = leading_keyword(query);
        if keyword.eq_ignore_ascii_case("LOAD")
            || keyword.eq_ignore_ascii_case("IMPORT")
            || keyword.eq_ignore_ascii_case("EXPORT")
            || keyword.eq_ignore_ascii_case("ATTACH")
            || keyword.eq_ignore_ascii_case("COPY")
            || keyword.eq_ignore_ascii_case("INSTALL")
            || keyword.eq_ignore_ascii_case("UNINSTALL")
            || keyword.eq_ignore_ascii_case("UPDATE")
        {
            return self.ensure_writable_op();
        }
        Ok(())
    }

    fn parse_stmt(&self, query: &str) -> Option<linkrs_query::parser::ast::Stmt> {
        let mut parser = linkrs_query::parser::Parser::new(query);
        let parsed = parser.parse().ok()?;
        if parser.has_errors() {
            return None;
        }
        Some(parsed.ast.stmt().clone())
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

    /// Base request carrying the session-scoped context: current space,
    /// autocommit mode, and the session-variable snapshot. Callers layer
    /// per-statement fields on top.
    pub(crate) fn session_query_request(&self) -> QueryRequestBuilder {
        QueryRequestBuilder::new()
            .space_id(*self.space_id.read())
            .space_name(self.space_name.read().clone())
            .auto_commit(self.auto_commit)
            .session_variables(Some(self.variables_snapshot()))
    }

    /// Configuration statements resolve against the server-owned config
    /// store, which embedded sessions do not have: the pipeline emits an
    /// unresolved intent that would otherwise surface as empty rows.
    pub(crate) fn reject_server_only_config_intent(result: &CoreQueryResult) -> CoreResult<()> {
        use linkrs_query::executor::base::ExecutionResult as BaseExecutionResult;
        match &result.execution {
            BaseExecutionResult::ShowConfigs { .. }
            | BaseExecutionResult::ConfigUpdate { .. } => {
                Err(CoreError::InvalidParameter(
                    "SHOW CONFIGS and UPDATE CONFIGS require the server unary query interface and are not supported in embedded sessions"
                        .to_string(),
                ))
            }
            _ => Ok(()),
        }
    }

    /// Build a single-row, single-column result carrying an admin message.
    pub(crate) fn single_message_result(message: String) -> QueryResult {
        let columns = vec!["result".to_string()];
        let rows = vec![vec![Value::string(message)]];
        let execution = ExecutionResult::from_data_set(DataSet::from_rows(rows, columns));
        QueryResult::from_core(CoreQueryResult::new(
            execution,
            ExecutionMetadata {
                rows_returned: 1,
                ..Default::default()
            },
        ))
    }

    /// Get space ID (internal use)
    #[cfg(feature = "c_api")]
    pub(crate) fn space_id(&self) -> Option<u64> {
        *self.space_id.read()
    }

    /// Getting the transaction manager (internal use)
    pub(crate) fn txn_manager(&self) -> Arc<linkrs_transaction::TransactionManager> {
        self.db.txn_manager.clone()
    }

    /// Acquiring stored write locks (for internal use)
    pub(crate) fn storage_mut(&self) -> RwLockWriteGuard<'_, S> {
        self.db.storage.write()
    }

    /// Get current space name (for internal use)
    pub(crate) fn space_name(&self) -> Option<String> {
        self.space_name.read().clone()
    }

    /// Get the text-begun transaction binding, if any (internal use).
    pub(crate) fn current_transaction_id(&self) -> Option<linkrs_transaction::TransactionId> {
        *self.current_transaction.read()
    }

    /// Getting a mutable lock on the query API (internal use)
    pub(crate) fn query_api_mut(&self) -> RwLockWriteGuard<'_, QueryApi<S>> {
        self.db.query_api.as_ref().write()
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
    /// ```rust,no_run
    /// use linkrs_api::embedded::GraphDatabase;
    /// use linkrs_core::types::VertexId;
    /// use linkrs_core::{Tag, Vertex};
    ///
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = GraphDatabase::open("my_db")?;
    /// let session = db.session()?;
    ///
    /// // Create a batch inserter that automatically refreshes every 100 entries
    /// let mut inserter = session.batch_inserter(100);
    ///
    /// // Add vertices
    /// for i in 0..1000 {
    ///     let vertex = Vertex::new(VertexId::try_from_int64(i).expect("test vertex id"), Tag::new("test".into(), std::collections::HashMap::new()));
    ///     inserter.add_vertex(vertex);
    /// }
    ///
    /// // Perform batch insertion
    /// let result = inserter.execute()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn batch_inserter(
        &self,
        batch_size: usize,
    ) -> crate::embedded::batch::BatchInserter<'_, S> {
        crate::embedded::batch::BatchInserter::new(self, batch_size)
    }

    /// Batch insert vertices
    ///
    /// # Parameters
    /// - `vertices` - list of vertices to insert
    ///
    /// # Return
    /// - Returns the number of vertices inserted on success
    /// - Return error on failure
    pub fn batch_insert_vertices(&self, vertices: Vec<Vertex>) -> CoreResult<usize> {
        self.ensure_writable_op()?;
        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let count = vertices.len();
        let mut storage = self.storage_mut();
        storage
            .batch_insert_vertices(&space_name, vertices)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        drop(storage);

        // Bulk inserts bypass the statement pipeline, so the statement-level
        // DML notification is emitted here instead.
        self.notify_dml_op(DmlOp::Insert, &space_name, count as u64);
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
    pub fn batch_insert_edges(&self, edges: Vec<Edge>) -> CoreResult<usize> {
        self.ensure_writable_op()?;
        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        let count = edges.len();
        let mut storage = self.storage_mut();
        storage
            .batch_insert_edges(&space_name, edges)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        drop(storage);

        // Bulk inserts bypass the statement pipeline, so the statement-level
        // DML notification is emitted here instead.
        self.notify_dml_op(DmlOp::Insert, &space_name, count as u64);
        Ok(count)
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
    pub fn create_space(&self, name: &str, config: SpaceConfig) -> CoreResult<()> {
        self.ensure_writable_op()?;
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
        self.ensure_writable_op()?;
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
}

/// First whitespace-delimited keyword of a statement, used only for the
/// read-only gate (comment stripping is intentionally minimal: parsed
/// statements are classified first, this covers the remaining unparseable
/// mutating openers).
fn leading_keyword(query: &str) -> &str {
    let rest = query.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    &rest[..end]
}
