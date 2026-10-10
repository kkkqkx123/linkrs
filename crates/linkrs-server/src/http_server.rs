//! HTTP and gRPC server bootstrap functions

use std::sync::Arc;

use log::info;

use crate::config::Config;
use crate::storage::UndoTarget;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSnapshotOps,
    StorageSyncContextOps,
};
use crate::HttpServer;
use linkrs_core::error::DBResult;

use super::shutdown::async_shutdown_signal;

/// Start an HTTP server using an asynchronous runtime.
pub async fn start_http_server<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + crate::storage::AutoCommitBatchOps
        + crate::storage::AutoCommitGroupOps
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

    if config.server.http.https_enabled {
        return Err(linkrs_core::DBError::validation(
            "http.https_enabled is not served by this build; terminate TLS upstream and leave it disabled",
        ));
    }
    if !config.server.http.enabled {
        return Err(linkrs_core::DBError::validation(
            "http.enabled is false; nothing to serve in http-only mode",
        ));
    }

    let state = crate::http::AppState::new(server.clone());

    // Create WebState for web management APIs
    let storage_path = format!("{}/metadata.db", config.storage_path());
    let web_router = match crate::web::WebState::new(&storage_path, state.clone()).await {
        Ok(web_state) => Some(crate::web::create_router(web_state)),
        Err(e) => {
            log::warn!(
                "Failed to initialize web management: {}, continuing without it",
                e
            );
            None
        }
    };

    let app = crate::http::router::create_router(state, web_router);

    let addr = format!("{}:{}", config.http_bind_address(), config.http_port());
    let listener = TcpListener::bind(&addr).await?;

    info!("HTTP server listening on {}", addr);

    serve(listener, app)
        .with_graceful_shutdown(async_shutdown_signal())
        .await?;

    Ok(())
}

/// Start both HTTP and gRPC servers concurrently.
#[cfg(feature = "grpc")]
pub async fn start_http_and_grpc_servers<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSnapshotOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + crate::storage::AutoCommitBatchOps
        + crate::storage::AutoCommitGroupOps
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

    if config.server.http.https_enabled {
        return Err(linkrs_core::DBError::validation(
            "http.https_enabled is not served by this build; terminate TLS upstream and leave it disabled",
        ));
    }

    let http_state = crate::http::AppState::new(http_server.clone());

    // Create WebState for web management APIs
    let storage_path = format!("{}/metadata.db", config.storage_path());
    let web_router = match crate::web::WebState::new(&storage_path, http_state.clone()).await {
        Ok(web_state) => Some(crate::web::create_router(web_state)),
        Err(e) => {
            log::warn!(
                "Failed to initialize web management: {}, continuing without it",
                e
            );
            None
        }
    };

    let http_enabled = config.server.http.enabled;
    let grpc_enabled = config.server.grpc.enabled;
    if !http_enabled && !grpc_enabled {
        return Err(linkrs_core::DBError::validation(
            "at least one of http.enabled and grpc.enabled must be true",
        ));
    }

    let http_app = crate::http::router::create_router(http_state.clone(), web_router);

    // Clone state for gRPC server
    let grpc_state = http_state.clone();
    let grpc_config = config.clone();
    let http_bind = config.http_bind_address().to_string();
    let http_port = config.http_port();

    // Start HTTP server when enabled, otherwise wait for shutdown only.
    let http_future = async move {
        if !http_enabled {
            async_shutdown_signal().await;
            return Ok::<(), linkrs_core::error::DBError>(());
        }
        let http_addr = format!("{}:{}", http_bind, http_port);
        let http_listener = TcpListener::bind(&http_addr).await?;
        info!("HTTP server listening on {}", http_addr);
        serve(http_listener, http_app)
            .with_graceful_shutdown(async_shutdown_signal())
            .await?;
        Ok::<(), linkrs_core::error::DBError>(())
    };

    // Start gRPC server when enabled, otherwise wait for shutdown only.
    let grpc_future = async move {
        if !grpc_enabled {
            async_shutdown_signal().await;
            return Ok::<(), linkrs_core::error::DBError>(());
        }
        let grpc_addr = format!(
            "{}:{}",
            grpc_state.server.get_config().grpc_bind_address(),
            grpc_state.server.get_config().grpc_port()
        )
        .parse::<std::net::SocketAddr>()
        .map_err(|e| linkrs_core::error::DBError::internal(e.to_string()))?;
        info!("gRPC server listening on {}", grpc_addr);
        crate::grpc::run_server(grpc_state, grpc_config, grpc_addr)
            .await
            .map_err(|e| linkrs_core::error::DBError::internal(e.to_string()))?;
        Ok::<(), linkrs_core::error::DBError>(())
    };

    // Run both servers concurrently
    tokio::select! {
        result = http_future => result?,
        result = grpc_future => result?,
    }

    Ok(())
}
