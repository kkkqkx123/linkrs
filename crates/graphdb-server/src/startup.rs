//! Server startup functions
//!
//! Orchestrates storage, sync, transaction manager, and graph service initialization.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "vector-qdrant")]
use graphdb_embedding::EmbeddingService;
#[cfg(feature = "vector")]
use log::warn;
use log::{error, info};
#[cfg(feature = "vector-qdrant")]
use vector_client::VectorManager;

use crate::config::Config;
use crate::core::error::DBResult;
use crate::core::types::set_bcrypt_cost;
use crate::server::{GraphService, HttpServer};
use crate::storage::{
    GraphStorage, MetricsStorage, PersistenceConfig, PropertyGraphConfig, ResourceConfig,
    StorageCommitOps, SyncWrapper,
};
#[cfg(feature = "vector")]
use crate::sync::backend::VectorBackend;
use crate::transaction::{TransactionConfig, TransactionManager, TransactionManagerConfig};

/// Start the service using the user configuration directory.
pub async fn start_service() -> DBResult<()> {
    let config = match Config::load_user_config() {
        Ok(config) => config,
        Err(e) => {
            error!("Failed to load user config, using default config: {}", e);
            Config::default()
        }
    };
    start_service_with_config(config).await
}

/// Start the service using the configuration object.
pub async fn start_service_with_config(config: Config) -> DBResult<()> {
    info!("Initializing GraphDB service...");
    info!("Configuration loaded: {:?}", config);

    // Apply the bcrypt cost factor before any password hashing happens.
    // Only the first call in the process takes effect.
    set_bcrypt_cost(config.server.auth.bcrypt_cost);

    info!(
        "Log system has been initialized: {}/{}",
        config.log_dir(),
        config.log_file()
    );

    // Create shared StatsManager for all components before wiring storage decorators.
    let slow_query_config = config.to_slow_query_config();
    let m = &config.monitoring;
    let stats_manager = Arc::new(
        crate::core::stats::StatsManager::with_slow_query_logger(
            m.enabled,
            m.memory_cache_size,
            m.slow_query_threshold_ms * 1000,
            slow_query_config,
        )
        .expect("Failed to create StatsManager with slow query logger"),
    );

    let storage_path = PathBuf::from(config.storage_path());
    let mut persistence_config = PersistenceConfig::for_work_dir(&storage_path)
        .with_property_graph_config(property_graph_config_from_config(&config.storage));
    if config.storage.checkpoint_interval_secs > 0 {
        persistence_config.auto_checkpoint_interval =
            Duration::from_secs(config.storage.checkpoint_interval_secs);
    }
    let mut graph_storage =
        GraphStorage::open_with_persistence_config(storage_path, persistence_config)?;
    graph_storage = graph_storage.set_stats_manager(stats_manager.clone());
    let version_manager = graph_storage.version_manager();
    let inner_storage = Arc::new(MetricsStorage::new(graph_storage));
    info!(
        "Storage initialized (persistent mode at {}, metrics enabled)",
        config.storage_path()
    );

    // Build the shared vector backend from configuration (Local or Qdrant).
    #[cfg(feature = "vector")]
    let (vector_backend, local_engine_handle): (
        Option<VectorBackend>,
        Option<Arc<vector_search::LocalVectorEngine>>,
    ) = if config.is_vector_enabled() {
        match config.vector_config().engine {
            graphdb_config::config::VectorEngineKind::Local => {
                let data_dir = config.vector_data_dir();
                match vector_search::LocalVectorEngine::open(&data_dir) {
                    Ok(engine) => {
                        if let Some(hnsw) =
                            graphdb_api::local_hnsw_config(&config.vector_config().local)
                        {
                            engine.set_default_hnsw_config(hnsw);
                        }
                        if let Some(ivf) =
                            graphdb_api::local_ivf_config(&config.vector_config().local)
                        {
                            engine.set_default_ivf_config(ivf);
                        }
                        if let Some(quant) = graphdb_api::vector_config::local_quantization_config(
                            &config.vector_config().local,
                        ) {
                            engine.set_default_quantization_config(quant);
                        }
                        info!("Local vector engine initialized at {}", data_dir.display());
                        let arc = Arc::new(engine);
                        let handle = Arc::clone(&arc);
                        (Some(VectorBackend::from_local_arc(arc)), Some(handle))
                    }
                    Err(e) => {
                        warn!(
                            "Failed to open local vector engine at {}: {}. Vector search will be disabled.",
                            data_dir.display(),
                            e
                        );
                        (None, None)
                    }
                }
            }
            #[cfg(feature = "vector-qdrant")]
            graphdb_config::config::VectorEngineKind::Qdrant => {
                match VectorManager::new(config.vector_config().qdrant.clone()).await {
                    Ok(vm) => {
                        info!("VectorManager initialized");
                        (Some(VectorBackend::qdrant(Arc::new(vm))), None)
                    }
                    Err(e) => {
                        warn!(
                            "Failed to create VectorManager: {}. Vector search will be disabled.",
                            e
                        );
                        (None, None)
                    }
                }
            }
            #[cfg(not(feature = "vector-qdrant"))]
            graphdb_config::config::VectorEngineKind::Qdrant => {
                warn!("Qdrant engine requested but the `vector-qdrant` feature is not enabled. Vector search will be disabled.");
                (None, None)
            }
        }
    } else {
        (None, None)
    };
    #[cfg(not(feature = "vector"))]
    let _vector_backend = (None::<()>, None::<()>);

    // Forward local vector engine metrics into the shared StatsManager.
    #[cfg(feature = "vector")]
    if let Some(engine) = local_engine_handle {
        crate::vector_metrics::spawn_vector_metrics_sampler(engine, stats_manager.clone());
        info!("vector metrics sampling enabled");
    }

    // Forward remote (Qdrant) vector engine metrics into the shared StatsManager.
    #[cfg(feature = "vector-qdrant")]
    if let Some(manager) = vector_backend.as_ref().and_then(|b| b.as_qdrant_manager()) {
        let conn = &manager.config().connection;
        let http_port = conn.http_port.unwrap_or(6333);
        crate::vector_metrics::spawn_remote_vector_metrics_sampler(
            conn.host.clone(),
            http_port,
            conn.api_key.clone(),
            stats_manager.clone(),
        );
        info!(
            "remote vector metrics sampling enabled ({}:{})",
            conn.host, http_port
        );
    }

    let mut sync_manager = if config.fulltext.enabled || config.is_vector_enabled() {
        use crate::sync::SyncManager;

        let sync_manager = if config.fulltext.enabled {
            #[cfg(feature = "fulltext-search")]
            {
                use crate::search::manager::FulltextIndexManager;

                let manager = Arc::new(
                    FulltextIndexManager::new(config.fulltext.clone())
                        .expect("Failed to create FulltextIndexManager"),
                );

                use crate::search::{SyncConfig, SyncFailurePolicy};

                let sync_config = SyncConfig {
                    queue_size: 10000,
                    commit_interval_ms: 1000,
                    batch_size: 100,
                    failure_policy: SyncFailurePolicy::FailOpen,
                };

                let batch_config = crate::sync::batch::BatchConfig::from(sync_config.clone());
                let sync_coordinator = Arc::new(crate::sync::coordinator::SyncCoordinator::new(
                    manager.clone(),
                    batch_config,
                ));

                let sync_manager = SyncManager::with_sync_config(sync_coordinator, sync_config);

                // Attach vector coordinator if a backend is available
                #[cfg(feature = "vector")]
                let sync_manager = if let Some(backend) = &vector_backend {
                    attach_vector_coordinator(sync_manager, backend.clone(), &config)
                } else {
                    sync_manager
                };

                Some(Arc::new(sync_manager))
            }
            #[cfg(not(feature = "fulltext-search"))]
            {
                let sync_manager = SyncManager::new_without_fulltext();

                // Attach vector coordinator if a backend is available
                #[cfg(feature = "vector")]
                let sync_manager = if let Some(backend) = &vector_backend {
                    attach_vector_coordinator(sync_manager, backend.clone(), &config)
                } else {
                    sync_manager
                };

                Some(Arc::new(sync_manager))
            }
        } else {
            let sync_manager = SyncManager::new_without_fulltext();

            // Attach vector coordinator if a backend is available
            #[cfg(feature = "vector")]
            let sync_manager = if let Some(backend) = &vector_backend {
                attach_vector_coordinator(sync_manager, backend.clone(), &config)
            } else {
                sync_manager
            };

            Some(Arc::new(sync_manager))
        };

        if sync_manager.is_some() {
            info!("SyncManager initialized");
        }

        sync_manager
    } else {
        None
    };

    if let Some(manager) = sync_manager.as_mut().and_then(Arc::get_mut) {
        manager.set_stats_manager(stats_manager.clone());
        let outbox_path = PathBuf::from(config.storage_path()).join("outbox/outbox.sqlite");
        if let Err(error) = manager.configure_outbox(outbox_path) {
            return Err(crate::core::DBError::storage(format!(
                "Failed to initialize sync outbox: {}",
                error
            )));
        }
        if let Err(error) = manager.retry_outbox_sync() {
            error!("Initial outbox delivery will be retried later: {}", error);
        }
        let recovered = inner_storage
            .recover_outbox_projection(manager)
            .map_err(|error| {
                crate::core::DBError::storage(format!(
                    "Failed to recover the SQLite outbox projection: {}",
                    error
                ))
            })?;
        if recovered > 0 {
            info!("Recovered {} outbox intents from committed WAL", recovered);
        }
    }

    if let Some(manager) = sync_manager.as_ref() {
        manager.start().await.map_err(|error| {
            crate::core::DBError::storage(format!("Failed to start sync manager: {}", error))
        })?;
    }

    let storage = if let Some(ref sync_manager) = sync_manager {
        let sync_storage =
            SyncWrapper::with_sync_manager((*inner_storage).clone(), sync_manager.clone());
        info!("Sync enabled for fulltext and vector indexes");
        Arc::new(sync_storage)
    } else {
        let sync_storage = SyncWrapper::new((*inner_storage).clone());
        Arc::new(sync_storage)
    };

    // Create a transaction manager
    let txn_config = TransactionManagerConfig {
        default_timeout: std::time::Duration::from_secs(config.transaction.default_timeout),
        max_concurrent_transactions: config.transaction.max_concurrent_transactions,
        auto_cleanup: true,
        commit_retry_attempts: 3,
        abort_retry_attempts: 3,
        txn_config: TransactionConfig {
            auto_commit: config.transaction.auto_commit,
            ..TransactionConfig::default()
        },
    };

    let mut transaction_manager = TransactionManager::with_shared_version_manager(
        txn_config,
        stats_manager.clone(),
        version_manager,
    );
    if let Some(ref sync_manager) = sync_manager {
        transaction_manager = transaction_manager.with_sync_manager(sync_manager.clone());
    }
    transaction_manager = transaction_manager.with_commit_sink(storage.clone());
    let transaction_manager = Arc::new(transaction_manager);
    let _cleanup_task = transaction_manager.start_auto_cleanup_task();
    info!("Transaction manager initialized with StatsManager");

    // Create GraphService with shared VectorBackend to avoid duplicate initialization
    #[cfg(feature = "vector")]
    let graph_service = if let Some(backend) = &vector_backend {
        GraphService::with_shared_vector_backend(
            config.clone(),
            storage.clone(),
            transaction_manager.clone(),
            stats_manager.clone(),
            backend.clone(),
        )
        .await
    } else {
        GraphService::new_with_transaction_manager_and_stats(
            config.clone(),
            storage.clone(),
            transaction_manager.clone(),
            stats_manager.clone(),
        )
        .await
    };

    #[cfg(not(feature = "vector"))]
    let graph_service = GraphService::new_with_transaction_manager_and_stats(
        config.clone(),
        storage.clone(),
        transaction_manager.clone(),
        stats_manager.clone(),
    )
    .await;
    info!("Graph service initialized with transaction management");

    // Inject StatsManager into FulltextIndexManager to enable search metrics
    #[cfg(feature = "fulltext-search")]
    if let Some(sync_api) = graph_service.sync_api() {
        let fulltext_manager = sync_api.sync_manager().fulltext_manager();
        let stats_manager = graph_service.get_stats_manager().clone();
        fulltext_manager.set_stats_manager(stats_manager);
        info!("StatsManager injected into FulltextIndexManager for search metrics");
    }

    // Create HTTP server
    let http_server = Arc::new(HttpServer::new(
        graph_service.clone(),
        Arc::new(parking_lot::RwLock::new((*storage).clone())),
        transaction_manager,
        &config,
    ));
    info!("HTTP server created");

    info!(
        "Starting HTTP server on {}:{}",
        config.host(),
        config.port()
    );

    // Start HTTP server
    if let Err(e) = super::start_http_server(http_server, &config).await {
        error!("HTTP server error: {}", e);
    }

    super::shutdown_signal().await;

    info!("Shutting down GraphDB service...");
    graph_service.shutdown();
    Ok(())
}

/// Attach a vector sync coordinator backed by `backend` to a SyncManager.
#[cfg(feature = "vector")]
fn attach_vector_coordinator(
    sync_manager: crate::sync::SyncManager,
    backend: VectorBackend,
    _config: &Config,
) -> crate::sync::SyncManager {
    let handle = tokio::runtime::Handle::current();
    #[cfg(feature = "vector-qdrant")]
    let config = _config;
    #[cfg(feature = "vector-qdrant")]
    let embedding_service = {
        let es = config
            .vector_config()
            .qdrant
            .embedding
            .as_ref()
            .map(|ec| {
                EmbeddingService::from_config(ec.clone())
                    .map_err(|e| format!("Failed to create embedding service: {}", e))
            })
            .transpose();
        match es {
            Ok(es) => es.map(Arc::new),
            Err(e) => {
                warn!("Failed to create embedding service: {}", e);
                None
            }
        }
    };
    #[cfg(feature = "vector-qdrant")]
    let vector_coordinator = Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
        backend,
        embedding_service,
        handle,
    ));
    #[cfg(not(feature = "vector-qdrant"))]
    let vector_coordinator = Arc::new(
        crate::sync::vector_sync::VectorSyncCoordinator::new_without_embedding(backend, handle),
    );
    info!("Vector index sync enabled");
    sync_manager.with_vector_coordinator(vector_coordinator)
}

fn property_graph_config_from_config(
    storage: &crate::config::StorageConfig,
) -> PropertyGraphConfig {
    let mut config = PropertyGraphConfig::default();
    let cache_memory = match usize::try_from(storage.max_memory_bytes) {
        Ok(value) => value.min(config.cache_memory),
        Err(_) => config.cache_memory,
    };
    config.cache_memory = cache_memory;
    config.resources = ResourceConfig {
        max_memory_bytes: storage.max_memory_bytes,
        index_memory_bytes: storage.index_memory_bytes,
        memory_soft_ratio: storage.memory_soft_ratio,
        memory_hard_ratio: storage.memory_hard_ratio,
        max_active_snapshots: storage.max_active_snapshots,
        max_snapshot_age: Duration::from_secs(storage.max_snapshot_age_secs),
        max_tombstones: storage.max_tombstones,
        max_tombstone_bytes: storage.max_tombstone_bytes,
        index_gc_batch: storage.index_gc_batch,
        operation_timeout: Duration::from_secs(storage.operation_timeout_secs),
        dirty_flush_operations: storage.dirty_flush_operations,
        dirty_flush_bytes: storage.dirty_flush_bytes,
        cache_ttl: (storage.cache_ttl_secs > 0)
            .then(|| Duration::from_secs(storage.cache_ttl_secs)),
        cache_tti: (storage.cache_tti_secs > 0)
            .then(|| Duration::from_secs(storage.cache_tti_secs)),
        index_pool_capacity_bytes: storage.index_pool_capacity_bytes,
        index_eviction_enabled: storage.index_eviction_enabled,
        index_eviction_high_ratio: storage.index_eviction_high_ratio,
        index_eviction_low_ratio: storage.index_eviction_low_ratio,
        ..config.resources
    };
    config
}

/// Execute a single query directly (for CLI / quick testing).
pub async fn execute_query(query_str: &str) -> DBResult<()> {
    info!("Executing query: {}", query_str);

    let config = crate::config::Config::default();
    let inner_storage = Arc::new(GraphStorage::new()?);

    let sync_storage = SyncWrapper::new((*inner_storage).clone());
    let storage = Arc::new(sync_storage);

    let graph_service =
        GraphService::<SyncWrapper<GraphStorage>>::new_for_test(config, storage).await;

    let session = match graph_service
        .get_session_manager()
        .create_session("anonymous".to_string(), "127.0.0.1".to_string())
        .await
    {
        Ok(session) => session,
        Err(e) => {
            error!("Failed to create session: {}", e);
            return Err(crate::core::error::DBError::from(
                crate::server::session::SessionError::manager_error(format!(
                    "Failed to create session: {}",
                    e
                )),
            ));
        }
    };

    let session_id = session.id();

    match graph_service.execute(session_id, query_str).await {
        Ok(result) => {
            info!("Query executed successfully: {:?}", result);
        }
        Err(e) => {
            error!("Query execution error: {}", e);
        }
    }

    Ok(())
}
