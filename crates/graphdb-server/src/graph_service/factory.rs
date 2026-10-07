use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use parking_lot::RwLock;

#[cfg(feature = "vector")]
use graphdb_api::api_core::VectorApi;
use graphdb_api::api_core::{QueryApi, SyncApi};
use graphdb_core::event_dispatch::EventSubscriptions;
use graphdb_core::metadata::SchemaManager;
use graphdb_metrics::StatsManager;
use graphdb_query::query_manager::QueryManager;
use graphdb_query::SessionEvent;
#[cfg(feature = "vector")]
use graphdb_sync::backend::VectorBackend;
use graphdb_transaction::TransactionManager;

use super::GraphService;
use crate::auth::AuthenticatorFactory;
use crate::config::Config;
use crate::permission::{PermissionManager, GOD_SPACE_ID};
use crate::query::executor::streaming::pool::SharedScheduler;
use crate::query::executor::streaming::query_registry::QueryRegistry;
use crate::query::optimizer::PartitioningConfig;
use crate::session::GraphSessionManager;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

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
        crate::query::executor::streaming::operators::source_operator::set_column_block_enabled(
            config.common.columnar.column_block_enabled,
        );

        let session_idle_timeout = Duration::from_secs(config.transaction.default_timeout * 10);
        let session_events = Arc::new(EventSubscriptions::<SessionEvent>::new());
        let query_manager = Arc::new(QueryManager::new_with_shared(session_events.clone()));
        let session_manager = GraphSessionManager::new_with_shared(
            format!("{}:{}", config.database.host, config.database.port),
            config.database.max_connections,
            session_idle_timeout,
            session_events.clone(),
        );

        if start_cleanup_task {
            session_manager.start_cleanup_task().await;
        }

        let schema_manager: Option<Arc<SchemaManager>> = storage.get_schema_manager();

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

        let runtime = crate::config::RuntimeConfig::file(config.storage_path());
        let cost_profile = config
            .common
            .optimizer
            .storage_cost_profile
            .resolve_for_runtime(&runtime);
        let cost_config = graphdb_api::api_core::cost_config_for_profile(cost_profile);
        info!(
            "Optimizer cost profile resolved to {:?} (random_page_cost={})",
            cost_profile, cost_config.random_page_cost,
        );
        let mut optimizer_engine = crate::query::OptimizerEngine::new(cost_config);
        optimizer_engine.set_partitioning_config(Self::partitioning_config_from(&config));
        let shared_scheduler = Arc::new(SharedScheduler::new(
            optimizer_engine.partitioning_config().max_workers.max(1),
        ));
        let optimizer_engine = Arc::new(optimizer_engine);
        let query_registry = Arc::new(QueryRegistry::new());
        query_manager.set_query_registry(Arc::clone(&query_registry));
        info!(
            "Shared query scheduler created with {} worker(s)",
            shared_scheduler.max_workers()
        );

        #[cfg(feature = "vector")]
        let (query_api, vector_api) = if config.is_vector_enabled() {
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
                    api = api
                        .with_statistics_settings(
                            config.common.optimizer.statistics_sample_limit,
                            config.common.optimizer.statistics_min_epoch_delta,
                        )
                        .with_default_cost_profile_label(format!("{cost_profile:?}"))
                        .with_space_cost_profiles(
                            &config.common.optimizer.space_cost_profiles,
                            &runtime,
                        );
                    api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
                    api.install_query_manager(Arc::clone(&query_manager));
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
                        &config.common.optimizer,
                        cost_profile,
                        &runtime,
                    );
                    api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
                    api.install_query_manager(Arc::clone(&query_manager));
                    (Arc::new(RwLock::new(api)), None)
                }
            }
        } else {
            let mut api = Self::build_query_api(
                &storage,
                &stats_manager,
                schema_manager.as_ref(),
                optimizer_engine.clone(),
                &config.common.optimizer,
                cost_profile,
                &runtime,
            );
            api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
            api.install_query_manager(Arc::clone(&query_manager));
            (Arc::new(RwLock::new(api)), None)
        };

        #[cfg(not(feature = "vector"))]
        let query_api = {
            let mut api = Self::build_query_api(
                &storage,
                &stats_manager,
                schema_manager.as_ref(),
                optimizer_engine.clone(),
                &config.common.optimizer,
                cost_profile,
                &runtime,
            );
            api.install_shared_scheduler(shared_scheduler.clone(), query_registry.clone());
            api.install_query_manager(Arc::clone(&query_manager));
            Arc::new(RwLock::new(api))
        };

        if start_cleanup_task {
            Self::spawn_startup_statistics_load(query_api.clone(), storage.clone());
        }

        let permission_manager = Arc::new(PermissionManager::new());
        Self::ensure_admin_seed_and_rebuild(&storage, &permission_manager, &config);

        let authenticator =
            AuthenticatorFactory::create_with_storage(&config.server.auth, Arc::clone(&storage));

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
            query_manager,
            progress_rows_interval: config.monitoring.progress_report_rows_interval,
            next_query_id: std::sync::atomic::AtomicU64::new(1),
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
        optimizer_config: &crate::config::OptimizerConfig,
        cost_profile: crate::config::StorageCostProfile,
        runtime: &crate::config::RuntimeConfig,
    ) -> QueryApi<S> {
        let inner = Arc::new(RwLock::new((**storage).clone()));
        QueryApi::with_optimizer_engine(
            inner,
            stats_manager.clone(),
            optimizer_engine,
            schema_manager.cloned(),
        )
        .with_statistics_settings(
            optimizer_config.statistics_sample_limit,
            optimizer_config.statistics_min_epoch_delta,
        )
        .with_default_cost_profile_label(format!("{cost_profile:?}"))
        .with_space_cost_profiles(&optimizer_config.space_cost_profiles, runtime)
    }

    /// Map the `[parallel]` config section onto the query optimizer's
    /// partitioning configuration.
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

    /// Seed the default admin on first start and rebuild role mappings.
    ///
    /// When the user table is empty, the configured default credentials are
    /// hashed into a new admin with the global God role; the plaintext
    /// password is never compared afterwards. Otherwise permission state is
    /// rebuilt from the persisted role segment so restarts keep grants.
    fn ensure_admin_seed_and_rebuild(
        storage: &Arc<S>,
        permission_manager: &Arc<PermissionManager>,
        config: &Config,
    ) {
        let mut handle = (**storage).clone();
        if handle.list_users().is_empty() {
            let username = config.server.auth.default_username.clone();
            let password = config.server.auth.default_password.clone();
            if !username.is_empty() && !password.is_empty() {
                match graphdb_core::types::UserInfo::new(username.clone(), password) {
                    Ok(info) => {
                        if let Err(error) = handle.create_user(&info) {
                            warn!("Admin seed failed for '{}': {}", username, error);
                        } else if let Err(error) =
                            handle.grant_role(&username, GOD_SPACE_ID, graphdb_core::RoleType::God)
                        {
                            warn!("Admin seed grant failed for '{}': {}", username, error);
                        } else {
                            info!("Seeded default admin '{}'", username);
                        }
                    }
                    Err(error) => warn!("Admin seed hashing failed: {}", error),
                }
            }
        }
        let roles = handle.list_all_user_roles();
        if !roles.is_empty() {
            permission_manager.rebuild_from_snapshot(roles);
        }
    }
}
