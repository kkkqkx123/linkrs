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
use crate::core::types::TransactionContextInfo;
use crate::core::{DataType, MetricType, Permission};
use crate::query::executor::ExecutionResult;
use crate::query::DataSet;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use crate::transaction::TransactionManager;
use log::{info, warn};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::Duration;
#[cfg(feature = "qdrant")]
use vector_client::VectorManager;

/// RAII guard to ensure transaction context is always cleared from storage.
struct TransactionContextGuard<'a, S: StorageTransactionContextOps + ?Sized> {
    storage: &'a S,
}

impl<'a, S: StorageTransactionContextOps + ?Sized> TransactionContextGuard<'a, S> {
    fn new(storage: &'a S) -> Self {
        Self { storage }
    }
}

impl<S: StorageTransactionContextOps + ?Sized> Drop for TransactionContextGuard<'_, S> {
    fn drop(&mut self) {
        self.storage.set_transaction_context(None);
    }
}

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
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageTransactionContextOps
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
                Ok(api) => {
                    let vector_api = Arc::new(VectorApi::new(vm));
                    (Arc::new(RwLock::new(api)), Some(vector_api))
                }
                Err(e) => {
                    warn!(
                        "Failed to initialize vector search, falling back to basic QueryApi: {}",
                        e
                    );
                    let api =
                        Self::build_query_api(&storage, &stats_manager, schema_manager.as_ref());
                    (Arc::new(RwLock::new(api)), None)
                }
            }
        } else {
            let api = Self::build_query_api(&storage, &stats_manager, schema_manager.as_ref());
            (Arc::new(RwLock::new(api)), None)
        };

        #[cfg(not(feature = "qdrant"))]
        let query_api = {
            let api = Self::build_query_api(&storage, &stats_manager, schema_manager.as_ref());
            Arc::new(RwLock::new(api))
        };

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
        };
        Arc::new(service)
    }

    /// Shared helper: build a QueryApi with optional SchemaManager
    fn build_query_api(
        storage: &Arc<S>,
        stats_manager: &Arc<StatsManager>,
        schema_manager: Option<&Arc<SchemaManager>>,
    ) -> QueryApi<S> {
        let inner = Arc::new(RwLock::new((**storage).clone()));
        if let Some(sm) = schema_manager {
            QueryApi::with_schema_manager(inner, stats_manager.clone(), sm.clone())
        } else {
            QueryApi::new(inner, stats_manager.clone())
        }
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
            return self.handle_begin_transaction(&session);
        } else if trimmed_stmt.starts_with("COMMIT") {
            return self.handle_commit_transaction(&session).await;
        } else if trimmed_stmt.starts_with("ROLLBACK") {
            return self.handle_rollback_transaction(&session, stmt);
        } else if trimmed_stmt.starts_with("SAVEPOINT") {
            return self.handle_savepoint(&session, stmt);
        } else if trimmed_stmt.starts_with("RELEASE SAVEPOINT") {
            return self.handle_release_savepoint(&session, stmt);
        }

        // Perform a regular query using core layer QueryApi
        let mut result = self.execute_query_with_permission(session_id, stmt, space_id);

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

    fn execute_query_with_permission(
        &self,
        session_id: i64,
        stmt: &str,
        space_id: i64,
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

        // If session has an active transaction, set the transaction context on storage
        // so that subsequent queries execute within the same transaction
        let txn_context = if let Some(txn_id) = session.current_transaction() {
            if let Some(ref txn_manager) = self.transaction_manager {
                match txn_manager.get_context(txn_id) {
                    Ok(ctx) => {
                        if !ctx.state().can_execute() {
                            warn!(
                                "Transaction {} is in invalid state {}, cleaning up session binding",
                                txn_id, ctx.state()
                            );
                            session.unbind_transaction();
                            None
                        } else {
                            Some(ctx)
                        }
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get transaction context for {}: {}, unbinding from session",
                            txn_id, e
                        );
                        session.unbind_transaction();
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ref ctx) = txn_context {
            let ctx_info = Arc::new(TransactionContextInfo::new(
                ctx.id,
                ctx.start_timestamp,
                ctx.read_only,
                0,
            ));
            self.storage.set_transaction_context(Some(ctx_info));
        }

        // RAII guard ensures transaction context is cleared even if query panics
        let _guard = TransactionContextGuard::new(self.storage.as_ref());

        // Use core layer QueryApi to execute query
        let query_request = crate::api::core::QueryRequest {
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: session.is_auto_commit(),
            transaction_id: session.current_transaction(),
            parameters: None,
        };

        let mut query_api = self.query_api.write();
        let result = query_api.execute(stmt, query_request);

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
                                txn_id, ctx.state()
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

        ExecutionResult::DataSet(DataSet {
            col_names: result.columns,
            rows,
        })
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
            ExecutionResult::DataSet(ds) => {
                let name_idx = ds.col_names.iter().position(|c| c == "space_name")?;
                let id_idx = ds.col_names.iter().position(|c| c == "space_id")?;
                let vid_type_idx = ds.col_names.iter().position(|c| c == "vid_type");
                let row = ds.rows.first()?;
                let name = match row.get(name_idx)? {
                    crate::core::Value::String(s) => s.clone(),
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

    /// Processing the BEGIN TRANSACTION statement
    fn handle_begin_transaction(
        &self,
        session: &Arc<ClientSession>,
    ) -> Result<ExecutionResult, String> {
        self.validate_session_transaction_state(session)?;

        if session.has_active_transaction() {
            return Err("Session already has an active transaction".to_string());
        }

        let txn_manager = self
            .transaction_manager
            .as_ref()
            .ok_or("Transaction manager not initialized")?;

        let options = session.transaction_options();
        match txn_manager.begin_transaction(options) {
            Ok(txn_id) => {
                session.bind_transaction(txn_id);
                session.set_auto_commit(false);
                info!("Session {} started transaction {}", session.id(), txn_id);
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
                    match txn_manager.begin_transaction(options) {
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
            let savepoint_name = trimmed
                .strip_prefix("ROLLBACK TO ")
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

        let context = txn_manager
            .get_context(txn_id)
            .map_err(|e| format!("Failed to get transaction context: {}", e))?;

        let savepoint_id = context.create_savepoint(Some(savepoint_name.to_string()), 0);

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

        // Try to find the save point by using its name.
        let savepoint_info = context
            .find_savepoint_by_name(savepoint_name)
            .ok_or_else(|| format!("Savepoint '{}' does not exist", savepoint_name))?;

        // Release the savepoint.
        if let Err(e) = context.release_savepoint(savepoint_info.id) {
            return Err(format!("Failed to release savepoint: {}", e));
        }

        info!(
            "Session {} released savepoint {} in transaction {}",
            session.id(),
            savepoint_name,
            txn_id
        );

        Ok(ExecutionResult::Success)
    }
}
