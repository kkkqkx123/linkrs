use graphdb_api::api_core::{QueryApi, QueryResult, SyncApi};

use crate::auth::{Authenticator, AuthenticatorFactory, PasswordAuthenticator};
use crate::config::Config;
use crate::permission::PermissionManager;
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::query::executor::streaming::transaction_scope::CancelReason;
use crate::query::executor::streaming::StreamingQueryResult;
use crate::query::optimizer::PartitioningConfig;
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::Stmt;
use crate::query::parser::{Parser, ParserResult};
use crate::session::{ClientSession, GraphSessionManager};
use crate::session::{SessionError, SessionResult};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
#[cfg(feature = "vector")]
use graphdb_api::api_core::VectorApi;
use graphdb_core::metadata::SchemaManager;
use graphdb_core::stats::StatsManager;
use graphdb_core::types::SpaceSummary;
use graphdb_core::{MetricType, Permission};
#[cfg(feature = "vector")]
use graphdb_sync::backend::VectorBackend;
use graphdb_transaction::{TransactionId, TransactionManager};
use log::{info, warn};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct GraphService<S: StorageClient + Clone + 'static> {
    session_manager: Arc<GraphSessionManager>,
    query_api: Arc<RwLock<QueryApi<S>>>,
    authenticator: PasswordAuthenticator,
    permission_manager: Arc<PermissionManager>,
    pub stats_manager: Arc<StatsManager>,
    storage: Arc<S>,
    #[cfg(feature = "vector")]
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
        #[cfg(feature = "vector")]
        return Self::create_service(config, storage, None, true, None, None).await;
        #[cfg(not(feature = "vector"))]
        return Self::create_service(config, storage, None, true, None).await;
    }

    /// Create a new GraphService (without a transaction manager and without starting any background tasks, for testing purposes).
    pub async fn new_for_test(config: Config, storage: Arc<S>) -> Arc<Self> {
        #[cfg(feature = "vector")]
        return Self::create_service(config, storage, None, false, None, None).await;
        #[cfg(not(feature = "vector"))]
        return Self::create_service(config, storage, None, false, None).await;
    }

    /// Use the transaction manager to create a GraphService.
    pub async fn new_with_transaction_manager(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
    ) -> Arc<Self> {
        #[cfg(feature = "vector")]
        return Self::create_service(config, storage, Some(transaction_manager), true, None, None)
            .await;
        #[cfg(not(feature = "vector"))]
        return Self::create_service(config, storage, Some(transaction_manager), true, None).await;
    }

    /// Use the transaction manager and external StatsManager to create a GraphService.
    pub async fn new_with_transaction_manager_and_stats(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
        stats_manager: Arc<StatsManager>,
    ) -> Arc<Self> {
        #[cfg(feature = "vector")]
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
        #[cfg(not(feature = "vector"))]
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

    /// Use the transaction manager, external StatsManager, and shared VectorBackend to create a GraphService.
    #[cfg(feature = "vector")]
    pub async fn with_shared_vector_backend(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Arc<TransactionManager>,
        stats_manager: Arc<StatsManager>,
        backend: VectorBackend,
    ) -> Arc<Self> {
        Self::create_service(
            config,
            storage,
            Some(transaction_manager),
            true,
            Some(stats_manager),
            Some(backend),
        )
        .await
    }

    /// Internal constructor: Extracts the common logic
    ///
    /// # Parameters
    /// `start_cleanup_task` – Whether to initiate the background task for session cleanup
    /// `shared_vector_backend` – Optional shared VectorBackend to avoid duplicate initialization
    async fn create_service(
        config: Config,
        storage: Arc<S>,
        transaction_manager: Option<Arc<TransactionManager>>,
        start_cleanup_task: bool,
        external_stats_manager: Option<Arc<StatsManager>>,
        #[cfg(feature = "vector")] shared_vector_backend: Option<VectorBackend>,
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

        #[cfg(feature = "vector")]
        let (query_api, vector_api) = if config.is_vector_enabled() {
            // Use shared backend if available, otherwise build one from config.
            let backend = match shared_vector_backend {
                Some(backend) => backend,
                None => {
                    let engine = vector_search::LocalVectorEngine::open(config.vector_data_dir())
                        .unwrap_or_else(|_| panic!("Failed to open local vector engine"));
                    if let Some(hnsw) =
                        graphdb_api::vector_config::local_hnsw_config(&config.vector_config().local)
                    {
                        engine.set_default_hnsw_config(hnsw);
                    }
                    if let Some(ivf) =
                        graphdb_api::vector_config::local_ivf_config(&config.vector_config().local)
                    {
                        engine.set_default_ivf_config(ivf);
                    }
                    if let Some(quant) = graphdb_api::vector_config::local_quantization_config(
                        &config.vector_config().local,
                    ) {
                        engine.set_default_quantization_config(quant);
                    }
                    graphdb_sync::backend::VectorBackend::local(engine)
                }
            };

            match QueryApi::with_vector_backend(
                Arc::new(RwLock::new((*storage).clone())),
                stats_manager.clone(),
                backend.clone(),
                schema_manager.clone(),
            )
            .await
            {
                Ok(mut api) => {
                    api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
                    let vector_api = Arc::new(VectorApi::new(backend));
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

        #[cfg(not(feature = "vector"))]
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
            #[cfg(feature = "vector")]
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

    pub async fn execute(&self, session_id: i64, stmt: &str) -> Result<QueryResult, String> {
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
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
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

        // Unified classification entry: transaction / session commands are
        // dispatched through the parser (single AST entry point), replacing
        // the legacy text-prefix dispatch. `LET $name = expr` is a session
        // command (evaluated through the query engine, stored on the
        // session); the six transaction commands perform the TransactionManager
        // side effect, execute the state-machine plan, and apply session
        // post-processing.
        match Self::parse_command(stmt) {
            Err(parse_error) => return Err(parse_error),
            Ok(Some(parsed)) => {
                let parsed_ast = parsed.ast;
                let stmt_ast = parsed_ast.stmt();
                match stmt_ast {
                    Stmt::AssignVariable(assign) => {
                        return self.execute_variable_assignment(
                            &session,
                            parsed_ast.clone(),
                            assign,
                            stmt,
                            space_id,
                            parameters,
                            session_variables,
                        );
                    }
                    _ => {
                        return self.execute_transaction_command(
                            &session,
                            stmt,
                            stmt_ast,
                            parsed_ast.clone(),
                            space_id,
                            parameters,
                            session_variables,
                        );
                    }
                }
            }
            Ok(None) => {}
        }

        // Perform a regular query using core layer QueryApi
        let mut result = self.execute_query_with_permission(
            session_id,
            stmt,
            None,
            space_id,
            parameters,
            session_variables,
        );

        // Handle SpaceSwitched result from USE statement
        // The core QueryApi carries the engine SpaceSwitched variant through
        // unchanged; the summary is read directly from it.
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

        // Transaction / session commands are forwarded to the materialized
        // `execute` path: the streaming path does not build a session
        // controller (no CommandScope branch in the stream executor), and
        // command results are single-row anyway.
        match Self::parse_command(stmt) {
            Err(parse_error) => return Err(parse_error),
            Ok(Some(_)) => {
                return self
                    .execute(session_id, stmt)
                    .await
                    .map(|result| StreamingQueryResult::from_execution_result(result.execution));
            }
            Ok(None) => {}
        }

        // Assign a server-side monotonic query ID up front so the request-scoped
        // id is threaded through QueryRequestContext → ExecutionContext → runtime.
        let query_id = self.next_query_id.fetch_add(1, Ordering::Relaxed) as u32;
        let query_request = graphdb_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: session.current_transaction(),
            parameters: None,
            session_variables: Some(session.variables_snapshot()),
            query_id: Some(query_id as u64),
            parsed_statement: None,
            consistency: Default::default(),
            minimum_lsn: None,
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

    // ==================== Unified transaction / session commands ====================

    /// Whether the statement text begins with a transaction / session
    /// command keyword (used to surface the first specific parse error for
    /// malformed commands instead of the generic recovery abort).
    fn is_command_like(stmt: &str) -> bool {
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

    /// Unified classification entry: parse the statement and return it when
    /// it is one of the transaction / session commands (BEGIN / COMMIT /
    /// ROLLBACK / SAVEPOINT / RELEASE SAVEPOINT / LET).
    ///
    /// The parse is gated behind the zero-cost [`Self::is_command_like`]
    /// text check: regular statements skip the API-layer parse entirely
    /// (the query engine parses them once on the regular path), so the
    /// single-parse pipeline applies to every statement.
    ///
    /// - `Ok(Some(parser_result))`: a command; the pre-parsed AST is reused
    ///   by the engine (skipping its internal parse) and by the command
    ///   handlers (no re-parse to read AST fields).
    /// - `Ok(None)`: a regular statement (or an unrecognized one) — the
    ///   regular query path handles it (and surfaces any parse error).
    /// - `Err(message)`: a command-like statement failed to parse; the first
    ///   specific parse error is surfaced instead of the generic recovery
    ///   abort (the parser's recovery masks the first error with
    ///   "Too many parse errors").
    fn parse_command(stmt: &str) -> Result<Option<ParserResult>, String> {
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
                    return Err(format!("Parse error: {}", first.message));
                }
                Ok(None)
            }
        }
    }

    /// Execute a transaction / session command through the unified
    /// AST → plan → operator → state machine pipeline.
    ///
    /// Flow per command: ① API-layer TransactionManager side effect (the
    /// parameters are read from the AST), ② QueryRequest construction with
    /// the transaction id semantics, ③ plan execution through the
    /// query API (the `TxnOperator` validates the session controller and
    /// produces a structured result), ④ API-layer session post-processing.
    /// Execute a transaction / session command through the unified
    /// AST → plan → operator → state machine pipeline.
    ///
    /// Flow per command: ① API-layer TransactionManager side effect (the
    /// parameters are read from the AST), ② QueryRequest construction with
    /// the transaction id semantics, ③ plan execution through the
    /// query API (the `TxnOperator` validates the session controller and
    /// produces a structured result), ④ API-layer session post-processing.
    #[allow(clippy::too_many_arguments)]
    fn execute_transaction_command(
        &self,
        session: &Arc<ClientSession>,
        stmt_text: &str,
        stmt: &Stmt,
        parsed_ast: Arc<Ast>,
        space_id: i64,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
        self.validate_session_transaction_state(session)?;
        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        match stmt {
            Stmt::BeginTransaction(begin_stmt) => {
                if session.has_active_transaction() {
                    return Err("Session already has an active transaction".to_string());
                }

                // ① TM side effect: begin with the AST access mode.
                let mut options = session.transaction_options();
                if let Some(read_only) = begin_stmt.read_only {
                    options.read_only = read_only;
                }
                let txn_id = match txn_manager
                    .begin_transaction_with_owner(options.clone(), session.id().to_string())
                {
                    Ok(txn_id) => txn_id,
                    Err(e) => {
                        // If the error is a write conflict, try cleaning up
                        // expired transactions and retry once. This handles
                        // the case where a stale transaction is blocking new
                        // write transactions.
                        if matches!(
                            e.kind(),
                            graphdb_transaction::TransactionErrorKind::WriteTransactionConflict
                        ) {
                            txn_manager.cleanup_expired_transactions();
                            match txn_manager
                                .begin_transaction_with_owner(options, session.id().to_string())
                            {
                                Ok(txn_id) => txn_id,
                                Err(retry_err) => {
                                    return Err(format!(
                                        "Failed to start transaction: {}",
                                        retry_err
                                    ));
                                }
                            }
                        } else {
                            return Err(format!("Failed to start transaction: {}", e));
                        }
                    }
                };

                // ② session binding + ③ plan execution (the controller
                // begins tracking the fresh transaction; the operator emits
                // the structured BEGIN result).
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
                    stmt_text,
                    parsed_ast,
                    space_id,
                    Some(txn_id),
                    parameters,
                    session_variables,
                );
                if result.is_err() {
                    // The plan failed unexpectedly (state-machine
                    // divergence is a bug); clean up the fresh binding so
                    // the session does not carry a stale transaction.
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

                // ① TM side effect: commit first so the request can carry
                // the (now finished) transaction id for state tracking.
                txn_manager
                    .commit_transaction(txn_id)
                    .map_err(|e| format!("Failed to commit transaction: {}", e))?;

                // ② session unbind + ④ session post-processing (variable
                // overlay merge) + ③ plan execution.
                session.unbind_transaction();
                session.set_auto_commit(true);
                session.commit_variables();
                info!("Session {} committed transaction {}", session.id(), txn_id);
                self.run_transaction_command_plan(
                    session.id(),
                    stmt_text,
                    parsed_ast,
                    space_id,
                    Some(txn_id),
                    parameters,
                    session_variables,
                )
            }

            Stmt::RollbackTransaction(rollback_stmt) => {
                if let Some(savepoint_name) = &rollback_stmt.savepoint_name {
                    // ROLLBACK TO SAVEPOINT: partial rollback; the
                    // transaction stays active.
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
                        stmt_text,
                        parsed_ast,
                        space_id,
                        Some(txn_id),
                        parameters,
                        session_variables,
                    )
                } else {
                    // Full transaction rollback.
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
                        stmt_text,
                        parsed_ast,
                        space_id,
                        Some(txn_id),
                        parameters,
                        session_variables,
                    )
                }
            }

            Stmt::Savepoint(savepoint_stmt) => {
                let txn_id = session
                    .current_transaction()
                    .ok_or("No active transaction, cannot create savepoint")?;
                let savepoint_id = txn_manager
                    .create_savepoint(txn_id, Some(savepoint_stmt.name.clone()))
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
                    stmt_text,
                    parsed_ast,
                    space_id,
                    Some(txn_id),
                    parameters,
                    session_variables,
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
                let context = txn_manager
                    .get_context(txn_id)
                    .map_err(|e| format!("Failed to get transaction context: {}", e))?;
                let savepoint_info = context
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
                    stmt_text,
                    parsed_ast,
                    space_id,
                    Some(txn_id),
                    parameters,
                    session_variables,
                );
                if result.is_ok() {
                    session.release_variable_savepoint(&release_stmt.name);
                }
                result
            }

            _ => Err("Statement is not a transaction command".to_string()),
        }
    }

    /// Execute the transaction-command plan: permission check + query API
    /// invocation with the explicit transaction id + result conversion.
    ///
    /// The command's plan runs in `TransactionScope::CommandScope`: the
    /// `TxnOperator` validates/tracks the session controller and produces
    /// the structured command/result row. An active transaction binds
    /// through `create_execution` (preserving timestamps and the read-only
    /// mode); finished transactions (COMMIT / ROLLBACK already
    /// performed the TM side effect) bind an auto-commit context.
    #[allow(clippy::too_many_arguments)]
    fn run_transaction_command_plan(
        &self,
        session_id: i64,
        stmt: &str,
        parsed_ast: Arc<Ast>,
        space_id: i64,
        transaction_id: Option<TransactionId>,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;

        session.charge();
        let username = session.user();

        // Permission check: transaction commands now go through the unified
        // permission path (previously bypassed by the prefix dispatch).
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

        // Resolve the immutable execution binding: an active transaction
        // binds through `create_execution`; a finished transaction (the TM
        // side effect already ran for COMMIT / ROLLBACK) binds an
        // auto-commit context — the command plan performs no data access.
        let execution = transaction_id.and_then(|id| {
            let manager = self.transaction_manager.as_ref()?;
            if manager.is_transaction_active(id) {
                manager.create_execution(id, false).ok()
            } else {
                None
            }
        });

        self.run_query_plan(
            &session,
            stmt,
            Some(parsed_ast.clone()),
            transaction_id,
            execution,
            parameters,
            session_variables,
        )
    }

    /// Execute a `LET $name = expr` session-variable assignment.
    ///
    /// The right-hand side is evaluated through the query engine (the LET
    /// statement plans to a single-row, single-column expression evaluation
    /// plan reusing the RETURN chain). The first value of the first row is
    /// stored in the session; inside an explicit transaction the assignment
    /// is recorded on the variable overlay so ROLLBACK / ROLLBACK TO
    /// SAVEPOINT restore the previous value. Client-supplied parameters and
    /// session variables are passed through to the evaluation.
    #[allow(clippy::too_many_arguments)]
    fn execute_variable_assignment(
        &self,
        session: &Arc<ClientSession>,
        parsed_ast: Arc<Ast>,
        assign: &crate::query::parser::ast::stmt::AssignVariableStmt,
        stmt: &str,
        space_id: i64,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
        let result = self.execute_query_with_permission(
            session.id(),
            stmt,
            Some(parsed_ast),
            space_id,
            parameters,
            session_variables,
        )?;
        let value = {
            // Contract: the LET plan evaluates to exactly one row with
            // one value column. Guard instead of silently taking the
            // first value if a planner regression changes the shape.
            if result.columns().len() != 1 {
                return Err(format!(
                    "LET expression must evaluate to a single value, got {} columns",
                    result.columns().len()
                ));
            }
            if result.rows().len() != 1 {
                return Err(format!(
                    "LET expression must evaluate to a single row, got {} rows",
                    result.rows().len()
                ));
            }
            result
                .first_value()
                .cloned()
                .ok_or_else(|| "LET expression returned no value".to_string())?
        };
        session.set_variable(assign.name.clone(), value);
        info!("Session {} set session variable", session.id());
        Ok(graphdb_api::api_core::QueryResult::empty())
    }

    fn execute_query_with_permission(
        &self,
        session_id: i64,
        stmt: &str,
        parsed_ast: Option<Arc<Ast>>,
        space_id: i64,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;

        session.charge();
        let username = session.user();

        // Permission check: The admin has all permissions, so no check is required.
        // USE is a session-level operation that does not access data — skip permission
        // check so any authenticated user can switch to a space. LET assigns a
        // session variable without touching data, so it is exempt the same way.
        let stmt_upper = stmt.trim().to_uppercase();
        let session_only_statement =
            stmt_upper.starts_with("USE ") || stmt_upper.starts_with("LET ");
        if !self.permission_manager.is_admin(&username) && !session_only_statement {
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
        let execution = if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                match txn_manager.begin_statement(txn_id) {
                    Ok((ctx, statement_start)) => {
                        statement_guard = Some((txn_manager.clone(), ctx.clone(), statement_start));
                        Some(
                            txn_manager
                                .create_execution(ctx.id, false)
                                .map_err(|error| error.to_string())?,
                        )
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

        let mut result = self.run_query_plan(
            &session,
            stmt,
            parsed_ast,
            None,
            execution,
            parameters,
            session_variables,
        );

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
                            session.rollback_variables();
                        }
                    }
                }
            }
        }

        result
    }

    /// Shared query execution segment for regular statements and transaction
    /// commands: builds the `QueryRequest` and invokes the query API with the
    /// resolved execution binding, converting the result.
    ///
    /// `transaction_id` overrides the session binding when provided (used by
    /// transaction commands whose TM side effect already happened); `execution`
    /// is an explicit transaction binding created by the caller (optionally
    /// after `begin_statement`); `None` binds an auto-commit context.
    /// `parsed_ast` is the classification-pass AST for command statements;
    /// the engine reuses it instead of re-parsing the text.
    #[allow(clippy::too_many_arguments)]
    fn run_query_plan(
        &self,
        session: &Arc<ClientSession>,
        stmt: &str,
        parsed_ast: Option<Arc<Ast>>,
        transaction_id: Option<TransactionId>,
        execution: Option<graphdb_transaction::types::TransactionExecution>,
        parameters: Option<HashMap<String, graphdb_core::Value>>,
        session_variables: Option<HashMap<String, graphdb_core::Value>>,
    ) -> Result<QueryResult, String> {
        // Session variables are injected through the dedicated
        // session_variables channel (captured once per statement), fully
        // decoupled from query parameters. Client-supplied session variables
        // override only the keys they name; all other keys keep the session
        // snapshot values (set via `LET $name = expr`).
        let merged_session_variables = match session_variables {
            Some(client_variables) => {
                let mut merged = session.variables_snapshot();
                merged.extend(client_variables);
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
            parameters,
            session_variables: merged_session_variables,
            query_id: None,
            parsed_statement: parsed_ast,
            consistency: Default::default(),
            minimum_lsn: None,
        };

        let mut query_api = self.query_api.write();
        let result = if let Some(execution) = execution.as_ref() {
            query_api
                .execute_with_execution(stmt, query_request, execution)
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

        result
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

    /// Extract the SpaceSummary from a USE-statement result.
    ///
    /// The engine executes USE as a DataSet with `space_name`/`space_id`/
    /// `vid_type` columns (the `SpaceSwitched` variant is never produced);
    /// `QueryResult::space_summary` recognizes both representations.
    fn extract_space_summary_from_result(result: &QueryResult) -> Option<SpaceSummary> {
        result.space_summary()
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

    #[cfg(feature = "vector")]
    pub fn vector_api(&self) -> Option<&Arc<VectorApi>> {
        self.vector_api.as_ref()
    }

    pub fn sync_api(&self) -> Option<&Arc<SyncApi>> {
        self.sync_api.as_ref()
    }

    /// Obtain the session list (SHOW SESSIONS)
    pub async fn list_sessions(&self) -> Vec<crate::session::SessionInfo> {
        self.session_manager.list_sessions().await
    }

    /// Obtain detailed information about the specified session.
    pub async fn get_session_info(&self, session_id: i64) -> Option<crate::session::SessionInfo> {
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
    /// auto-commit batch window: the write gate is acquired and MVCC
    /// snapshots registered once for the whole batch instead of once per
    /// statement. Each statement is permission-checked individually and runs
    /// independently (own timestamp / transaction id / undo log); a failure
    /// rolls back only its own statement and does not abort the rest of the
    /// batch.
    ///
    /// Inside an explicit transaction (or a non-auto-commit session) execution
    /// falls back to per-statement [`execute`](Self::execute), preserving
    /// transaction semantics. Transaction commands are rejected with an
    /// explicit error (the batch window contract is DML only); `LET`
    /// statements are routed through [`execute`](Self::execute) one by one
    /// (session side effects must run per statement). Returns one outcome per
    /// input statement, in order.
    pub async fn execute_batch(
        &self,
        session_id: i64,
        statements: &[String],
    ) -> Vec<Result<QueryResult, String>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| Err(format!("Invalid session ID: {session_id}")))
                .collect();
        };
        session.charge();
        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();

        // Classification pass: transaction commands get an explicit error,
        // LET statements run per-statement through `execute`, everything
        // else joins the batch window.
        let mut results: Vec<Option<Result<QueryResult, String>>> = vec![None; statements.len()];
        let mut batch_indices: Vec<usize> = Vec::new();
        let mut batch_statements: Vec<String> = Vec::new();
        for (index, stmt) in statements.iter().enumerate() {
            match Self::parse_command(stmt) {
                Err(parse_error) => results[index] = Some(Err(parse_error)),
                Ok(Some(parsed)) => match parsed.ast.stmt() {
                    Stmt::AssignVariable(_) => {
                        results[index] = Some(self.execute(session_id, stmt).await);
                    }
                    _ => {
                        results[index] = Some(Err(
                            "Transaction commands are not supported in batch execution".to_string(),
                        ));
                    }
                },
                Ok(None) => {
                    batch_indices.push(index);
                    batch_statements.push(stmt.clone());
                }
            }
        }

        // The batch window is for auto-commit DML only. Inside an explicit
        // transaction (or a non-auto-commit session) fall back to
        // per-statement execution so transaction semantics are preserved.
        if session.current_transaction().is_some() || !session.is_auto_commit() {
            for (index, stmt) in batch_statements.iter().enumerate() {
                results[batch_indices[index]] = Some(self.execute(session_id, stmt).await);
            }
            return finalize_batch_outcomes(results);
        }

        // Permission-check first so denied statements are never executed.
        let mut denied: Vec<(usize, String)> = Vec::new();
        let mut permitted: Vec<String> = Vec::with_capacity(batch_statements.len());
        for (index, stmt) in batch_statements.iter().enumerate() {
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

        let query_request = graphdb_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
            consistency: Default::default(),
            minimum_lsn: None,
        };
        let outcomes = self
            .query_api
            .write()
            .execute_batch(&permitted, query_request);

        merge_batch_outcomes(&mut results, &batch_indices, &denied, outcomes);
        finalize_batch_outcomes(results)
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
    /// group-commit windows: each consecutive group of `group_size`
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
    ) -> Vec<Result<QueryResult, String>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| Err(format!("Invalid session ID: {session_id}")))
                .collect();
        };
        session.charge();
        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();

        // Classification pass (same policy as execute_batch): transaction
        // commands get an explicit error, LET statements run per-statement.
        let mut results: Vec<Option<Result<QueryResult, String>>> = vec![None; statements.len()];
        let mut batch_indices: Vec<usize> = Vec::new();
        let mut batch_statements: Vec<String> = Vec::new();
        for (index, stmt) in statements.iter().enumerate() {
            match Self::parse_command(stmt) {
                Err(parse_error) => results[index] = Some(Err(parse_error)),
                Ok(Some(parsed)) => match parsed.ast.stmt() {
                    Stmt::AssignVariable(_) => {
                        results[index] = Some(self.execute(session_id, stmt).await);
                    }
                    _ => {
                        results[index] = Some(Err(
                            "Transaction commands are not supported in batch execution".to_string(),
                        ));
                    }
                },
                Ok(None) => {
                    batch_indices.push(index);
                    batch_statements.push(stmt.clone());
                }
            }
        }

        // Inside an explicit transaction (or a non-auto-commit session) fall
        // back to per-statement execution so transaction semantics are
        // preserved.
        if session.current_transaction().is_some() || !session.is_auto_commit() {
            for (index, stmt) in batch_statements.iter().enumerate() {
                results[batch_indices[index]] = Some(self.execute(session_id, stmt).await);
            }
            return finalize_batch_outcomes(results);
        }

        let mut denied: Vec<(usize, String)> = Vec::new();
        let mut permitted: Vec<String> = Vec::with_capacity(batch_statements.len());
        for (index, stmt) in batch_statements.iter().enumerate() {
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

        let query_request = graphdb_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
            consistency: Default::default(),
            minimum_lsn: None,
        };
        let outcomes =
            self.query_api
                .write()
                .execute_batch_grouped(&permitted, query_request, group_size);

        merge_batch_outcomes(&mut results, &batch_indices, &denied, outcomes);
        finalize_batch_outcomes(results)
    }
}

/// Convert the per-slot batch results into the final ordered outcome vector.
fn finalize_batch_outcomes(
    results: Vec<Option<Result<QueryResult, String>>>,
) -> Vec<Result<QueryResult, String>> {
    results
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| Err("Batch outcome missing".to_string())))
        .collect()
}

/// Merge the batch-window outcomes back into the per-slot results.
fn merge_batch_outcomes(
    results: &mut [Option<Result<QueryResult, String>>],
    batch_indices: &[usize],
    denied: &[(usize, String)],
    outcomes: Vec<Result<graphdb_api::api_core::QueryResult, graphdb_api::api_core::CoreError>>,
) {
    let mut permitted_outcomes = outcomes.into_iter();
    for (batch_pos, original_index) in batch_indices.iter().enumerate() {
        if let Some((_, error)) = denied.iter().find(|(i, _)| *i == batch_pos) {
            results[*original_index] = Some(Err(error.clone()));
            continue;
        }
        match permitted_outcomes.next() {
            Some(Ok(result)) => {
                results[*original_index] = Some(Ok(result));
            }
            Some(Err(error)) => results[*original_index] = Some(Err(error.to_string())),
            None => results[*original_index] = Some(Err("Batch outcome missing".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GraphService;
    use crate::storage::MockStorage;

    #[test]
    fn transaction_command_classification() {
        // The seven transaction / session commands are classified through
        // the unified parser entry; other statements pass through.
        let begin = GraphService::<MockStorage>::parse_command("BEGIN READ ONLY")
            .expect("BEGIN should classify")
            .expect("BEGIN should be a command");
        assert!(matches!(
            begin.ast.stmt(),
            crate::query::parser::ast::Stmt::BeginTransaction(ref s) if s.read_only == Some(true)
        ));

        let commit = GraphService::<MockStorage>::parse_command("COMMIT")
            .expect("COMMIT should classify")
            .expect("COMMIT should be a command");
        assert!(matches!(
            commit.ast.stmt(),
            crate::query::parser::ast::Stmt::CommitTransaction(_)
        ));

        let rollback = GraphService::<MockStorage>::parse_command("ROLLBACK TO sp1")
            .expect("ROLLBACK TO should classify")
            .expect("ROLLBACK TO should be a command");
        assert!(matches!(
            rollback.ast.stmt(),
            crate::query::parser::ast::Stmt::RollbackTransaction(ref s)
                if s.savepoint_name.as_deref() == Some("sp1")
        ));

        let savepoint = GraphService::<MockStorage>::parse_command("SAVEPOINT sp1")
            .expect("SAVEPOINT should classify")
            .expect("SAVEPOINT should be a command");
        assert!(matches!(
            savepoint.ast.stmt(),
            crate::query::parser::ast::Stmt::Savepoint(ref s) if s.name == "sp1"
        ));

        let release = GraphService::<MockStorage>::parse_command("RELEASE SAVEPOINT sp1")
            .expect("RELEASE SAVEPOINT should classify")
            .expect("RELEASE SAVEPOINT should be a command");
        assert!(matches!(
            release.ast.stmt(),
            crate::query::parser::ast::Stmt::ReleaseSavepoint(ref s) if s.name == "sp1"
        ));

        let let_stmt = GraphService::<MockStorage>::parse_command("LET $x = 1 + 2")
            .expect("LET should classify")
            .expect("LET should be a command");
        assert!(matches!(
            let_stmt.ast.stmt(),
            crate::query::parser::ast::Stmt::AssignVariable(ref s) if s.name == "x"
        ));

        // Regular statements and malformed commands are not classified.
        assert!(
            GraphService::<MockStorage>::parse_command("MATCH (n) RETURN n")
                .unwrap()
                .is_none()
        );
        assert!(GraphService::<MockStorage>::parse_command("COMMIT junk")
            .unwrap()
            .is_none());

        // Malformed commands surface the first specific parse error.
        let err =
            GraphService::<MockStorage>::parse_command("LET $x").expect_err("LET $x must fail");
        assert!(
            err.contains("LET requires an assignment"),
            "unexpected error: {}",
            err
        );

        // A bare `LET` also surfaces a specific error (command-like).
        let bare_let =
            GraphService::<MockStorage>::parse_command("LET").expect_err("bare LET must fail");
        assert!(
            bare_let.contains("Invalid session variable name"),
            "unexpected error: {}",
            bare_let
        );
    }
}
