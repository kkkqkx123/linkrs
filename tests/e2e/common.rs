//! Common test utilities for E2E tests
//!
//! Uses the QueryApi with schema manager for proper initialization.
//! This is the recommended way to create test databases for E2E tests.

use graphdb::api::core::query_api::QueryApi;
use graphdb::api::core::types::QueryResult;
use graphdb::api::core::CoreResult;
use graphdb::core::metadata::SchemaManager;
use graphdb::core::StatsManager;
use graphdb::core::Value;
use graphdb::query::executor::streaming::StreamingQueryResult;
use graphdb::storage::{GraphStorage, StorageSchemaContextOps};
use graphdb::sync::SyncManager;
use graphdb::transaction::{
    TransactionId, TransactionManager, TransactionManagerConfig, TransactionOptions,
};
use parking_lot::RwLock;
use std::sync::Arc;
use tempfile::TempDir;

#[cfg(feature = "fulltext-search")]
use graphdb::search::{FulltextConfig, FulltextIndexManager};
#[cfg(feature = "fulltext-search")]
use graphdb::sync::SyncConfig;

#[cfg(feature = "vector-qdrant")]
use vector_client::{HealthStatus, VectorClientConfig, VectorManager};

/// Test database wrapper with proper schema manager initialization
pub struct TestDb {
    /// RAII guard that keeps the temp directory alive for the lifetime of `TestDb`.
    /// Must not be dropped (the directory is deleted on drop), so this field is
    /// never read — it exists solely for lifetime management.
    #[allow(dead_code)]
    temp_dir: Option<TempDir>,
    storage: Arc<RwLock<GraphStorage>>,
    stats_manager: Arc<StatsManager>,
    schema_manager: Arc<SchemaManager>,
    query_api: QueryApi<GraphStorage>,
    transaction_manager: Arc<TransactionManager>,
    current_space_id: Option<u64>,
    current_space_name: Option<String>,
    current_transaction: Option<TransactionId>,
    /// Whether a vector coordinator is available (Qdrant is running and healthy).
    /// Vector tests check this to skip gracefully when Qdrant is not available.
    pub has_vector_coordinator: bool,
}

fn create_sync_manager() -> Arc<SyncManager> {
    #[cfg(feature = "fulltext-search")]
    let sync_manager = {
        let fulltext_temp_dir = tempfile::tempdir().expect("Failed to create fulltext temp dir");
        let fulltext_config = FulltextConfig {
            index_path: fulltext_temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let manager = Arc::new(
            FulltextIndexManager::new(fulltext_config).expect("Failed to create fulltext manager"),
        );
        // TempDir is intentionally leaked so the directory lives for the process lifetime
        // (Tantivy lock files must remain accessible for the duration of all tests)
        std::mem::forget(fulltext_temp_dir);
        let sync_config = SyncConfig::default();
        let batch_config = graphdb::sync::batch::BatchConfig::from(sync_config.clone());
        let sync_coordinator = Arc::new(graphdb::sync::coordinator::SyncCoordinator::new(
            manager,
            batch_config,
        ));

        SyncManager::with_sync_config(sync_coordinator, sync_config)
    };

    #[cfg(not(feature = "fulltext-search"))]
    let sync_manager = SyncManager::new_without_fulltext();

    #[cfg(feature = "vector-qdrant")]
    let sync_manager = {
        let mut sync_manager = sync_manager;
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        match rt.block_on(VectorManager::new(VectorClientConfig::qdrant())) {
            Ok(vector_manager) => {
                let health = rt
                    .block_on(vector_manager.engine().health_check())
                    .unwrap_or_else(|_| {
                        HealthStatus::unhealthy("unknown", "unknown", "health check failed")
                    });
                if health.is_healthy {
                    let vector_coordinator =
                        Arc::new(graphdb::sync::vector_sync::VectorSyncCoordinator::new(
                            graphdb::sync::VectorBackend::Qdrant(Arc::new(vector_manager)),
                            None,
                            rt.handle().clone(),
                        ));
                    sync_manager = sync_manager.with_vector_coordinator(vector_coordinator);
                } else {
                    eprintln!(
                        "WARNING: Qdrant connected but not healthy. Vector tests will be skipped."
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "WARNING: Failed to connect to Qdrant ({}). Vector tests will be skipped.",
                    e
                );
            }
        }
        sync_manager
    };

    Arc::new(sync_manager)
}

#[allow(clippy::new_without_default)]
impl TestDb {
    /// Create a new test database with a temporary file
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let storage = Arc::new(RwLock::new(
            GraphStorage::open(db_path).expect("Failed to create storage"),
        ));
        let stats_manager = Arc::new(StatsManager::new());
        let schema_manager = storage
            .read()
            .get_schema_manager()
            .expect("Storage should provide a schema manager");

        let sync_manager = create_sync_manager();
        let has_vector_coordinator = {
            #[cfg(feature = "vector-qdrant")]
            {
                sync_manager.vector_coordinator().is_some()
            }
            #[cfg(not(feature = "vector-qdrant"))]
            {
                false
            }
        };
        let query_api = QueryApi::with_schema_and_sync_manager(
            storage.clone(),
            stats_manager.clone(),
            schema_manager.clone(),
            sync_manager,
        );
        let transaction_manager = Arc::new(TransactionManager::with_shared_version_manager(
            TransactionManagerConfig::default(),
            stats_manager.clone(),
            storage.read().version_manager(),
        ));

        Self {
            temp_dir: Some(temp_dir),
            storage,
            stats_manager,
            schema_manager,
            query_api,
            transaction_manager,
            current_space_id: None,
            current_space_name: None,
            current_transaction: None,
            has_vector_coordinator,
        }
    }

    /// Create a new test database in memory
    pub fn new_in_memory() -> Self {
        let storage = Arc::new(RwLock::new(
            GraphStorage::new().expect("Failed to create storage"),
        ));
        let stats_manager = Arc::new(StatsManager::new());
        let schema_manager = storage
            .read()
            .get_schema_manager()
            .expect("Storage should provide a schema manager");

        let sync_manager = create_sync_manager();
        let has_vector_coordinator = {
            #[cfg(feature = "vector-qdrant")]
            {
                sync_manager.vector_coordinator().is_some()
            }
            #[cfg(not(feature = "vector-qdrant"))]
            {
                false
            }
        };
        let query_api = QueryApi::with_schema_and_sync_manager(
            storage.clone(),
            stats_manager.clone(),
            schema_manager.clone(),
            sync_manager,
        );
        let transaction_manager = Arc::new(TransactionManager::with_shared_version_manager(
            TransactionManagerConfig::default(),
            stats_manager.clone(),
            storage.read().version_manager(),
        ));

        Self {
            temp_dir: None,
            storage,
            stats_manager,
            schema_manager,
            query_api,
            transaction_manager,
            current_space_id: None,
            current_space_name: None,
            current_transaction: None,
            has_vector_coordinator,
        }
    }

    /// Get a reference to the storage
    pub fn storage(&self) -> Arc<RwLock<GraphStorage>> {
        self.storage.clone()
    }

    /// Get a reference to the stats manager
    pub fn stats_manager(&self) -> Arc<StatsManager> {
        self.stats_manager.clone()
    }

    /// Get a reference to the schema manager
    pub fn schema_manager(&self) -> Arc<SchemaManager> {
        self.schema_manager.clone()
    }

    /// Get a reference to the query API
    pub fn query_api(&self) -> &QueryApi<GraphStorage> {
        &self.query_api
    }

    /// Execute a query using a persistent session context
    pub fn execute_query(&mut self, query: &str) -> CoreResult<QueryResult> {
        let trimmed = query.trim().to_uppercase();
        if trimmed.starts_with("BEGIN") || trimmed.starts_with("START TRANSACTION") {
            if self.current_transaction.is_some() {
                return Err(graphdb::api::core::CoreError::QueryExecutionFailed(
                    "A transaction is already active".to_string(),
                ));
            }
            let mut options = TransactionOptions::default();
            if let Some(access_mode) = parse_begin_access_mode(query)? {
                if access_mode {
                    options = options.read_only();
                }
            }
            let txn_id = self
                .transaction_manager
                .begin_transaction(options)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            self.current_transaction = Some(txn_id);
            return Ok(empty_query_result());
        }
        if trimmed.starts_with("SAVEPOINT") {
            let name = query["SAVEPOINT".len()..].trim().to_string();
            if name.is_empty() {
                return Err(graphdb::api::core::CoreError::QueryExecutionFailed(
                    "Savepoint name cannot be empty".to_string(),
                ));
            }
            let txn_id = self.current_transaction.ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(
                    "No active transaction, cannot create savepoint".to_string(),
                )
            })?;
            self.transaction_manager
                .create_savepoint(txn_id, Some(name))
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            return Ok(empty_query_result());
        }
        if trimmed.starts_with("RELEASE SAVEPOINT") {
            let name = query["RELEASE SAVEPOINT".len()..].trim().to_string();
            if name.is_empty() {
                return Err(graphdb::api::core::CoreError::QueryExecutionFailed(
                    "Savepoint name cannot be empty".to_string(),
                ));
            }
            let txn_id = self.current_transaction.ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(
                    "No active transaction, cannot release savepoint".to_string(),
                )
            })?;
            let context = self
                .transaction_manager
                .get_context(txn_id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            let savepoint = context.find_savepoint_by_name(&name).ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(format!(
                    "Savepoint '{}' does not exist",
                    name
                ))
            })?;
            self.transaction_manager
                .release_savepoint(txn_id, savepoint.id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            return Ok(empty_query_result());
        }
        if trimmed.starts_with("ROLLBACK TO") {
            let name = query["ROLLBACK TO".len()..].trim().to_string();
            if name.is_empty() {
                return Err(graphdb::api::core::CoreError::QueryExecutionFailed(
                    "Savepoint name cannot be empty".to_string(),
                ));
            }
            let txn_id = self.current_transaction.ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(
                    "No active transaction, cannot rollback to savepoint".to_string(),
                )
            })?;
            let context = self
                .transaction_manager
                .get_context(txn_id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            let savepoint = context.find_savepoint_by_name(&name).ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(format!(
                    "Savepoint '{}' does not exist",
                    name
                ))
            })?;
            let storage = self.storage.read();
            self.transaction_manager
                .rollback_to_savepoint(txn_id, savepoint.id, &*storage)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            return Ok(empty_query_result());
        }
        if trimmed.starts_with("COMMIT") {
            let txn_id = self.current_transaction.ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(
                    "No active transaction to commit".to_string(),
                )
            })?;
            self.transaction_manager
                .commit_transaction(txn_id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            self.current_transaction = None;
            return Ok(empty_query_result());
        }
        if trimmed.starts_with("ROLLBACK") {
            let txn_id = self.current_transaction.ok_or_else(|| {
                graphdb::api::core::CoreError::QueryExecutionFailed(
                    "No active transaction to roll back".to_string(),
                )
            })?;
            self.transaction_manager
                .abort_transaction(txn_id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            self.current_transaction = None;
            return Ok(empty_query_result());
        }

        // Statements inside an explicit transaction are executed against a
        // transaction-bound storage context (mirroring the embedded Session
        // flow) so writes stay isolated until COMMIT / ROLLBACK.
        let result = if let Some(txn_id) = self.current_transaction {
            let (ctx, statement_start) = self
                .transaction_manager
                .begin_statement(txn_id)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            let execution = self
                .transaction_manager
                .create_execution(txn_id, false)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            let txn_ctx = graphdb::api::core::types::QueryRequest {
                space_id: self.current_space_id,
                space_name: self.current_space_name.clone(),
                auto_commit: false,
                transaction_id: Some(txn_id),
                parameters: None,
                session_variables: None,
                query_id: None,
                parsed_statement: None,
            };
            let result = self
                .query_api
                .execute_with_execution(query, txn_ctx, &execution);
            self.transaction_manager
                .finish_statement(&ctx, statement_start)
                .map_err(|e| graphdb::api::core::CoreError::QueryExecutionFailed(e.to_string()))?;
            result
        } else {
            let ctx = graphdb::api::core::types::QueryRequest {
                space_id: self.current_space_id,
                space_name: self.current_space_name.clone(),
                auto_commit: true,
                transaction_id: None,
                parameters: None,
                session_variables: None,
                query_id: None,
                parsed_statement: None,
            };
            self.query_api.execute(query, ctx)
        };
        let result = result?;

        // Track space switching from USE statements
        self.track_space_from_result(&result);

        Ok(result)
    }

    /// Execute a read query through the streaming path, returning a streaming handle.
    ///
    /// Routes through `QueryApi::execute_stream` so the streaming plan-cache
    /// path is exercised (as opposed to `execute_query`, which materializes).
    pub fn execute_stream_query(&mut self, query: &str) -> CoreResult<StreamingQueryResult> {
        let ctx = graphdb::api::core::types::QueryRequest {
            space_id: self.current_space_id,
            space_name: self.current_space_name.clone(),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
        };
        self.query_api.execute_stream(query, ctx)
    }

    /// Execute a statement as an independent auto-commit session, ignoring
    /// any session transaction tracked by this handle (simulates a second
    /// client). Transaction control statements are NOT handled here.
    pub fn execute_external(&mut self, query: &str) -> CoreResult<QueryResult> {
        let ctx = graphdb::api::core::types::QueryRequest {
            space_id: self.current_space_id,
            space_name: self.current_space_name.clone(),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
        };
        self.query_api.execute(query, ctx)
    }

    /// Execute a batch of auto-commit statements in one storage window (P6).
    ///
    /// Statements must be plain auto-commit statements (no BEGIN/COMMIT/ROLLBACK
    /// /USE); each still commits and rolls back independently. The first failing
    /// statement aborts the load (later statements were still executed by the
    /// window but their results are discarded).
    pub fn execute_batch(&mut self, statements: &[String]) -> CoreResult<Vec<QueryResult>> {
        let ctx = graphdb::api::core::types::QueryRequest {
            space_id: self.current_space_id,
            space_name: self.current_space_name.clone(),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
        };
        let outcomes = self.query_api.execute_batch(statements, ctx);
        let mut results = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            let result = outcome?;
            self.track_space_from_result(&result);
            results.push(result);
        }
        Ok(results)
    }

    /// Execute a batch of auto-commit statements using group-commit windows
    /// (P0 C). Each group of `group_size` statements shares one write timestamp
    /// and one WAL fsync.
    pub fn execute_batch_grouped(
        &mut self,
        statements: &[String],
        group_size: usize,
    ) -> CoreResult<Vec<QueryResult>> {
        let ctx = graphdb::api::core::types::QueryRequest {
            space_id: self.current_space_id,
            space_name: self.current_space_name.clone(),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
        };
        let outcomes = self
            .query_api
            .execute_batch_grouped(statements, ctx, group_size);
        let mut results = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            let result = outcome?;
            self.track_space_from_result(&result);
            results.push(result);
        }
        Ok(results)
    }

    /// Apply space-switch state from a USE result.
    fn track_space_from_result(&mut self, result: &QueryResult) {
        let columns = result.columns();
        if !columns.iter().any(|c| c == "space_name") {
            return;
        }
        if let Some(row) = result.rows().first() {
            if let Some(Value::String(name)) = columns
                .iter()
                .position(|c| c == "space_name")
                .and_then(|i| row.get(i))
            {
                self.current_space_name = Some(name.to_string());
            }
            if let Some(Value::BigInt(id)) = columns
                .iter()
                .position(|c| c == "space_id")
                .and_then(|i| row.get(i))
            {
                self.current_space_id = Some(*id as u64);
            }
        }
    }
}

/// Parse the transaction access mode from a `BEGIN [TRANSACTION] [READ
/// ONLY | READ WRITE]` statement.
///
/// Returns `Ok(Some(true))` for READ ONLY, `Ok(Some(false))` for READ
/// WRITE and `Ok(None)` when no access mode is specified. Malformed
/// suffixes (e.g. `BEGIN READ`) are rejected.
fn parse_begin_access_mode(stmt: &str) -> CoreResult<Option<bool>> {
    let upper = stmt.trim().to_uppercase();
    let (_, suffix) = if let Some(pos) = upper.find("READ") {
        (&upper[..pos], &upper[pos..])
    } else {
        return Ok(None);
    };
    let suffix = suffix.trim_start();
    if suffix.starts_with("READ ONLY") {
        return Ok(Some(true));
    }
    if suffix.starts_with("READ WRITE") {
        return Ok(Some(false));
    }
    Err(graphdb::api::core::CoreError::QueryExecutionFailed(
        "Invalid BEGIN access mode, expected READ ONLY or READ WRITE".to_string(),
    ))
}

fn empty_query_result() -> QueryResult {
    QueryResult::empty()
}

/// Create a test database
pub fn create_test_db() -> TestDb {
    TestDb::new()
}

/// Create an in-memory test database
pub fn create_test_db_in_memory() -> TestDb {
    TestDb::new_in_memory()
}

/// Setup a test space with schema
///
/// Creates a space, uses it, and creates the provided tags and edges.
/// Returns the test db for further operations.
pub fn setup_test_space(
    db: &mut TestDb,
    space_name: &str,
    tags: &[&str],
    edges: &[&str],
) -> CoreResult<()> {
    // Drop space if exists (ignore error)
    let _ = db.execute_query(&format!("DROP SPACE IF EXISTS {}", space_name));

    // Create and use space
    db.execute_query(&format!("CREATE SPACE {} (vid_type=STRING)", space_name))?;
    db.execute_query(&format!("USE {}", space_name))?;

    // Create tags
    for tag in tags {
        db.execute_query(tag)?;
    }

    // Create edges
    for edge in edges {
        db.execute_query(edge)?;
    }

    Ok(())
}

/// Assert that a query succeeds
pub fn assert_query_ok<T: std::fmt::Debug>(result: CoreResult<T>, context: &str) {
    assert!(result.is_ok(), "{}: {:?}", context, result.err());
}

/// Assert that a query fails
pub fn assert_query_err<T: std::fmt::Debug>(result: CoreResult<T>, context: &str) {
    assert!(result.is_err(), "{}: expected error but got Ok", context);
}

/// Load and execute a GQL data file
///
/// Reads the file line-by-line.  Blank lines and comment lines (`--`)
/// are statement separators.  Continuation lines (indented, or starting
/// with `)`) are appended to the current statement.
///
/// Consecutive `INSERT` statements are executed as one batch window (P6) so
/// the auto-commit write gate and MVCC snapshot registrations are shared
/// across the run; everything else (BEGIN/COMMIT/ROLLBACK, USE, DDL, reads)
/// runs statement-by-statement via `execute_query`.
pub fn load_gql_file(db: &mut TestDb, path: &str) -> CoreResult<()> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        graphdb::api::core::CoreError::Internal(format!("Failed to read {}: {}", path, e))
    })?;

    let mut statements: Vec<String> = Vec::new();
    let mut buffer = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || trimmed.starts_with(')') {
            buffer.push(' ');
            buffer.push_str(trimmed);
        } else {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            buffer = trimmed.to_string();
        }
    }
    if !buffer.is_empty() {
        statements.push(buffer);
    }

    let is_insert = |statement: &str| statement.trim().to_uppercase().starts_with("INSERT ");
    let mut index = 0;
    while index < statements.len() {
        if is_insert(&statements[index]) {
            let mut batch = vec![statements[index].clone()];
            index += 1;
            while index < statements.len() && is_insert(&statements[index]) {
                batch.push(statements[index].clone());
                index += 1;
            }
            db.execute_batch(&batch)?;
        } else {
            db.execute_query(&statements[index])?;
            index += 1;
        }
    }

    Ok(())
}

/// Load a GQL file, executing consecutive INSERT statements via group-commit
/// windows (P0 C). Non-INSERT statements execute individually.
pub fn load_gql_file_grouped(db: &mut TestDb, path: &str, group_size: usize) -> CoreResult<()> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        graphdb::api::core::CoreError::Internal(format!("Failed to read {}: {}", path, e))
    })?;

    let mut statements: Vec<String> = Vec::new();
    let mut buffer = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') || trimmed.starts_with(')') {
            buffer.push(' ');
            buffer.push_str(trimmed);
        } else {
            if !buffer.is_empty() {
                statements.push(std::mem::take(&mut buffer));
            }
            buffer = trimmed.to_string();
        }
    }
    if !buffer.is_empty() {
        statements.push(buffer);
    }

    let is_insert = |statement: &str| statement.trim().to_uppercase().starts_with("INSERT ");
    let mut index = 0;
    while index < statements.len() {
        if is_insert(&statements[index]) {
            let mut batch = vec![statements[index].clone()];
            index += 1;
            while index < statements.len() && is_insert(&statements[index]) {
                batch.push(statements[index].clone());
                index += 1;
            }
            db.execute_batch_grouped(&batch, group_size)?;
        } else {
            db.execute_query(&statements[index])?;
            index += 1;
        }
    }

    Ok(())
}

/// Assert that `result` is Ok and that the QueryResult contains exactly `expected` rows
pub fn assert_row_count(result: CoreResult<QueryResult>, expected: usize, context: &str) {
    match result {
        Ok(ref qr) => assert_eq!(
            qr.rows().len(),
            expected,
            "{}: expected {} rows, got {}",
            context,
            expected,
            qr.rows().len()
        ),
        Err(e) => panic!("{}: query failed: {:?}", context, e),
    }
}

/// Assert that a single-column count query returns the expected value
///
/// Executes `query` and reads the first row's first value as i64.
pub fn assert_count_eq(db: &mut TestDb, query: &str, expected: i64, context: &str) {
    match db.execute_query(query) {
        Ok(qr) => {
            let first = qr
                .rows()
                .first()
                .unwrap_or_else(|| panic!("{}: result set is empty", context));
            let val = first
                .first()
                .unwrap_or_else(|| panic!("{}: no column", context));
            let actual = match val {
                Value::BigInt(v) => *v,
                Value::Int(v) => *v as i64,
                Value::SmallInt(v) => *v as i64,
                other => panic!("{}: expected numeric value, got {:?}", context, other),
            };
            assert_eq!(
                actual, expected,
                "{}: expected count {}, got {}",
                context, expected, actual
            );
        }
        Err(e) => panic!("{}: query failed: {:?}", context, e),
    }
}

/// Assert that a query succeeds and returns exactly `expected` rows
pub fn assert_query_row_count(db: &mut TestDb, query: &str, expected: usize, context: &str) {
    match db.execute_query(query) {
        Ok(qr) => {
            let actual = qr.rows().len();
            assert_eq!(
                actual, expected,
                "{}: expected {} rows, got {}",
                context, expected, actual
            );
        }
        Err(e) => panic!("{}: query failed: {:?}", context, e),
    }
}

/// Assert that a single-value query returns the expected f64 value (within epsilon)
pub fn assert_float_eq(db: &mut TestDb, query: &str, expected: f64, context: &str) {
    match db.execute_query(query) {
        Ok(qr) => {
            let first = qr
                .rows()
                .first()
                .unwrap_or_else(|| panic!("{}: result set is empty", context));
            let val = first
                .first()
                .unwrap_or_else(|| panic!("{}: no column", context));
            let actual = match val {
                Value::Double(v) => *v,
                Value::Float(v) => *v as f64,
                other => panic!("{}: expected float, got {:?}", context, other),
            };
            assert!(
                (actual - expected).abs() < 1e-6,
                "{}: expected {}, got {}",
                context,
                expected,
                actual
            );
        }
        Err(e) => panic!("{}: query failed: {:?}", context, e),
    }
}
