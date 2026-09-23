//! Session Management Module
//!
//! Provide the concept of a "session" as the context in which queries are executed.

use crate::api_core::{CoreError, CoreResult, QueryApi, QueryRequest, SchemaApi};
use crate::embedded::batch::BatchInserter;
use crate::embedded::result::QueryResult;
use crate::embedded::transaction::{Transaction, TransactionConfig};
use crate::storage::StorageClient;
use graphdb_core::SessionStatistics;
use graphdb_core::Value;
#[cfg(feature = "fulltext")]
use graphdb_fulltext::FulltextIndexManager;
use graphdb_metrics::StatsManager;
use graphdb_query::executor::expression::functions::{CustomFunction, FunctionRegistry};
use graphdb_query::parser::ast::Stmt;
use graphdb_query::parser::{Parser, ParserResult};
#[cfg(feature = "vector")]
use graphdb_sync::vector_sync::SearchOptions;
use graphdb_sync::SyncManager;
use graphdb_transaction::TransactionId;
use graphdb_transaction::TransactionManager;
use graphdb_transaction::TransactionOptions;
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
/// use graphdb_api::embedded::{GraphDatabase, DatabaseConfig};
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
    session_variables: Arc<crate::session_variables::SessionVariables>,
    /// Cooperative interrupt flag: set by `interrupt()` or the C-API
    /// `graphdb_connection_interrupt`; `execute*` entry points fail fast
    /// when set. Queries already running need the kill path below instead.
    interrupted: Arc<std::sync::atomic::AtomicBool>,
}

/// Internal structure of the database, used for sharing data between Session and GraphDatabase
#[repr(C)]
pub(crate) struct GraphDatabaseInner<S: StorageClient + Clone + 'static> {
    pub(crate) query_api: Arc<RwLock<QueryApi<S>>>,
    pub(crate) schema_api: SchemaApi<S>,
    pub(crate) txn_manager: Arc<TransactionManager>,
    pub(crate) storage: Arc<RwLock<S>>,
    #[cfg(feature = "fulltext")]
    pub(crate) fulltext_manager: Option<Arc<FulltextIndexManager>>,
    pub(crate) sync_manager: Option<Arc<SyncManager>>,
    pub(crate) stats_manager: Arc<StatsManager>,
    /// Central event-hook facade.
    pub(crate) hooks: crate::embedded::hooks::HookBus,
    /// Tokio runtime for vector operations in embedded mode.
    /// Stored here to ensure the runtime lives as long as the database.
    #[cfg(feature = "vector")]
    pub(crate) vector_runtime: Arc<tokio::runtime::Runtime>,
    /// Whether the database was opened read-only. Sessions consult this
    /// flag to reject mutating statements up front.
    pub(crate) read_only: bool,
}

impl<S: StorageClient + Clone + 'static + graphdb_storage::UndoTarget> Session<S> {
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
            session_variables: Arc::new(crate::session_variables::SessionVariables::new()),
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Register a custom function
    pub fn register_custom_function(&self, function: CustomFunction) -> CoreResult<()> {
        let mut registry = self.function_registry.write();
        registry.register_custom_full(function);
        Ok(())
    }

    /// Load a UDF dynamic library and register the exported function.
    ///
    /// Returns the registered function name.
    pub fn load_extension(&self, path: &std::path::Path) -> CoreResult<String> {
        let mut registry = self.function_registry.write();
        registry
            .load_dynamic_udf(path)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Unload a previously loaded dynamic UDF by function name.
    pub fn unload_extension(&self, name: &str) -> CoreResult<()> {
        let mut registry = self.function_registry.write();
        registry
            .unload_dynamic_udf(name)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Reload a dynamic UDF from its original library path.
    ///
    /// Returns `true` when the library was reloaded, `false` when the file
    /// is unchanged and reloading was skipped.
    pub fn reload_extension(&self, name: &str) -> CoreResult<bool> {
        let mut registry = self.function_registry.write();
        registry
            .reload_dynamic_udf(name)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// Install a UDF from an `INSTALL EXTENSION ... FROM` source.
    ///
    /// Local file paths load directly; `http(s)://` sources return a
    /// repository-unsupported error until a repository module is added.
    pub fn install_extension(&self, source: &str) -> CoreResult<String> {
        let mut registry = self.function_registry.write();
        registry
            .install_dynamic_udf(source)
            .map_err(|e| CoreError::InvalidParameter(e.to_string()))
    }

    /// List all loaded dynamic UDFs ordered by name.
    pub fn list_extensions(
        &self,
    ) -> Vec<graphdb_query::executor::expression::functions::registry::DynamicUdfInfo> {
        self.function_registry.read().list_dynamic_udfs()
    }

    /// Execute an extension management statement
    /// (`LOAD EXTENSION` / `INSTALL EXTENSION` / `UNINSTALL EXTENSION` /
    /// `UPDATE EXTENSION`).
    ///
    /// Extension statements run directly against the session function
    /// registry and never reach the query planner.
    fn execute_extension(
        &self,
        extension: &graphdb_query::parser::ast::stmt::ExtensionStmt,
    ) -> CoreResult<QueryResult> {
        use graphdb_query::parser::ast::stmt::ExtensionAction;
        let message = match extension.action {
            ExtensionAction::Load => {
                let name = self.load_extension(std::path::Path::new(&extension.name))?;
                format!("Loaded extension '{name}' from '{}'", extension.name)
            }
            ExtensionAction::Uninstall => {
                self.unload_extension(&extension.name)?;
                format!("Uninstalled extension '{}'", extension.name)
            }
            ExtensionAction::Install => {
                let source = extension.source.clone().unwrap_or_default();
                let name = self.install_extension(&source)?;
                format!("Installed extension '{name}' from '{source}'")
            }
            ExtensionAction::Update => {
                let reloaded = self.reload_extension(&extension.name)?;
                if reloaded {
                    format!("Updated extension '{}'", extension.name)
                } else {
                    format!("Extension '{}' is already up to date", extension.name)
                }
            }
        };
        Ok(Self::single_message_result(message))
    }

    /// Build a single-row, single-column result carrying an admin message.
    fn single_message_result(message: String) -> QueryResult {
        let columns = vec!["result".to_string()];
        let rows = vec![vec![Value::string(message)]];
        let execution = graphdb_query::executor::base::ExecutionResult::from_data_set(
            graphdb_core::types::DataSet::from_rows(rows, columns),
        );
        QueryResult::from_core(crate::api_core::types::QueryResult::new(
            execution,
            crate::api_core::types::ExecutionMetadata {
                rows_returned: 1,
                ..Default::default()
            },
        ))
    }

    /// Detect `CREATE TAG/EDGE ... AS (query)` statements.
    ///
    /// Returns the parsed `CreateStmt` when the statement materializes a
    /// table from a query; regular statements yield `None` and malformed
    /// input falls through to the normal pipeline for error reporting.
    fn parse_create_as(query: &str) -> Option<graphdb_query::parser::ast::stmt::CreateStmt> {
        if !Self::is_create_as_query(query) {
            return None;
        }
        let mut parser = Parser::new(query);
        match parser.parse() {
            Ok(result) if !parser.has_errors() => match result.ast.stmt() {
                Stmt::Create(stmt) => match &stmt.target {
                    graphdb_query::parser::ast::stmt::CreateTarget::TagAsQuery { .. }
                    | graphdb_query::parser::ast::stmt::CreateTarget::EdgeAsQuery { .. } => {
                        Some(stmt.clone())
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    /// Check whether `query` looks like `CREATE TAG/EDGE ... AS (query)`.
    ///
    /// Only a cheap keyword prefix check; full validation happens in the
    /// parser so nested subqueries are never misclassified here.
    fn is_create_as_query(query: &str) -> bool {
        let mut words = query.split_whitespace();
        let head = (
            words.next().map(|w| w.to_ascii_uppercase()),
            words.next().map(|w| w.to_ascii_uppercase()),
            words.next().map(|w| w.to_ascii_uppercase()),
        );
        if !matches!(
            head,
            (Some(ref a), Some(ref b), _) if a == "CREATE" && (b == "TAG" || b == "EDGE")
        ) {
            return false;
        }
        let upper = query.to_ascii_uppercase();
        upper.contains(" AS ") && upper.contains('(')
    }

    /// Execute `CREATE TAG/EDGE <name> AS (<query>)`.
    ///
    /// Runs the inner query on this session, derives the new table schema
    /// from its output columns, creates the table, and bulk-loads one
    /// vertex or edge per result row. Vertex imports require a `vid`/`id`
    /// column; edge imports require `src` and `dst` columns.
    fn execute_create_as(
        &self,
        stmt: &graphdb_query::parser::ast::stmt::CreateStmt,
    ) -> CoreResult<QueryResult> {
        use graphdb_query::parser::ast::stmt::CreateTarget;

        let (name, query_text, is_edge) = match &stmt.target {
            CreateTarget::TagAsQuery { name, query_text } => {
                (name.clone(), query_text.clone(), false)
            }
            CreateTarget::EdgeAsQuery { name, query_text } => {
                (name.clone(), query_text.clone(), true)
            }
            _ => {
                return Err(CoreError::InvalidParameter(
                    "Not a CREATE ... AS statement".to_string(),
                ));
            }
        };
        if name.trim().is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS requires a table name".to_string(),
            ));
        }
        if query_text.trim().is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS requires a subquery".to_string(),
            ));
        }
        if self.current_transaction.read().is_some() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS cannot run inside an explicit transaction".to_string(),
            ));
        }
        if Self::is_create_as_query(&query_text) {
            return Err(CoreError::InvalidParameter(
                "Nested CREATE ... AS is not supported".to_string(),
            ));
        }

        let inner = self.execute(&query_text)?;
        let columns = inner.columns().to_vec();
        if columns.is_empty() {
            return Err(CoreError::InvalidParameter(
                "CREATE ... AS inner query returned no columns".to_string(),
            ));
        }

        let space_name = self
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;
        let space_id = match *self.space_id.read() {
            Some(id) => id,
            None => self.db.schema_api.use_space(&space_name)?,
        };

        if is_edge {
            self.create_edge_as(
                &space_name,
                space_id,
                &name,
                stmt.if_not_exists,
                &columns,
                &inner,
            )
        } else {
            self.create_tag_as(
                &space_name,
                space_id,
                &name,
                stmt.if_not_exists,
                &columns,
                &inner,
            )
        }
    }

    /// Materialize inner query rows as a new tag (vertex table).
    fn create_tag_as(
        &self,
        space_name: &str,
        space_id: u64,
        name: &str,
        if_not_exists: bool,
        columns: &[String],
        inner: &QueryResult,
    ) -> CoreResult<QueryResult> {
        let vid_col = find_result_column(columns, &["vid", "id", "_id", "vertex_id"])
            .ok_or_else(|| {
                CoreError::InvalidParameter(
                    "CREATE TAG ... AS requires the inner query to output a vertex id column (vid/id)"
                        .to_string(),
                )
            })?;
        let prop_cols: Vec<String> = columns.iter().filter(|c| *c != &vid_col).cloned().collect();

        let exists = self
            .db
            .storage
            .read()
            .get_tag(space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .is_some();
        if exists {
            if if_not_exists {
                return Ok(Self::single_message_result(format!(
                    "Tag '{name}' already exists"
                )));
            }
            return Err(CoreError::InvalidParameter(format!(
                "Tag '{name}' already exists"
            )));
        }

        let properties = infer_result_properties(&prop_cols, inner.rows());
        self.db
            .schema_api
            .create_tag(space_id, name, properties)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        let mut vertices = Vec::with_capacity(inner.len());
        for row in inner.rows() {
            let vid_value = row.get(&vid_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE TAG ... AS: missing vertex id in column '{vid_col}'"
                ))
            })?;
            let vid = vid_from_value(vid_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE TAG ... AS: value in column '{vid_col}' cannot be used as a vertex id"
                ))
            })?;
            let mut props = std::collections::HashMap::new();
            for col in &prop_cols {
                if let Some(value) = row.get(col) {
                    ensure_scalar_property(col, value)?;
                    if matches!(value, Value::Null(_) | graphdb_core::Value::Empty) {
                        continue;
                    }
                    props.insert(col.clone(), value.clone());
                }
            }
            vertices.push(graphdb_core::Vertex::new(
                vid,
                vec![graphdb_core::Tag::new(name.to_string(), props)],
            ));
        }
        let count = self.batch_insert_vertices(vertices)?;
        self.statistics.record_changes(count as u64);
        Ok(Self::single_message_result(format!(
            "Created tag '{name}' with {count} vertices from query"
        )))
    }

    /// Materialize inner query rows as a new edge type.
    fn create_edge_as(
        &self,
        space_name: &str,
        space_id: u64,
        name: &str,
        if_not_exists: bool,
        columns: &[String],
        inner: &QueryResult,
    ) -> CoreResult<QueryResult> {
        let src_col = find_result_column(columns, &["src", "_src", "source"]);
        let dst_col = find_result_column(columns, &["dst", "_dst", "destination", "dest"]);
        let (src_col, dst_col) = match (src_col, dst_col) {
            (Some(s), Some(d)) => {
                if s == d {
                    return Err(CoreError::InvalidParameter(
                        "CREATE EDGE ... AS: src and dst resolve to the same column".to_string(),
                    ));
                }
                (s, d)
            }
            (None, None) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS requires the inner query to output src and dst columns"
                        .to_string(),
                ));
            }
            (Some(_), None) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS: inner query names a source column but no destination column (dst)"
                        .to_string(),
                ));
            }
            (None, Some(_)) => {
                return Err(CoreError::InvalidParameter(
                    "CREATE EDGE ... AS: inner query names a destination column but no source column (src)"
                        .to_string(),
                ));
            }
        };
        let prop_cols: Vec<String> = columns
            .iter()
            .filter(|c| *c != &src_col && *c != &dst_col)
            .cloned()
            .collect();

        let exists = self
            .db
            .storage
            .read()
            .get_edge_type(space_name, name)
            .map_err(|e| CoreError::StorageError(e.to_string()))?
            .is_some();
        if exists {
            if if_not_exists {
                return Ok(Self::single_message_result(format!(
                    "Edge type '{name}' already exists"
                )));
            }
            return Err(CoreError::InvalidParameter(format!(
                "Edge type '{name}' already exists"
            )));
        }

        let properties = infer_result_properties(&prop_cols, inner.rows());
        self.db
            .schema_api
            .create_edge_type(space_id, name, properties)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;

        let mut edges = Vec::with_capacity(inner.len());
        for row in inner.rows() {
            let src_value = row.get(&src_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: missing source id in column '{src_col}'"
                ))
            })?;
            let dst_value = row.get(&dst_col).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: missing destination id in column '{dst_col}'"
                ))
            })?;
            let src = vid_from_value(src_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: value in column '{src_col}' cannot be used as a vertex id"
                ))
            })?;
            let dst = vid_from_value(dst_value).ok_or_else(|| {
                CoreError::InvalidParameter(format!(
                    "CREATE EDGE ... AS: value in column '{dst_col}' cannot be used as a vertex id"
                ))
            })?;
            let mut props = std::collections::HashMap::new();
            for col in &prop_cols {
                if let Some(value) = row.get(col) {
                    ensure_scalar_property(col, value)?;
                    if matches!(value, Value::Null(_) | graphdb_core::Value::Empty) {
                        continue;
                    }
                    props.insert(col.clone(), value.clone());
                }
            }
            edges.push(graphdb_core::Edge::new(
                src,
                dst,
                name.to_string(),
                0,
                props,
            ));
        }
        let count = self.batch_insert_edges(edges)?;
        self.statistics.record_changes(count as u64);
        Ok(Self::single_message_result(format!(
            "Created edge type '{name}' with {count} edges from query"
        )))
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

    /// Classify `query` and emit a statement-level DML notification when it
    /// is a write. No-op for read-only text and when nobody subscribes
    /// (`HookBus::subscribe_dml`). Returns the classified operation so
    /// callers sharing the classification (e.g. the C-API update hook) do
    /// not scan the text twice.
    pub(crate) fn notify_dml(&self, query: &str, rows: u64) -> Option<graphdb_query::DmlOp> {
        let op = graphdb_query::classify_dml(query)?;
        let space = self
            .current_space()
            .unwrap_or_else(|| "default".to_string());
        self.db.hooks.emit_dml(op, &space, rows);
        Some(op)
    }

    /// Whether any statement-level DML observer is registered.
    pub(crate) fn has_dml_observers(&self) -> bool {
        self.db.hooks.has_dml_observers()
    }

    /// After executing a query, check if the result represents a space switch
    /// (from USE <space>), and persist the new space context on this session.
    ///
    /// The core QueryApi converts SpaceSwitched to a QueryResult with
    /// "space_name", "space_id", "vid_type" columns. This method detects
    /// that pattern and updates the session's space state accordingly.
    fn update_space_from_result(&self, result: &crate::api_core::types::QueryResult) {
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

    fn ensure_not_interrupted(&self) -> CoreResult<()> {
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
    /// `classify_dml` covers DML plus leading CREATE/DROP/ALTER/INSERT/
    /// DELETE-family keywords; the extra set closes the remaining mutating
    /// openers (bulk load/export/attach and extension management).
    pub(crate) fn ensure_writable(&self, query: &str) -> CoreResult<()> {
        if !self.db.read_only {
            return Ok(());
        }
        if graphdb_query::classify_dml(query).is_some() {
            return self.ensure_writable_op();
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
        self.ensure_not_interrupted()?;
        self.ensure_writable(query)?;
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
            consistency: Default::default(),
        };

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

        self.notify_dml(query, result.metadata.rows_returned as u64);

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
            || upper.starts_with("LET ")
            || upper.starts_with("LOAD EXTENSION")
            || upper.starts_with("INSTALL EXTENSION")
            || upper.starts_with("UPDATE EXTENSION")
            || upper.starts_with("UNINSTALL EXTENSION");
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
                    | Stmt::AssignVariable(_)
                    | Stmt::Extension(_) => Ok(Some(result)),
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
        parsed_ast: Arc<graphdb_query::parser::ast::stmt::Ast>,
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
        parsed_ast: Arc<graphdb_query::parser::ast::stmt::Ast>,
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
            consistency: Default::default(),
        };
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
        self.ensure_writable(query)?;
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
            consistency: Default::default(),
        };

        let result = {
            let mut query_api = self.db.query_api.write();
            query_api.execute_with_execution(query, query_ctx, &execution)?
        };
        txn_manager
            .finish_statement(&ctx, statement_start)
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))?;
        self.notify_dml(query, result.metadata.rows_returned as u64);
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
        parsed_ast: Arc<graphdb_query::parser::ast::stmt::Ast>,
        assign: &graphdb_query::parser::ast::stmt::AssignVariableStmt,
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
            consistency: Default::default(),
        };

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
        Ok(QueryResult::from_core(
            crate::api_core::types::QueryResult::new(
                graphdb_query::executor::base::ExecutionResult::from_data_set(
                    graphdb_core::types::DataSet::from_rows(rows, columns),
                ),
                result.metadata,
            ),
        ))
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
            consistency: Default::default(),
        };

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

        self.notify_dml(query, result.metadata.rows_returned as u64);

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
            consistency: Default::default(),
        };

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

        self.notify_dml(query, result.metadata.rows_returned as u64);

        Ok(QueryResult::from_core(result))
    }

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
            .map_err(|e| crate::api_core::error::CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = crate::api_core::types::TransactionHandle(txn_id);

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
    /// use graphdb_api::embedded::{GraphDatabase, TransactionConfig};
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
            .map_err(|e| crate::api_core::error::CoreError::TransactionFailed(e.to_string()))?;
        let txn_handle = crate::api_core::types::TransactionHandle(txn_id);

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
        config: crate::api_core::types::SpaceConfig,
    ) -> CoreResult<()> {
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
    /// use graphdb_api::embedded::GraphDatabase;
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
    pub fn batch_insert_vertices(&self, vertices: Vec<graphdb_core::Vertex>) -> CoreResult<usize> {
        self.ensure_writable_op()?;
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
    pub fn batch_insert_edges(&self, edges: Vec<graphdb_core::Edge>) -> CoreResult<usize> {
        self.ensure_writable_op()?;
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
        txn_handle: crate::api_core::types::TransactionHandle,
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
        txn_handle: crate::api_core::types::TransactionHandle,
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
        txn_handle: &crate::api_core::types::TransactionHandle,
        name: &str,
    ) -> CoreResult<crate::api_core::types::SavepointId> {
        self.txn_manager()
            .create_savepoint(txn_handle.0, Some(name.to_string()))
            .map_err(|e| CoreError::TransactionFailed(e.to_string()))
            .map(crate::api_core::types::SavepointId)
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
        txn_handle: &crate::api_core::types::TransactionHandle,
        savepoint: crate::api_core::types::SavepointId,
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
        txn_handle: &crate::api_core::types::TransactionHandle,
        savepoint: crate::api_core::types::SavepointId,
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
    ) -> CoreResult<Vec<crate::api_core::VectorSearchResult>> {
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
            .map(|r| crate::api_core::VectorSearchResult {
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
    ) -> CoreResult<Vec<crate::api_core::VectorSearchResult>> {
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
            .map(|r| crate::api_core::VectorSearchResult {
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

/// Locate a result column by case-insensitive name among several aliases.
fn find_result_column(columns: &[String], aliases: &[&str]) -> Option<String> {
    columns
        .iter()
        .find(|c| aliases.iter().any(|a| c.eq_ignore_ascii_case(a)))
        .cloned()
}

/// First whitespace-delimited keyword of a statement, used only for the
/// read-only gate (comment stripping is intentionally minimal: statements
/// are classified by `classify_dml` first, this covers the remaining
/// mutating openers).
fn leading_keyword(query: &str) -> &str {
    let rest = query.trim_start();
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Derive property definitions from query result columns.
///
/// Each column takes the type of its first non-null value; columns that
/// only carry nulls default to strings.
fn infer_result_properties(
    columns: &[String],
    rows: &[crate::embedded::result::Row],
) -> Vec<crate::api_core::types::PropertyDef> {
    columns
        .iter()
        .map(|col| {
            let mut data_type = graphdb_core::DataType::String;
            for row in rows {
                match row.get(col) {
                    Some(Value::Null(_)) | Some(graphdb_core::Value::Empty) | None => continue,
                    Some(value) => {
                        data_type = infer_result_data_type(value);
                        break;
                    }
                }
            }
            crate::api_core::types::PropertyDef {
                name: col.clone(),
                data_type,
                nullable: true,
                default_value: None,
                comment: None,
            }
        })
        .collect()
}

/// Map a result value to the table column type used for `CREATE ... AS`.
fn infer_result_data_type(value: &Value) -> graphdb_core::DataType {
    use graphdb_core::DataType;
    match value {
        Value::Null(_) | Value::Empty => DataType::String,
        Value::Bool(_) => DataType::Bool,
        Value::SmallInt(_) => DataType::SmallInt,
        Value::Int(_) => DataType::Int,
        Value::BigInt(_) => DataType::BigInt,
        Value::Float(_) => DataType::Float,
        Value::Double(_) => DataType::Double,
        Value::Decimal128(_) => DataType::Decimal128,
        Value::String(_) | Value::FixedString(_) => DataType::String,
        Value::Date(_) => DataType::Date,
        Value::Time(_) => DataType::Time,
        Value::DateTime(_) => DataType::DateTime,
        Value::Uuid(_) => DataType::Uuid,
        Value::Json(_) => DataType::Json,
        Value::JsonB(_) => DataType::JsonB,
        Value::Blob(_) => DataType::Blob,
        Value::List(_) => DataType::List(Box::new(DataType::Empty)),
        Value::Map(_) => DataType::Map(Box::new(DataType::Empty)),
        Value::Set(_) => DataType::Set(Box::new(DataType::Empty)),
        _ => DataType::String,
    }
}

/// Reject graph and container values as table property columns.
fn ensure_scalar_property(col: &str, value: &Value) -> CoreResult<()> {
    let complex = matches!(
        value,
        Value::Vertex(_)
            | Value::Edge(_)
            | Value::Path(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::DataSet(_)
            | Value::Struct(_)
            | Value::Array(_)
            | Value::Vector(_)
            | Value::VertexId(_)
            | Value::EdgeId(_)
    );
    if complex {
        return Err(CoreError::InvalidParameter(format!(
            "CREATE ... AS: column '{col}' carries a graph or container value; project scalar properties instead"
        )));
    }
    Ok(())
}

/// Read a vertex id from a result value.
///
/// Numeric values map to the integer id domain, strings become string ids,
/// and vertex values contribute their own id.
fn vid_from_value(value: &Value) -> Option<graphdb_core::types::storage_ids::VertexId> {
    use graphdb_core::types::storage_ids::VertexId;
    match value {
        Value::SmallInt(i) => VertexId::try_from_int64(i64::from(*i)).ok(),
        Value::Int(i) => VertexId::try_from_int64(i64::from(*i)).ok(),
        Value::BigInt(i) => VertexId::try_from_int64(*i).ok(),
        Value::String(s) => VertexId::try_from_string(s.as_str()).ok(),
        Value::FixedString(s) => VertexId::try_from_string(s.as_str()).ok(),
        Value::Vertex(v) => Some(*v.vid()),
        Value::VertexId(id) => Some(*id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::database::GraphDatabase;
    use crate::storage::MockStorage;

    #[test]
    fn interrupt_flag_fails_execute_fast_and_clears() {
        let db = GraphDatabase::<MockStorage>::open_test().expect("test database opens");
        let session = db.session().expect("session creates");
        assert!(!session.is_interrupted());

        session.interrupt();
        assert!(session.is_interrupted());
        let err = session
            .execute("MATCH (n) RETURN n")
            .expect_err("interrupted session must fail fast");
        assert!(
            err.to_string().contains("interrupted"),
            "unexpected error: {err}"
        );

        session.clear_interrupt();
        assert!(!session.is_interrupted());
    }
}
