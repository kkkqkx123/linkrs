//! Server startup functions
//!
//! Orchestrates storage, sync, transaction manager, and graph service initialization.

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(feature = "qdrant")]
use log::warn;
use log::{error, info};
#[cfg(feature = "qdrant")]
use vector_client::EmbeddingService;
#[cfg(feature = "qdrant")]
use vector_client::VectorManager;

use crate::api::server::{GraphService, HttpServer};
use crate::config::Config;
use crate::core::error::DBResult;
use crate::storage::{GraphStorage, MetricsStorage, SyncWrapper};
use crate::transaction::{TransactionManager, TransactionManagerConfig};

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
    let mut graph_storage = GraphStorage::open(storage_path)?;
    graph_storage = graph_storage.set_stats_manager(stats_manager.clone());
    let inner_storage = Arc::new(MetricsStorage::new(
        graph_storage,
        stats_manager.clone(),
    ));
    info!(
        "Storage initialized (persistent mode at {}, metrics enabled)",
        config.storage_path()
    );

    // Initialize shared VectorManager if qdrant is enabled
    #[cfg(feature = "qdrant")]
    let vector_manager: Option<Arc<VectorManager>> = if config.is_vector_enabled() {
        match VectorManager::new(config.vector_config().clone()).await {
            Ok(vm) => {
                info!("VectorManager initialized");
                Some(Arc::new(vm))
            }
            Err(e) => {
                warn!(
                    "Failed to create VectorManager: {}. Vector search will be disabled.",
                    e
                );
                None
            }
        }
    } else {
        None
    };
    #[cfg(not(feature = "qdrant"))]
    let _vector_manager = None::<Arc<()>>;

    let sync_manager = if config.fulltext.enabled || config.is_vector_enabled() {
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

                // Attach vector coordinator if vector_manager is available
                #[cfg(feature = "qdrant")]
                let sync_manager = if let Some(vm) = &vector_manager {
                    let handle = tokio::runtime::Handle::current();
                    let embedding_service = config
                        .vector_config()
                        .embedding
                        .as_ref()
                        .map(|ec| {
                            EmbeddingService::from_config(ec.clone())
                                .map_err(|e| format!("Failed to create embedding service: {}", e))
                        })
                        .transpose();

                    let embedding_service = match embedding_service {
                        Ok(es) => es.map(Arc::new),
                        Err(e) => {
                            warn!("Failed to create embedding service: {}", e);
                            None
                        }
                    };

                    let vector_coordinator =
                        Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
                            vm.clone(),
                            embedding_service,
                            handle,
                        ));
                    info!("Vector index sync enabled");
                    sync_manager.with_vector_coordinator(vector_coordinator)
                } else {
                    sync_manager
                };

                Some(Arc::new(sync_manager))
            }
            #[cfg(not(feature = "fulltext-search"))]
            {
                let sync_manager = SyncManager::new_without_fulltext();

                // Attach vector coordinator if vector_manager is available
                #[cfg(feature = "qdrant")]
                let sync_manager = if let Some(vm) = &vector_manager {
                    let handle = tokio::runtime::Handle::current();
                    let embedding_service = config
                        .vector_config()
                        .embedding
                        .as_ref()
                        .map(|ec| {
                            EmbeddingService::from_config(ec.clone())
                                .map_err(|e| format!("Failed to create embedding service: {}", e))
                        })
                        .transpose();

                    let embedding_service = match embedding_service {
                        Ok(es) => es.map(Arc::new),
                        Err(e) => {
                            warn!("Failed to create embedding service: {}", e);
                            None
                        }
                    };

                    let vector_coordinator =
                        Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
                            vm.clone(),
                            embedding_service,
                            handle,
                        ));
                    info!("Vector index sync enabled");
                    sync_manager.with_vector_coordinator(vector_coordinator)
                } else {
                    sync_manager
                };

                Some(Arc::new(sync_manager))
            }
        } else {
            let sync_manager = SyncManager::new_without_fulltext();

            // Attach vector coordinator if vector_manager is available
            #[cfg(feature = "qdrant")]
            let sync_manager = if let Some(vm) = &vector_manager {
                let handle = tokio::runtime::Handle::current();
                let embedding_service = config
                    .vector_config()
                    .embedding
                    .as_ref()
                    .map(|ec| {
                        EmbeddingService::from_config(ec.clone())
                            .map_err(|e| format!("Failed to create embedding service: {}", e))
                    })
                    .transpose();

                let embedding_service = match embedding_service {
                    Ok(es) => es.map(Arc::new),
                    Err(e) => {
                        warn!("Failed to create embedding service: {}", e);
                        None
                    }
                };

                let vector_coordinator =
                    Arc::new(crate::sync::vector_sync::VectorSyncCoordinator::new(
                        vm.clone(),
                        embedding_service,
                        handle,
                    ));
                info!("Vector index sync enabled");
                sync_manager.with_vector_coordinator(vector_coordinator)
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
        write_lock_timeout: std::time::Duration::from_secs(10),
    };

    let mut transaction_manager =
        TransactionManager::with_stats_manager(txn_config, stats_manager.clone());
    if let Some(ref sync_manager) = sync_manager {
        transaction_manager = transaction_manager.with_sync_manager(sync_manager.clone());
    }
    let transaction_manager = Arc::new(transaction_manager);
    info!("Transaction manager initialized with StatsManager");

    // Create GraphService with shared VectorManager to avoid duplicate initialization
    #[cfg(feature = "qdrant")]
    let graph_service = if let Some(vm) = &vector_manager {
        GraphService::with_shared_vector_manager(
            config.clone(),
            storage.clone(),
            transaction_manager.clone(),
            stats_manager.clone(),
            vm.clone(),
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

    #[cfg(not(feature = "qdrant"))]
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
        graph_service,
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
    Ok(())
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
                crate::api::server::session::SessionError::manager_error(format!(
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
