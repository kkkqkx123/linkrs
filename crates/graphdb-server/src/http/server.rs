//! HTTP server
//!
//! Provides an HTTP-based interface to GraphDB services

use crate::auth::PasswordAuthenticator;
use crate::batch::BatchManager;
use crate::config::Config;
use crate::graph_service::GraphService;
use crate::query::executor::expression::functions::FunctionRegistry;
use crate::session::GraphSessionManager;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use graphdb_api::api_core::{QueryApi, SchemaApi, TransactionApi};
use graphdb_transaction::TransactionManager;
use parking_lot::RwLock;
use std::sync::Arc;

/// HTTP server
///
/// Note: HttpServer relies on GraphService for the Rights Manager and Statistics Manager.
/// The session manager is accessed through the GraphService
pub struct HttpServer<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
> {
    graph_service: Arc<GraphService<S>>,
    query_api: QueryApi<S>,
    txn_manager: Arc<TransactionManager>,
    txn_api: TransactionApi,
    schema_api: SchemaApi<S>,
    auth_service: PasswordAuthenticator,
    batch_manager: Arc<BatchManager<S>>,
    storage: Arc<RwLock<S>>,
    /// Live configuration. Mutated by config-write endpoints and persisted
    /// to `config_path` when one was retained at startup.
    config: Arc<RwLock<Config>>,
    /// Config file the running config was loaded from, if any. Updates are
    /// written back here; `None` means memory-only updates.
    config_path: Option<std::path::PathBuf>,
    function_registry: Arc<RwLock<FunctionRegistry>>,
    rebuild_tasks: super::handlers::rebuild::RebuildTaskRegistry,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > HttpServer<S>
{
    /// Create a new HTTP server
    pub fn new(
        graph_service: Arc<GraphService<S>>,
        storage: Arc<RwLock<S>>,
        txn_manager: Arc<TransactionManager>,
        config: &Config,
        config_path: Option<std::path::PathBuf>,
    ) -> Self {
        // Use the shared StatsManager from GraphService
        let stats_manager = graph_service.get_stats_manager().clone();
        Self {
            graph_service: graph_service.clone(),
            query_api: QueryApi::new(storage.clone(), stats_manager)
                .with_password_history_depth(config.server.security.password_policy.history_size),
            txn_manager: txn_manager.clone(),
            txn_api: TransactionApi::new(txn_manager),
            schema_api: SchemaApi::new(storage.clone()),
            auth_service: PasswordAuthenticator::new_default(config.server.auth.clone()),
            batch_manager: Arc::new(BatchManager::new(storage.clone())),
            storage: storage.clone(),
            config: Arc::new(RwLock::new(config.clone())),
            config_path,
            function_registry: Arc::new(RwLock::new(FunctionRegistry::new())),
            rebuild_tasks: super::handlers::rebuild::RebuildTaskRegistry::default(),
        }
    }

    /// Get GraphService
    pub fn get_graph_service(&self) -> Arc<GraphService<S>> {
        self.graph_service.clone()
    }

    /// Get Session Manager (via GraphService)
    pub fn get_session_manager(&self) -> &GraphSessionManager {
        self.graph_service.get_session_manager()
    }

    /// Getting the Query API
    pub fn get_query_api(&self) -> &QueryApi<S> {
        &self.query_api
    }

    /// Getting the Transaction Manager
    pub fn get_txn_manager(&self) -> Arc<TransactionManager> {
        self.txn_manager.clone()
    }

    /// Getting the Transaction API (core layer)
    pub fn get_txn_api(&self) -> &TransactionApi {
        &self.txn_api
    }

    /// Getting the Schema API
    pub fn get_schema_api(&self) -> &SchemaApi<S> {
        &self.schema_api
    }

    /// Access to Certification Services
    pub fn get_auth_service(&self) -> &PasswordAuthenticator {
        &self.auth_service
    }

    /// Get Bulk Task Manager
    pub fn get_batch_manager(&self) -> Arc<BatchManager<S>> {
        self.batch_manager.clone()
    }

    /// Getting the Statistics Manager (via GraphService)
    pub fn get_stats_manager(&self) -> &Arc<graphdb_metrics::StatsManager> {
        self.graph_service.get_stats_manager()
    }

    /// Getting the Storage Client
    pub fn get_storage(&self) -> Arc<RwLock<S>> {
        self.storage.clone()
    }

    /// Get live configuration (read guard; reflects applied updates).
    pub fn get_config(&self) -> parking_lot::RwLockReadGuard<'_, Config> {
        self.config.read()
    }

    /// Config file updates are written back to, if one was retained.
    pub fn get_config_path(&self) -> Option<std::path::PathBuf> {
        self.config_path.clone()
    }

    /// Live configuration store for config-write endpoints.
    pub(crate) fn config_store(&self) -> Arc<RwLock<Config>> {
        Arc::clone(&self.config)
    }

    /// Build the engine-facing migration config from live settings.
    ///
    /// Directory fields fall back to `migration/` subdirectories of the
    /// storage data directory; an empty data directory leaves that
    /// capability disabled instead of failing.
    pub fn migration_config(&self) -> graphdb_migration::MigrationConfig {
        let guard = self.config.read();
        let settings = &guard.common.migration;
        let data_dir = guard.common.database.storage_path.as_str();
        graphdb_migration::MigrationConfig {
            batch_size: settings.batch_size,
            lock_ttl_secs: 300,
            checkpoint_dir: settings.resolved_checkpoint_dir(data_dir),
            lock_path: settings.resolved_lock_path(data_dir),
            min_free_bytes: settings.min_free_bytes,
            drain_timeout_ms: settings.drain_timeout_ms,
            backup_dir: settings.resolved_backup_dir(data_dir),
        }
    }

    /// Get function registry
    pub fn get_function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        self.function_registry.clone()
    }

    /// Async index rebuild task registry.
    pub fn rebuild_tasks(&self) -> super::handlers::rebuild::RebuildTaskRegistry {
        self.rebuild_tasks.clone()
    }
}
