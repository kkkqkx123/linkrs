//! gRPC transport bootstrap.
//!
//! Builds the tonic router from live configuration.

use std::net::SocketAddr;

use tonic::transport::Server;

use crate::config::Config;
use crate::http::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSnapshotOps,
    StorageSyncContextOps,
};

use super::proto::graph_db_service_server::GraphDbServiceServer;
use super::service::GraphDBService;

/// Run the gRPC server
pub async fn run_server<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + StorageSnapshotOps
        + crate::storage::AutoCommitBatchOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    app_state: AppState<S>,
    config: Config,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    let service = GraphDBService::new(app_state.clone(), config.clone());

    tracing::info!("GraphDB gRPC service listening on {}", addr);

    let grpc_cfg = config.grpc().clone();
    let builder = Server::builder();
    let builder = if grpc_cfg.request_timeout_secs > 0 {
        builder.timeout(std::time::Duration::from_secs(
            grpc_cfg.request_timeout_secs,
        ))
    } else {
        builder
    };
    let builder = if grpc_cfg.keepalive_interval_secs > 0 {
        builder.http2_keepalive_interval(Some(std::time::Duration::from_secs(
            grpc_cfg.keepalive_interval_secs,
        )))
    } else {
        builder
    };
    let mut builder = if grpc_cfg.keepalive_timeout_secs > 0 {
        builder.http2_keepalive_timeout(Some(std::time::Duration::from_secs(
            grpc_cfg.keepalive_timeout_secs,
        )))
    } else {
        builder
    };
    let router = builder.add_service(
        GraphDbServiceServer::new(service)
            .max_decoding_message_size(grpc_cfg.max_request_size)
            .max_encoding_message_size(grpc_cfg.max_response_size),
    );

    router.serve(addr).await?;

    Ok(())
}

/// Run the gRPC server with custom service instance
pub async fn run_server_with_grpc_service<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + StorageSnapshotOps
        + crate::storage::AutoCommitBatchOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    service: GraphDBService<S>,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("GraphDB gRPC service listening on {}", addr);

    Server::builder()
        .add_service(GraphDbServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
