//! HTTP and gRPC server bootstrap functions

use std::sync::Arc;

use log::info;

use crate::api::server::HttpServer;
use crate::config::Config;
use crate::core::error::DBResult;
use crate::storage::UndoTarget;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSnapshotOps, StorageSyncContextOps,
    StorageTransactionContextOps,
};

use super::shutdown::async_shutdown_signal;

/// Start an HTTP server using an asynchronous runtime.
pub async fn start_http_server<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + UndoTarget
        + Clone
        + Send
        + Sync
        + 'static,
>(
    server: Arc<HttpServer<S>>,
    config: &Config,
) -> DBResult<()> {
    use axum::serve;
    use tokio::net::TcpListener;

    let state = crate::api::server::http::AppState::new(server.clone());

    // Create WebState for web management APIs
    let storage_path = format!("{}/metadata.db", config.storage_path());
    let web_router =
        match crate::api::server::web::WebState::new(&storage_path, state.clone()).await {
            Ok(web_state) => Some(crate::api::server::web::create_router(web_state)),
            Err(e) => {
                log::warn!(
                    "Failed to initialize web management: {}, continuing without it",
                    e
                );
                None
            }
        };

    let app = crate::api::server::http::router::create_router(state, web_router);

    let addr = format!("{}:{}", config.host(), config.port());
    let listener = TcpListener::bind(&addr).await?;

    info!("HTTP server listening on {}", addr);

    serve(listener, app)
        .with_graceful_shutdown(async_shutdown_signal())
        .await?;

    Ok(())
}

/// Start both HTTP and gRPC servers concurrently.
#[cfg(all(feature = "server", feature = "grpc"))]
pub async fn start_http_and_grpc_servers<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    http_server: Arc<HttpServer<S>>,
    config: &Config,
) -> DBResult<()> {
    use axum::serve;
    use tokio::net::TcpListener;

    let http_state = crate::api::server::http::AppState::new(http_server.clone());

    // Create WebState for web management APIs
    let storage_path = format!("{}/metadata.db", config.storage_path());
    let web_router =
        match crate::api::server::web::WebState::new(&storage_path, http_state.clone()).await {
            Ok(web_state) => Some(crate::api::server::web::create_router(web_state)),
            Err(e) => {
                log::warn!(
                    "Failed to initialize web management: {}, continuing without it",
                    e
                );
                None
            }
        };

    let http_app = crate::api::server::http::router::create_router(http_state.clone(), web_router);

    // Setup gRPC address
    let grpc_addr = format!("{}:{}", config.host(), config.grpc_port())
        .parse::<std::net::SocketAddr>()
        .map_err(|e| crate::core::error::DBError::internal(e.to_string()))?;

    // Setup HTTP address
    let http_addr = format!("{}:{}", config.host(), config.port());

    info!("HTTP server listening on {}", http_addr);
    info!("gRPC server listening on {}", grpc_addr);

    // Clone state for gRPC server
    let grpc_state = http_state.clone();
    let grpc_config = config.clone();

    // Start HTTP server
    let http_future = async move {
        let http_listener = TcpListener::bind(&http_addr).await?;
        serve(http_listener, http_app)
            .with_graceful_shutdown(async_shutdown_signal())
            .await?;
        Ok::<(), crate::core::error::DBError>(())
    };

    // Start gRPC server
    let grpc_future = async move {
        crate::api::server::grpc::run_server(grpc_state, grpc_config, grpc_addr)
            .await
            .map_err(|e| crate::core::error::DBError::internal(e.to_string()))?;
        Ok::<(), crate::core::error::DBError>(())
    };

    // Run both servers concurrently
    tokio::select! {
        result = http_future => result?,
        result = grpc_future => result?,
    }

    Ok(())
}
