//! Metrics and health handlers.
//!
//! `GetMetrics` serves the same Prometheus text as HTTP `/metrics`, so a
//! gRPC-only deployment (HTTP disabled) keeps its monitoring surface.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::*;
use super::service::LinkrsService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > LinkrsService<S>
{
    pub(crate) async fn handle_get_metrics(
        &self,
        _request: Request<GetMetricsRequest>,
    ) -> Result<Response<GetMetricsResponse>, Status> {
        let stats = self.app_state.server.get_stats_manager().clone();
        let body = linkrs_metrics::render_prometheus(&stats);
        Ok(Response::new(GetMetricsResponse {
            content_type: linkrs_metrics::PROMETHEUS_CONTENT_TYPE.to_string(),
            body,
        }))
    }

    /// Probe storage and the transaction manager instead of reporting a
    /// constant healthy value.
    pub(crate) async fn handle_health_check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let storage_ok = self.app_state.server.get_storage().try_read().is_some();
        let healthy = storage_ok;
        let uptime = self.start_time.elapsed().as_secs();
        Ok(Response::new(HealthCheckResponse {
            healthy,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: uptime as i64,
        }))
    }
}
