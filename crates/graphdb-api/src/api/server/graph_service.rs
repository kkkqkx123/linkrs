use crate::api::core::{QueryApi, SyncApi};

#[cfg(feature = "qdrant")]
use crate::api::core::VectorApi;
use crate::api::server::auth::{Authenticator, AuthenticatorFactory, PasswordAuthenticator};
use crate::api::server::permission::PermissionManager;
use crate::api::server::session::{ClientSession, GraphSessionManager};
use crate::api::server::session::{SessionError, SessionResult};
use crate::config::Config;
use crate::core::metadata::SchemaManager;
use crate::core::stats::StatsManager;
use crate::core::types::SpaceSummary;
use crate::core::{DataType, MetricType, Permission};
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::query::executor::streaming::transaction_scope::CancelReason;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::executor::ExecutionResult;
use crate::query::optimizer::PartitioningConfig;
use crate::query::DataSet;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use crate::transaction::TransactionManager;
use log::{info, warn};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "qdrant")]
use vector_client::VectorManager;

pub struct GraphService<S: StorageClient + Clone + 'static> {
    session_manager: Arc<GraphSessionManager>,
    query_api: Arc<RwLock<QueryApi<S>>>,
    authenticator: PasswordAuthenticator,
    permission_manager: Arc<PermissionManager>,
    pub stats_manager: Arc<StatsManager>,
    storage: Arc<S>,
    #[cfg(feature = "qdrant")]
    vector_api: Option<Arc<VectorApi>>,
    sync_api: Option<Arc<SyncApi>>,

    // Transaction management-related
    transaction_manager: Option<Arc<TransactionManager>>,

    /// Engine-level shared scheduler, created once at startup.
    shared_scheduler: Arc<SharedScheduler>,
    /// Process-level query registry, created once at startup.
    query_registry: Arc<QueryRegistry>,

    /// Monotonically increasing query ID counter (server-assigned, not hash-based).
    next_query_id: AtomicU64,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    /// Create a new GraphService (without a transaction manager, for use in a production environment).
    pub async fn new(config: Config, storage: Arc<S>) -> Arc<Self> {
        #[cfg(feature = "qdrant")]
        return Self::create_service(config, storage, None, true, None, None).await;
        #[cfg(not(feature = "qdrant"))]
        return Self::create_service(config, storage, None, true, None).await;
    }

    /// Create a new GraphService (without a transaction manager and without starting any background tasks, for testing purposes).
    pub async fn new_for_test(config: Config, storage: Arc<S>) -> Arc<Self> {
        #[cfg(feature = "qdrant")]
        return Self::create_service(config, storage, None, false, None, None).await;
        #[cfg(not(feature = "qdrant"))]
        return Self::create_service(config, storage, None, false, None).await;
    }

    /// Use the transaction manager to create a GraphService.
    pub async fn new_with_transaction_manager(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
    ) -> Arc<Self> {
        #[cfg(feature = "qdrant")]
        return Self::create_service(config, storage, Some(transaction_manager), true, None, None)
            .await;
        #[cfg(not(feature = "qdrant"))]
        return Self::create_service(config, storage, Some(transaction_manager), true, None).await;
    }

    /// Use the transaction manager and external StatsManager to create a GraphService.
    pub async fn new_with_transaction_manager_and_stats(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
        stats_manager: Arc<StatsManager>,
    ) -> Arc<Self> {
        #[cfg(feature = "qdrant")]
        {
            Self::create_service(
                config,
                storage,
                Some(transaction_manager),
                true,
                Some(stats_manager),
                None,
            )
            .await
        }
        #[cfg(not(feature = "qdrant"))]
        {
            Self::create_service(
                config,
                storage,
                Some(transaction_manager),
                true,
                Some(stats_manager),
            )
            .await
        }
    }

    /// Use the transaction manager, external StatsManager, and shared VectorManager to create a GraphService.
    #[cfg(feature = "qdrant")]
    pub async fn with_shared_vector_manager(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
        stats_manager: Arc<StatsManager>,
        vector_manager: Arc<vector_client::VectorManager>,
    ) -> Arc<Self> {
        Self::create_service(
            config,
            storage,
            Some(transaction_manager),
            true,
            Some(stats_manager),
            Some(vector_manager),
        )
        .await
    }

    /// Internal constructor: Extracts the common logic
    ///
    /// # Parameters
    /// `start_cleanup_task` – Whether to initiate the background task for session cleanup
    /// `shared_vector_manager` – Optional shared VectorManager to avoid duplicate initialization
    async fn create_service(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Option<Arc<TransactionManager>>,
        start_cleanup_task: bool,
        external_stats_manager: Option<Arc<StatsManager>>,
        #[cfg(feature = "qdrant")] shared_vector_manager: Option<Arc<vector_client::VectorManager>>,
    ) -> Arc<Self> {
        // Columnar fast-path switches are process-level toggles in the query
        // executor; mirror the `[columnar]` config section onto them so the
        // server config (e.g. `column_block_enabled`) becomes the production
        // channel for the A1 scan path (default off).
        crate::query::executor::streaming::operators::source_operator::set_column_block_enabled(
            config.common.columnar.column_block_enabled,
        );

        let session_idle_timeout = Duration::from_secs(config.transaction.default_timeout * 10);
        let session_manager = GraphSessionManager::new(
            format!("{}:{}", config.database.host, config.database.port),
            config.database.max_connections,
            session_idle_timeout,
        );

        if start_cleanup_task {
            session_manager.start_cleanup_task().await;
        }

        let schema_manager: Option<Arc<SchemaManager>> = storage.get_schema_manager();

        // Create StatsManager with slow query logger FIRST (shared across all components)
        // Use external StatsManager if provided (e.g., from api/mod.rs for TransactionManager wiring)
        let stats_manager = if let Some(ext_stats) = external_stats_manager {
            ext_stats
        } else {
            let slow_query_config = config.to_slow_query_config();
            let m = &config.monitoring;
            Arc::new(
                StatsManager::with_slow_query_logger(
                    m.enabled,
                    m.memory_cache_size,
                    m.slow_query_threshold_ms * 1000,
                    slow_query_config,
                )
                .expect("Failed to create StatsManager with slow query logger"),
            )
        };

        // Engine-level shared scheduler + query registry, created once at
        // startup and reused across all queries (worker threads persist).
        let mut optimizer_engine = crate::query::OptimizerEngine::default();
        optimizer_engine.set_partitioning_config(Self::partitioning_config_from(&config));
        let shared_scheduler = Arc::new(SharedScheduler::new(
            optimizer_engine.partitioning_config().max_workers.max(1),
        ));
        let optimizer_engine = Arc::new(optimizer_engine);
        let query_registry = Arc::new(QueryRegistry::new());
        info!(
            "Shared query scheduler created with {} worker(s)",
            shared_scheduler.max_workers()
        );

        #[cfg(feature = "qdrant")]
        let (query_api, vector_api) = if config.is_vector_enabled() {
            // Use shared VectorManager if available, otherwise create a new one
            let vm = match shared_vector_manager {
                Some(vm) => vm,
                None => Arc::new(
                    VectorManager::new(config.vector_config().clone())
                        .await
                        .unwrap_or_else(|_| panic!("Failed to create vector manager")),
                ),
            };

            match QueryApi::with_vector_manager(
                Arc::new(RwLock::new((*storage).clone())),
                stats_manager.clone(),
                vm.clone(),
                schema_manager.clone(),
            )
            .await
            {
                Ok(mut api) => {
                    api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
                    let vector_api = Arc::new(VectorApi::new(vm));
                    (Arc::new(RwLock::new(api)), Some(vector_api))
                }
                Err(e) => {
                    warn!(
                        "Failed to initialize vector search, falling back to basic QueryApi: {}",
                        e
                    );
                    let mut api = Self::build_query_api(
                        &storage,
                        &stats_manager,
                        schema_manager.as_ref(),
                        optimizer_engine.clone(),
                    );
                    api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
                    (Arc::new(RwLock::new(api)), None)
                }
            }
        } else {
            let mut api = Self::build_query_api(
                &storage,
                &stats_manager,
                schema_manager.as_ref(),
                optimizer_engine.clone(),
            );
            api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
            (Arc::new(RwLock::new(api)), None)
        };

        #[cfg(not(feature = "qdrant"))]
        let query_api = {
            let mut api = Self::build_query_api(
                &storage,
                &stats_manager,
                schema_manager.as_ref(),
                optimizer_engine.clone(),
            );
            api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
            Arc::new(RwLock::new(api))
        };

        // Startup statistics load: collect optimizer statistics for every
        // loaded space in the background. Failures are logged as warnings
        // and never block server startup.
        if start_cleanup_task {
            Self::spawn_startup_statistics_load(query_api.clone(), storage.clone());
        }

        let authenticator = AuthenticatorFactory::create_default(&config.server.auth);
        let permission_manager = Arc::new(PermissionManager::new());

        // Create sync API if storage supports it
        let sync_api = storage
            .get_sync_manager()
            .map(|sync_manager| Arc::new(SyncApi::new(sync_manager)));

        let service = Self {
            session_manager,
            query_api,
            authenticator,
            permission_manager,
            stats_manager,
            storage,
            #[cfg(feature = "qdrant")]
            vector_api,
            sync_api,
            transaction_manager,
            shared_scheduler,
            query_registry,
            next_query_id: AtomicU64::new(1),
        };
        Arc::new(service)
    }

    /// Shared helper: build a QueryApi with optional SchemaManager, reusing the
    /// server-level optimizer engine so `[parallel]` settings take effect.
    fn build_query_api(
        storage: &Arc<S>,
        stats_manager: &Arc<StatsManager>,
        schema_manager: Option<&Arc<SchemaManager>>,
        optimizer_engine: Arc<crate::query::OptimizerEngine>,
    ) -> QueryApi<S> {
        let inner = Arc::new(RwLock::new((**storage).clone()));
        QueryApi::with_optimizer_engine(
            inner,
            stats_manager.clone(),
            optimizer_engine,
            schema_manager.cloned(),
        )
    }

    /// Map the `[parallel]` config section onto the query optimizer's
    /// partitioning configuration. An unconfigured section keeps the default
    /// (partitioning disabled, single worker) so server behavior is unchanged.
    fn partitioning_config_from(config: &Config) -> PartitioningConfig {
        let parallel = &config.common.parallel;
        PartitioningConfig {
            enabled: parallel.enabled,
            min_rows_per_partition: parallel.min_rows_per_partition,
            max_partitions: parallel.max_partitions,
            vertex_id_range: parallel.vertex_id_range(),
            max_workers: parallel.workers,
            max_buffered_chunks: parallel.max_buffered_chunks,
        }
    }

    /// Spawn a background task that collects optimizer statistics for all
    /// loaded spaces. Failure of any space is a warning only.
    fn spawn_startup_statistics_load(query_api: Arc<RwLock<QueryApi<S>>>, storage: Arc<S>) {
        tokio::spawn(async move {
            let spaces = match storage.list_spaces() {
                Ok(spaces) => spaces,
                Err(error) => {
                    warn!("Startup statistics load: failed to list spaces: {}", error);
                    return;
                }
            };
            for space in spaces {
                let result = query_api
                    .read()
                    .collect_statistics(&space.space_name, false);
                match result {
                    Ok(()) => info!("Startup statistics loaded for space '{}'", space.space_name),
                    Err(error) => warn!(
                        "Startup statistics load failed for space '{}': {}",
                        space.space_name, error
                    ),
                }
            }
        });
    }

    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Arc<ClientSession>, String> {
        if username.is_empty() || password.is_empty() {
            self.stats_manager
                .add_value(MetricType::NumAuthFailedSessions);
            return Err("User name or password cannot be empty".to_string());
        }

        if self.session_manager.is_out_of_connections().await {
            self.stats_manager
                .add_value(MetricType::NumAuthFailedSessions);
            return Err("More than the maximum number of connections limit".to_string());
        }

        match self.authenticator.authenticate(username, password) {
            Ok(_) => {
                let session = self
                    .session_manager
                    .create_session(username.to_string(), "127.0.0.1".to_string())
                    .await
                    .map_err(|e| format!("Creating a session failed: {}", e))?;

                Ok(session)
            }
            Err(e) => {
                self.stats_manager
                    .add_value(MetricType::NumAuthFailedSessions);
                Err(format!("authentication failure: {}", e))
            }
        }
    }

    pub async fn execute(&self, session_id: i64, stmt: &str) -> Result<ExecutionResult, String> {
        self.execute_with_params(session_id, stmt, None, None).await
    }

    /// Execute a query with client-supplied query parameters (`@name`
    /// references) and/or session variables (`$name` references).
    ///
    /// `session_variables` supplied here replace the session-managed snapshot
    /// (set via `LET $name = expr`) for this statement; pass `None` to use the
    /// snapshot. Parameters are bound to `@name` references only and are fully
    /// decoupled from session variables.
    pub async fn execute_with_params(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, crate::core::Value>>,
        session_variables: Option<HashMap<String, crate::core::Value>>,
    ) -> Result<ExecutionResult, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;

        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);

        // Cleanup expired transactions before processing any statement.
        // This prevents stale transactions from blocking new write operations.
        if let Some(ref txn_manager) = self.transaction_manager {
            txn_manager.cleanup_expired_transactions();
        }

        // Handle transaction control statements
        let trimmed_stmt = stmt.trim().to_uppercase();
        if trimmed_stmt.starts_with("BEGIN") || trimmed_stmt.starts_with("START TRANSACTION") {
            return self.handle_begin_transaction(&session, stmt);
        } else if trimmed_stmt.starts_with("COMMIT") {
            return self.handle_commit_transaction(&session).await;
        } else if trimmed_stmt.starts_with("ROLLBACK") {
            return self.handle_rollback_transaction(&session, stmt);
        } else if trimmed_stmt.starts_with("SAVEPOINT") {
            return self.handle_savepoint(&session, stmt);
        } else if trimmed_stmt.starts_with("RELEASE SAVEPOINT") {
            return self.handle_release_savepoint(&session, stmt);
        } else if trimmed_stmt.starts_with("LET ") {
            return self.handle_let_statement(&session, stmt);
        }

        // Perform a regular query using core layer QueryApi
        let mut result = self.execute_query_with_permission(
            session_id,
            stmt,
            space_id,
            parameters,
            session_variables,
        );

        // Handle SpaceSwitched result from USE statement
        // The core QueryApi converts SpaceSwitched to a DataSet with space_name/space_id columns,
        // so we need to extract space info from the DataSet for USE statements.
        if stmt.trim().to_uppercase().starts_with("USE ") {
            if let Ok(ref exec_result) = result {
                if let Some(space_summary) = Self::extract_space_summary_from_result(exec_result) {
                    session.set_space(space_summary);
                }
            }
        }

        // Automatic submission mode processing
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
                            result = Err(format!("Auto-commit failed: {}", e));
                        }
                    }
                }
            }
        }

        result
    }

    /// Execute a query and return a [`StreamingQueryResult`] for chunk-at-a-time consumption.
    ///
    /// Similar to [`execute`] but returns a streaming handle instead of a materialized result.
    pub async fn execute_stream(
        &self,
        session_id: i64,
        stmt: &str,
    ) -> Result<StreamingQueryResult, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;

        // Handle transaction control statements
        let trimmed_stmt = stmt.trim().to_uppercase();
        if trimmed_stmt.starts_with("BEGIN")
            || trimmed_stmt.starts_with("START TRANSACTION")
            || trimmed_stmt.starts_with("COMMIT")
            || trimmed_stmt.starts_with("ROLLBACK")
            || trimmed_stmt.starts_with("SAVEPOINT")
            || trimmed_stmt.starts_with("RELEASE SAVEPOINT")
            || trimmed_stmt.starts_with("LET ")
        {
            return self
                .execute(session_id, stmt)
                .await
                .map(StreamingQueryResult::from_execution_result);
        }

        // Assign a server-side monotonic query ID up front so the request-scoped
        // id is threaded through QueryRequestContext → ExecutionContext → runtime.
        let query_id = self.next_query_id.fetch_add(1, Ordering::Relaxed) as u32;
        let query_request = crate::api::core::QueryRequest {
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: session.current_transaction(),
            parameters: None,
            session_variables: Some(session.variables_snapshot()),
            query_id: Some(query_id as u64),
        };

        let mut query_api = self.query_api.write();
        let result = if let Some(txn_id) = session.current_transaction() {
            let manager = self
                .transaction_manager
                .as_ref()
                .ok_or_else(|| "Transaction manager is not configured".to_string())?;
            manager
                .refresh_statement_snapshot(txn_id)
                .map_err(|error| error.to_string())?;
            let execution = manager
                .create_execution(txn_id, false)
                .map_err(|error| error.to_string())?;
            query_api
                .execute_stream_with_execution(stmt, query_request, &execution)
                .map_err(|e| e.to_string())?
        } else {
            let execution_storage = self
                .storage
                .bind_auto_commit_context()
                .map_err(|error| error.to_string())?;
            query_api
                .execute_stream_with_operation_storage(stmt, query_request, execution_storage)
                .map_err(|e| e.to_string())?
        };

        // Assign a server-side monotonic query ID (not from SQL text hash).
        result.runtime().assign_query_id(query_id as u64);
        session.register_streaming_query(query_id, stmt.to_string(), result.runtime_downgrade());

        // Auto-deregister on Drop (covers completion, error, and disconnect).
        let session_clone = session.clone();
        result.set_on_drop(Box::new(move || {
            session_clone.unregister_streaming_query(query_id);
        }));

        Ok(result)
    }

    fn execute_query_with_permission(
        &self,
        session_id: i64,
        stmt: &str,
        space_id: i64,
        parameters: Option<HashMap<String, crate::core::Value>>,
        session_variables: Option<HashMap<String, crate::core::Value>>,
    ) -> Result<ExecutionResult, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;

        session.charge();
        let username = session.user();

        // Permission check: The admin has all permissions, so no check is required.
        // USE is a session-level operation that does not access data — skip permission
        // check so any authenticated user can switch to a space.
        if !self.permission_manager.is_admin(&username)
            && !stmt.trim().to_uppercase().starts_with("USE ")
        {
            let permission = self.extract_permission_from_statement(stmt);
            if let Err(e) = self
                .permission_manager
                .check_permission(&username, space_id, permission)
            {
                return Err(format!("Permission check failed: {}", e));
            }
        }

        // Resolve the immutable operation context for this query.
        let mut statement_guard = None;
        let txn_context = if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                match txn_manager.begin_statement(txn_id) {
                    Ok((ctx, statement_start)) => {
                        statement_guard = Some((txn_manager.clone(), ctx.clone(), statement_start));
                        Some(ctx)
                    }
                    Err(e) => {
                        if e.is_timeout() {
                            warn!(
                                "Transaction {} exceeded a timeout before statement execution",
                                txn_id
                            );
                        }
                        return Err(e.to_string());
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        // Use core layer QueryApi to execute query. Session variables are
        // injected through the dedicated session_variables channel (captured
        // once per statement), fully decoupled from query parameters.
        // Client-supplied session variables replace the session snapshot;
        // otherwise the snapshot (set via `LET $name = expr`) is used.
        let query_request = crate::api::core::QueryRequest {
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: session.current_transaction(),
            parameters,
            session_variables: session_variables.or_else(|| Some(session.variables_snapshot())),
            query_id: None,
        };

        let mut query_api = self.query_api.write();
        let mut result = if let Some(context) = txn_context.as_ref() {
            let manager = self
                .transaction_manager
                .as_ref()
                .ok_or_else(|| "Transaction manager is not configured".to_string())?;
            let execution = manager
                .create_execution(context.id, false)
                .map_err(|error| error.to_string())?;
            query_api
                .execute_with_execution(stmt, query_request, &execution)
                .map_err(|e| e.to_string())
        } else {
            let execution_storage = self
                .storage
                .bind_auto_commit_context()
                .map_err(|error| error.to_string())?;
            query_api
                .execute_with_operation_storage(stmt, query_request, execution_storage)
                .map_err(|e| e.to_string())
        };

        if let Some((txn_manager, context, statement_start)) = statement_guard {
            if let Err(error) = txn_manager.finish_statement(&context, statement_start) {
                result = Err(error.to_string());
            }
        }

        // If the query failed and we have an active transaction, check if the
        // transaction is still in a valid state. If the transaction has become
        // invalid (e.g. due to a storage error), clean it up.
        if result.is_err() {
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
                        }
                    }
                }
            }
        }

        match result {
            Ok(query_result) => Ok(Self::convert_to_execution_result(query_result)),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Convert core QueryResult to query ExecutionResult
    fn convert_to_execution_result(result: crate::api::core::QueryResult) -> ExecutionResult {
        if result.rows.is_empty() {
            return ExecutionResult::Empty;
        }

        // General case: return DataSet
        let rows: Vec<Vec<crate::core::Value>> = result
            .rows
            .into_iter()
            .map(|row| {
                result
                    .columns
                    .iter()
                    .filter_map(|col| row.get(col).cloned())
                    .collect()
            })
            .collect();

        ExecutionResult::DataSet {
            data: DataSet {
                col_names: result.columns,
                rows,
            },
        }
    }

    fn extract_permission_from_statement(&self, stmt: &str) -> Permission {
        let stmt_upper = stmt.trim().to_uppercase();

        if stmt_upper.starts_with("SELECT") || stmt_upper.starts_with("MATCH") {
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

    /// Parse a DataType from its Display string representation.
    /// Mirrors the Display impl in graphdb_core::core::types::mod.rs.
    fn parse_data_type(s: &str) -> DataType {
        match s.to_uppercase().as_str() {
            "EMPTY" => DataType::Empty,
            "NULL" => DataType::Null,
            "BOOL" => DataType::Bool,
            "SMALLINT" => DataType::SmallInt,
            "INT" => DataType::Int,
            "BIGINT" => DataType::BigInt,
            "FLOAT" => DataType::Float,
            "DOUBLE" => DataType::Double,
            "DECIMAL128" => DataType::Decimal128,
            "STRING" => DataType::String,
            "DATE" => DataType::Date,
            "TIME" => DataType::Time,
            "DATETIME" => DataType::DateTime,
            "VERTEX" => DataType::Vertex,
            "EDGE" => DataType::Edge,
            "PATH" => DataType::Path,
            "LIST" => DataType::List,
            "MAP" => DataType::Map,
            "SET" => DataType::Set,
            "GEOGRAPHY" => DataType::Geography,
            "DATASET" => DataType::DataSet,
            "VID" => DataType::VID,
            "BLOB" => DataType::Blob,
            "TIMESTAMP" => DataType::Timestamp,
            "VECTOR" => DataType::Vector,
            "JSON" => DataType::Json,
            "JSONB" => DataType::JsonB,
            "UUID" => DataType::Uuid,
            "INTERVAL" => DataType::Interval,
            _ if s.starts_with("FIXEDSTRING(") => {
                let n = s
                    .trim_start_matches("FIXEDSTRING(")
                    .trim_end_matches(')')
                    .parse::<usize>()
                    .unwrap_or(0);
                DataType::FixedString(n)
            }
            _ if s.starts_with("VECTOR_DENSE(") => {
                let n = s
                    .trim_start_matches("VECTOR_DENSE(")
                    .trim_end_matches(')')
                    .parse::<usize>()
                    .unwrap_or(0);
                DataType::VectorDense(n)
            }
            _ if s.starts_with("VECTOR_SPARSE(") => {
                let n = s
                    .trim_start_matches("VECTOR_SPARSE(")
                    .trim_end_matches(')')
                    .parse::<usize>()
                    .unwrap_or(0);
                DataType::VectorSparse(n)
            }
            _ => DataType::BigInt,
        }
    }

    /// Extract SpaceSummary from an ExecutionResult DataSet that contains space info.
    /// This is used for USE statement results that have been converted from SpaceSwitched.
    fn extract_space_summary_from_result(result: &ExecutionResult) -> Option<SpaceSummary> {
        match result {
            ExecutionResult::DataSet { data: ds, .. } => {
                let name_idx = ds.col_names.iter().position(|c| c == "space_name")?;
                let id_idx = ds.col_names.iter().position(|c| c == "space_id")?;
                let vid_type_idx = ds.col_names.iter().position(|c| c == "vid_type");
                let row = ds.rows.first()?;
                let name = match row.get(name_idx)? {
                    crate::core::Value::String(s) => s.to_string(),
                    _ => return None,
                };
                let id = match row.get(id_idx)? {
                    crate::core::Value::BigInt(id) => *id as u64,
                    _ => return None,
                };
                let vid_type = match vid_type_idx.and_then(|idx| row.get(idx)) {
                    Some(crate::core::Value::String(s)) => Self::parse_data_type(s),
                    _ => DataType::BigInt,
                };
                Some(SpaceSummary::new(id, name, vid_type))
            }
            _ => result.space_summary().cloned(),
        }
    }

    /// Graceful shutdown of the shared execution infrastructure.
    ///
    /// Order matters: `cancel_all` must complete before the scheduler is
    /// shut down so no query can submit work during teardown.
    pub fn shutdown(&self) {
        let cancelled = self.query_registry.cancel_all(CancelReason::Shutdown);
        if !cancelled.is_empty() {
            info!(
                "Cancelled {} active query(s) during shutdown",
                cancelled.len()
            );
        }
        self.shared_scheduler.shutdown_shared();
        info!("Shared query scheduler shut down");
    }

    pub async fn signout(&self, session_id: i64) {
        if let Some(session) = self.session_manager.find_session(session_id) {
            if let Some(space_name) = session.space_name() {
                self.stats_manager
                    .dec_space_metric(&space_name, MetricType::NumActiveQueries);
            }
        }
        self.session_manager.remove_session(session_id).await;
    }

    pub fn get_session_manager(&self) -> &Arc<GraphSessionManager> {
        &self.session_manager
    }

    pub fn get_permission_manager(&self) -> &Arc<PermissionManager> {
        &self.permission_manager
    }

    pub fn get_stats_manager(&self) -> &Arc<StatsManager> {
        &self.stats_manager
    }

    #[cfg(feature = "qdrant")]
    pub fn vector_api(&self) -> Option<&Arc<VectorApi>> {
        self.vector_api.as_ref()
    }

    pub fn sync_api(&self) -> Option<&Arc<SyncApi>> {
        self.sync_api.as_ref()
    }

    /// Obtain the session list (SHOW SESSIONS)
    pub async fn list_sessions(&self) -> Vec<crate::api::server::session::SessionInfo> {
        self.session_manager.list_sessions().await
    }

    /// Obtain detailed information about the specified session.
    pub async fn get_session_info(
        &self,
        session_id: i64,
    ) -> Option<crate::api::server::session::SessionInfo> {
        self.session_manager.get_session_info(session_id).await
    }

    /// Terminate the session (KILL SESSION)
    pub async fn kill_session(&self, session_id: i64, current_user: &str) -> SessionResult<()> {
        // Obtain the current session in order to check permissions.
        let current_session = self
            .session_manager
            .find_session(session_id)
            .ok_or(SessionError::session_not_found(session_id))?;

        let is_admin = current_session.is_admin();

        self.session_manager
            .kill_session(session_id, current_user, is_admin)
            .await
    }

    /// Terminate the query (KILL QUERY)
    pub fn kill_query(&self, session_id: i64, query_id: u32) -> SessionResult<()> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or(SessionError::session_not_found(session_id))?;

        match session.kill_query(query_id) {
            Ok(()) => {
                self.stats_manager.dec_value(MetricType::NumActiveQueries);
                Ok(())
            }
            Err(e) => Err(SessionError::manager_error(e.to_string())),
        }
    }

    // ==================== Transaction Control Methods ====================

    /// Validate that session's transaction binding is consistent with transaction manager state.
    /// Returns Ok(()) if session has no transaction or the transaction is valid.
    /// Returns Err if session has a stale transaction binding that was cleaned up.
    fn validate_session_transaction_state(
        &self,
        session: &Arc<ClientSession>,
    ) -> Result<(), String> {
        if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                if !txn_manager.is_transaction_active(txn_id) {
                    warn!(
                        "Session {} has stale transaction binding to {}, cleaning up",
                        session.id(),
                        txn_id
                    );
                    session.unbind_transaction();
                    return Err(format!(
                        "Transaction {} is no longer active, please retry the operation",
                        txn_id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Parse the transaction access mode from a `BEGIN [TRANSACTION] [READ
    /// ONLY | READ WRITE]` statement.
    ///
    /// Returns `Ok(Some(true))` for READ ONLY, `Ok(Some(false))` for READ
    /// WRITE and `Ok(None)` when no access mode is specified. Malformed
    /// suffixes (e.g. `BEGIN READ`) are rejected.
    fn parse_begin_access_mode(stmt: &str) -> Result<Option<bool>, String> {
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
        Err("Invalid BEGIN access mode, expected READ ONLY or READ WRITE".to_string())
    }

    /// Processing the BEGIN TRANSACTION statement
    fn handle_begin_transaction(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
    ) -> Result<ExecutionResult, String> {
        self.validate_session_transaction_state(session)?;

        if session.has_active_transaction() {
            return Err("Session already has an active transaction".to_string());
        }

        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        // READ ONLY transactions start a consistent MVCC snapshot read
        // (begin_read_transaction -> acquire_read_timestamp); READ WRITE /
        // unspecified transactions proceed with the session defaults.
        let access_mode = Self::parse_begin_access_mode(stmt)?;
        let mut options = session.transaction_options();
        if let Some(read_only) = access_mode {
            options.read_only = read_only;
        }
        match txn_manager.begin_transaction_with_owner(options, session.id().to_string()) {
            Ok(txn_id) => {
                session.bind_transaction(txn_id);
                session.set_auto_commit(false);
                info!(
                    "Session {} started {} transaction {}",
                    session.id(),
                    if access_mode == Some(true) {
                        "read-only"
                    } else {
                        "read-write"
                    },
                    txn_id
                );
                Ok(ExecutionResult::Success)
            }
            Err(e) => {
                // If the error is a write conflict, try cleaning up expired transactions
                // and retry once. This handles the case where a stale transaction
                // is blocking new write transactions.
                if matches!(
                    e.kind(),
                    crate::transaction::TransactionErrorKind::WriteTransactionConflict
                ) {
                    txn_manager.cleanup_expired_transactions();
                    let options = session.transaction_options();
                    match txn_manager
                        .begin_transaction_with_owner(options, session.id().to_string())
                    {
                        Ok(txn_id) => {
                            session.bind_transaction(txn_id);
                            session.set_auto_commit(false);
                            info!(
                                "Session {} started transaction {} after cleanup retry",
                                session.id(),
                                txn_id
                            );
                            return Ok(ExecutionResult::Success);
                        }
                        Err(retry_err) => {
                            return Err(format!("Failed to start transaction: {}", retry_err));
                        }
                    }
                }
                Err(format!("Failed to start transaction: {}", e))
            }
        }
    }

    /// Processing the COMMIT statement
    async fn handle_commit_transaction(
        &self,
        session: &Arc<ClientSession>,
    ) -> Result<ExecutionResult, String> {
        self.validate_session_transaction_state(session)?;

        let txn_id = session
            .current_transaction()
            .ok_or("No active transaction to commit")?;

        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        match txn_manager.commit_transaction(txn_id) {
            Ok(()) => {
                session.unbind_transaction();
                session.set_auto_commit(true);
                session.commit_variables();
                info!("Session {} committed transaction {}", session.id(), txn_id);
                Ok(ExecutionResult::Success)
            }
            Err(e) => Err(format!("Failed to commit transaction: {}", e)),
        }
    }

    /// Processing the ROLLBACK statement
    fn handle_rollback_transaction(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
    ) -> Result<ExecutionResult, String> {
        self.validate_session_transaction_state(session)?;

        let trimmed = stmt.trim().to_uppercase();

        // Check whether it is a command to perform a ROLLBACK TO SAVEPOINT.
        if trimmed.starts_with("ROLLBACK TO ") {
            // Extract the savepoint name from the ORIGINAL statement to
            // preserve case (the transaction layer matches names verbatim).
            let original = stmt.trim();
            let savepoint_name = original
                .get("ROLLBACK TO ".len()..)
                .map(|s| s.trim())
                .ok_or("Invalid ROLLBACK TO syntax")?;

            let txn_id = session
                .current_transaction()
                .ok_or("No active transaction to rollback")?;

            let txn_manager = self
                .transaction_manager
                .as_ref()
                .ok_or("Transaction manager not initialized")?;

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
            Ok(ExecutionResult::Success)
        } else {
            // Full transaction rollback
            let txn_id = session
                .current_transaction()
                .ok_or("No active transaction to rollback")?;

            let txn_manager = self
                .transaction_manager
                .as_ref()
                .ok_or("Transaction manager not initialized")?;

            match txn_manager.abort_transaction(txn_id) {
                Ok(()) => {
                    session.unbind_transaction();
                    session.set_auto_commit(true);
                    session.rollback_variables();
                    info!(
                        "Session {} rolled back transaction {}",
                        session.id(),
                        txn_id
                    );
                    Ok(ExecutionResult::Success)
                }
                Err(e) => Err(format!("Failed to rollback transaction: {}", e)),
            }
        }
    }

    /// Processing the SAVEPOINT statement
    fn handle_savepoint(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
    ) -> Result<ExecutionResult, String> {
        let savepoint_name = stmt["SAVEPOINT".len()..].trim();

        if savepoint_name.is_empty() {
            return Err("Savepoint name cannot be empty".to_string());
        }

        let txn_id = session
            .current_transaction()
            .ok_or("No active transaction, cannot create savepoint")?;

        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        let savepoint_id = txn_manager
            .create_savepoint(txn_id, Some(savepoint_name.to_string()))
            .map_err(|e| format!("Failed to create savepoint: {}", e))?;

        session.push_variable_savepoint(savepoint_name);

        info!(
            "Session {} created savepoint {} in transaction {} (ID: {})",
            session.id(),
            savepoint_name,
            txn_id,
            savepoint_id
        );

        Ok(ExecutionResult::Success)
    }

    /// Processing the RELEASE SAVEPOINT statement
    fn handle_release_savepoint(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
    ) -> Result<ExecutionResult, String> {
        let savepoint_name = stmt["RELEASE SAVEPOINT".len()..].trim();

        if savepoint_name.is_empty() {
            return Err("Savepoint name cannot be empty".to_string());
        }

        let txn_id = session
            .current_transaction()
            .ok_or("No active transaction, cannot release savepoint")?;

        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        let context = txn_manager
            .get_context(txn_id)
            .map_err(|e| format!("Failed to get transaction context: {}", e))?;
        let savepoint_info = context
            .find_savepoint_by_name(savepoint_name)
            .ok_or_else(|| format!("Savepoint '{}' does not exist", savepoint_name))?;

        if let Err(e) = txn_manager.release_savepoint(txn_id, savepoint_info.id) {
            return Err(format!("Failed to release savepoint: {}", e));
        }

        info!(
            "Session {} released savepoint {} in transaction {}",
            session.id(),
            savepoint_name,
            txn_id
        );

        session.release_variable_savepoint(savepoint_name);

        Ok(ExecutionResult::Success)
    }

    /// Processing the `LET $name = expr` session-variable assignment.
    ///
    /// The right-hand side is evaluated through the query engine as
    /// `RETURN <expr>` (session variables are injected as parameters, so an
    /// expression may reference earlier assignments). The first value of the
    /// first row is stored in the session; inside an explicit transaction the
    /// assignment is recorded on the variable overlay so ROLLBACK /
    /// ROLLBACK TO SAVEPOINT restore the previous value.
    fn handle_let_statement(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
    ) -> Result<ExecutionResult, String> {
        let (name, expr) = Self::parse_let_statement(stmt)?;
        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let evaluate_stmt = format!("RETURN ({})", expr);
        let result =
            self.execute_query_with_permission(session.id(), &evaluate_stmt, space_id, None, None)?;
        let value = match result {
            ExecutionResult::DataSet { data, .. } => data
                .rows
                .into_iter()
                .next()
                .and_then(|row| row.into_iter().next())
                .ok_or_else(|| format!("LET expression '{}' returned no value", expr))?,
            _ => {
                return Err(format!("LET expression '{}' could not be evaluated", expr));
            }
        };
        session.set_variable(name, value);
        info!("Session {} set session variable", session.id());
        Ok(ExecutionResult::Success)
    }

    /// Parse `LET $name = expr` into the variable name and expression text.
    ///
    /// Both `LET $name = expr` and `LET name = expr` are accepted. The name
    /// must be a valid identifier (`[A-Za-z_][A-Za-z0-9_]*`).
    fn parse_let_statement(stmt: &str) -> Result<(String, String), String> {
        let rest = stmt.trim_start();
        let rest = &rest["LET".len()..];
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('$').unwrap_or(rest);
        let eq_pos = rest
            .find('=')
            .ok_or("LET requires an assignment: LET $name = expr")?;
        let name = rest[..eq_pos].trim();
        let expr = rest[eq_pos + 1..].trim();
        let valid = !name.is_empty()
            && name
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("Invalid session variable name '{}'", name));
        }
        if expr.is_empty() {
            return Err(format!("LET {} requires an expression", name));
        }
        Ok((name.to_string(), expr.to_string()))
    }
}

impl<S> GraphService<S>
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + crate::storage::AutoCommitBatchOps
        + 'static,
{
    /// Execute a batch of auto-commit DML statements inside one shared
    /// auto-commit batch window (P4/P6): the write gate is acquired and MVCC
    /// snapshots registered once for the whole batch instead of once per
    /// statement. Each statement is permission-checked individually and runs
    /// independently (own timestamp / transaction id / undo log); a failure
    /// rolls back only its own statement and does not abort the rest of the
    /// batch.
    ///
    /// Inside an explicit transaction (or a non-auto-commit session) execution
    /// falls back to per-statement [`execute`](Self::execute), preserving
    /// transaction semantics. Returns one outcome per input statement, in
    /// order.
    pub async fn execute_batch(
        &self,
        session_id: i64,
        statements: &[String],
    ) -> Vec<Result<ExecutionResult, String>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| Err(format!("Invalid session ID: {session_id}")))
                .collect();
        };
        session.charge();
        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();

        // The batch window is for auto-commit DML only. Inside an explicit
        // transaction (or a non-auto-commit session) fall back to
        // per-statement execution so transaction semantics are preserved.
        if session.current_transaction().is_some() || !session.is_auto_commit() {
            let mut results = Vec::with_capacity(statements.len());
            for stmt in statements {
                results.push(self.execute(session_id, stmt).await);
            }
            return results;
        }

        // Permission-check first so denied statements are never executed.
        let mut denied: Vec<(usize, String)> = Vec::new();
        let mut permitted: Vec<String> = Vec::with_capacity(statements.len());
        for (index, stmt) in statements.iter().enumerate() {
            if !self.permission_manager.is_admin(&username) {
                let permission = self.extract_permission_from_statement(stmt);
                if let Err(e) = self
                    .permission_manager
                    .check_permission(&username, space_id, permission)
                {
                    denied.push((index, format!("Permission check failed: {}", e)));
                    continue;
                }
            }
            permitted.push(stmt.clone());
        }

        let query_request = crate::api::core::QueryRequest {
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
        };
        let outcomes = self
            .query_api
            .write()
            .execute_batch(&permitted, query_request);

        let mut results = Vec::with_capacity(statements.len());
        let mut permitted_outcomes = outcomes.into_iter();
        for index in 0..statements.len() {
            if let Some((_, error)) = denied.iter().find(|(i, _)| *i == index) {
                results.push(Err(error.clone()));
                continue;
            }
            match permitted_outcomes.next() {
                Some(Ok(result)) => results.push(Ok(Self::convert_to_execution_result(result))),
                Some(Err(error)) => results.push(Err(error.to_string())),
                None => results.push(Err("Batch outcome missing".to_string())),
            }
        }
        results
    }
}

impl<S> GraphService<S>
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + crate::storage::AutoCommitBatchOps
        + crate::storage::AutoCommitGroupOps
        + 'static,
{
    /// Execute a batch of auto-commit DML statements inside shared
    /// group-commit windows (P0 C): each consecutive group of `group_size`
    /// statements shares one write timestamp, one WAL fsync, and one commit
    /// point. See [`QueryApi::execute_batch_grouped`].
    ///
    /// Same fallback / permission semantics as
    /// [`execute_batch`](Self::execute_batch): inside an explicit transaction
    /// execution falls back to per-statement [`execute`](Self::execute), and
    /// each statement is permission-checked individually.
    pub async fn execute_batch_grouped(
        &self,
        session_id: i64,
        statements: &[String],
        group_size: usize,
    ) -> Vec<Result<ExecutionResult, String>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| Err(format!("Invalid session ID: {session_id}")))
                .collect();
        };
        session.charge();
        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();

        if session.current_transaction().is_some() || !session.is_auto_commit() {
            let mut results = Vec::with_capacity(statements.len());
            for stmt in statements {
                results.push(self.execute(session_id, stmt).await);
            }
            return results;
        }

        let mut denied: Vec<(usize, String)> = Vec::new();
        let mut permitted: Vec<String> = Vec::with_capacity(statements.len());
        for (index, stmt) in statements.iter().enumerate() {
            if !self.permission_manager.is_admin(&username) {
                let permission = self.extract_permission_from_statement(stmt);
                if let Err(e) = self
                    .permission_manager
                    .check_permission(&username, space_id, permission)
                {
                    denied.push((index, format!("Permission check failed: {}", e)));
                    continue;
                }
            }
            permitted.push(stmt.clone());
        }

        let query_request = crate::api::core::QueryRequest {
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
        };
        let outcomes =
            self.query_api
                .write()
                .execute_batch_grouped(&permitted, query_request, group_size);

        let mut results = Vec::with_capacity(statements.len());
        let mut permitted_outcomes = outcomes.into_iter();
        for index in 0..statements.len() {
            if let Some((_, error)) = denied.iter().find(|(i, _)| *i == index) {
                results.push(Err(error.clone()));
                continue;
            }
            match permitted_outcomes.next() {
                Some(Ok(result)) => results.push(Ok(Self::convert_to_execution_result(result))),
                Some(Err(error)) => results.push(Err(error.to_string())),
                None => results.push(Err("Batch outcome missing".to_string())),
            }
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::GraphService;
    use crate::storage::MockStorage;

    #[test]
    fn parse_begin_access_mode_variants() {
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN"),
            Ok(None)
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN TRANSACTION"),
            Ok(None)
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN READ ONLY"),
            Ok(Some(true))
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN READ WRITE"),
            Ok(Some(false))
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN TRANSACTION READ ONLY"),
            Ok(Some(true))
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("START TRANSACTION READ WRITE"),
            Ok(Some(false))
        );
        assert_eq!(
            GraphService::<MockStorage>::parse_begin_access_mode("begin read only"),
            Ok(Some(true))
        );
        assert!(GraphService::<MockStorage>::parse_begin_access_mode("BEGIN READ").is_err());
        assert!(
            GraphService::<MockStorage>::parse_begin_access_mode("BEGIN TRANSACTION READ").is_err()
        );
    }
}
