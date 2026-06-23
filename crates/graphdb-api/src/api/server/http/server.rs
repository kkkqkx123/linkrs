//! HTTP server
//!
//! Provides an HTTP-based interface to GraphDB services

use crate::api::core::{QueryApi, SchemaApi, TransactionApi};
use crate::api::server::auth::PasswordAuthenticator;
use crate::api::server::batch::BatchManager;
use crate::api::server::graph_service::GraphService;
use crate::api::server::session::GraphSessionManager;
use crate::config::Config;
use crate::query::executor::expression::functions::FunctionRegistry;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use crate::transaction::TransactionManager;
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
        + StorageTransactionContextOps
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
    config: Config,
    function_registry: Arc<RwLock<FunctionRegistry>>,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageTransactionContextOps
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
    ) -> Self {
        // Use the shared StatsManager from GraphService
        let stats_manager = graph_service.get_stats_manager().clone();
        Self {
            graph_service: graph_service.clone(),
            query_api: QueryApi::new(storage.clone(), stats_manager),
            txn_manager: txn_manager.clone(),
            txn_api: TransactionApi::new(txn_manager),
            schema_api: SchemaApi::new(storage.clone()),
            auth_service: PasswordAuthenticator::new_default(config.server.auth.clone()),
            batch_manager: Arc::new(BatchManager::new(storage.clone())),
            storage: storage.clone(),
            config: config.clone(),
            function_registry: Arc::new(RwLock::new(FunctionRegistry::new())),
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
    pub fn get_stats_manager(&self) -> &Arc<crate::core::StatsManager> {
        self.graph_service.get_stats_manager()
    }

    /// Getting the Storage Client
    pub fn get_storage(&self) -> Arc<RwLock<S>> {
        self.storage.clone()
    }

    /// Get Configuration
    pub fn get_config(&self) -> &Config {
        &self.config
    }

    /// Getting a variable reference to a configuration
    pub fn get_config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// Get function registry
    pub fn get_function_registry(&self) -> Arc<RwLock<FunctionRegistry>> {
        self.function_registry.clone()
    }
}
