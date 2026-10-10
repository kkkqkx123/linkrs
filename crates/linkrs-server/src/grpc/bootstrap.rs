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

use super::proto::linkrs_service_server::LinkrsServiceServer;
use super::service::LinkrsService;

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
    let service = LinkrsService::new(app_state.clone(), config.clone());

    tracing::info!("Linkrs gRPC service listening on {}", addr);

    let grpc_cfg = config.grpc().clone();
    if grpc_cfg.tls.enabled {
        return Err(format!(
            "grpc.tls.enabled is not served by this build; terminate TLS upstream and leave it disabled"
        )
        .into());
    }
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
        LinkrsServiceServer::new(service)
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
    service: LinkrsService<S>,
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Linkrs gRPC service listening on {}", addr);

    Server::builder()
        .add_service(LinkrsServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
