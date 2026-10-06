//! Sync/outbox management RPCs (parity with `/v1/sync/*`).

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::*;
use super::service::GraphDBService;

fn require_sync<S>(
    service: &GraphDBService<S>,
) -> Result<std::sync::Arc<graphdb_api::api_core::SyncApi>, Status>
where
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
{
    service
        .app_state
        .server
        .get_graph_service()
        .sync_api()
        .cloned()
        .ok_or_else(|| Status::failed_precondition("Synchronization is not configured"))
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > GraphDBService<S>
{
    pub(crate) async fn handle_get_sync_status(
        &self,
        _request: Request<GetSyncStatusRequest>,
    ) -> Result<Response<GetSyncStatusResponse>, Status> {
        let sync_api = require_sync(self)?;
        let outbox = sync_api.outbox_stats();
        let status_json = serde_json::to_string(&serde_json::json!({
            "is_running": sync_api.is_running(),
            "dlq_size": sync_api.get_dlq_size(),
            "unrecovered_dlq_size": sync_api.get_unrecovered_dlq_size(),
            "outbox_pending": outbox.pending,
            "outbox_retries": outbox.retries,
            "outbox_dead_lettered": outbox.dead_lettered,
            "outbox_leased": outbox.leased,
        }))
        .unwrap_or_else(|_| "{}".to_string());
        Ok(Response::new(GetSyncStatusResponse {
            is_running: sync_api.is_running(),
            outbox_pending: outbox.pending as u64,
            outbox_retries: outbox.retries,
            outbox_dead_lettered: outbox.dead_lettered as u64,
            status_json,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_retry_outbox(
        &self,
        _request: Request<RetryOutboxRequest>,
    ) -> Result<Response<RetryOutboxResponse>, Status> {
        let sync_api = require_sync(self)?;
        match sync_api.retry_outbox_projection() {
            Ok(delivered) => Ok(Response::new(RetryOutboxResponse {
                delivered: delivered as u64,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(RetryOutboxResponse {
                delivered: 0,
                error: e.to_string(),
            })),
        }
    }

    pub(crate) async fn handle_get_outbox_diagnostics(
        &self,
        _request: Request<GetOutboxDiagnosticsRequest>,
    ) -> Result<Response<GetOutboxDiagnosticsResponse>, Status> {
        let sync_api = require_sync(self)?;
        match sync_api.sync_diagnostics() {
            Ok(diag) => Ok(Response::new(GetOutboxDiagnosticsResponse {
                diagnostics_json: serde_json::to_string(&diag).unwrap_or_else(|_| "{}".to_string()),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(GetOutboxDiagnosticsResponse {
                diagnostics_json: "{}".to_string(),
                error: e.to_string(),
            })),
        }
    }

    pub(crate) async fn handle_list_dead_letters(
        &self,
        request: Request<ListDeadLettersRequest>,
    ) -> Result<Response<ListDeadLettersResponse>, Status> {
        let req = request.into_inner();
        let sync_api = require_sync(self)?;
        let target = req
            .target
            .filter(|s| !s.is_empty())
            .map(|s| {
                graphdb_core::types::TargetId::new(s)
                    .map_err(|e| Status::invalid_argument(e.to_string()))
            })
            .transpose()?;
        let rows = sync_api
            .list_dead_letters(
                target.as_ref(),
                req.index_id,
                req.generation,
                req.limit.unwrap_or(100) as usize,
                req.offset.unwrap_or(0) as usize,
            )
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(ListDeadLettersResponse {
            dead_letters_json: serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_requeue_dead_letters(
        &self,
        request: Request<RequeueDeadLettersRequest>,
    ) -> Result<Response<RequeueDeadLettersResponse>, Status> {
        let req = request.into_inner();
        let sync_api = require_sync(self)?;
        if !req.event_ids.is_empty() {
            let mut requeued = 0u64;
            for id in req.event_ids {
                match sync_api.requeue_dead_letter(id) {
                    Ok(true) => requeued += 1,
                    Ok(false) => {}
                    Err(e) => {
                        return Ok(Response::new(RequeueDeadLettersResponse {
                            requeued,
                            error: e.to_string(),
                        }));
                    }
                }
            }
            return Ok(Response::new(RequeueDeadLettersResponse {
                requeued,
                error: String::new(),
            }));
        }
        let target = req
            .target
            .filter(|s| !s.is_empty())
            .map(|s| {
                graphdb_core::types::TargetId::new(s)
                    .map_err(|e| Status::invalid_argument(e.to_string()))
            })
            .transpose()?;
        match sync_api.requeue_dead_letters_batch(
            target.as_ref(),
            req.index_id,
            req.generation,
            req.limit.unwrap_or(100) as usize,
        ) {
            Ok(count) => Ok(Response::new(RequeueDeadLettersResponse {
                requeued: count as u64,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(RequeueDeadLettersResponse {
                requeued: 0,
                error: e.to_string(),
            })),
        }
    }

    pub(crate) async fn handle_get_retention_status(
        &self,
        _request: Request<GetRetentionStatusRequest>,
    ) -> Result<Response<GetRetentionStatusResponse>, Status> {
        let sync_api = require_sync(self)?;
        match sync_api.retention_lsn() {
            Ok(lsn) => Ok(Response::new(GetRetentionStatusResponse {
                retention_lsn: lsn.get(),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(GetRetentionStatusResponse {
                retention_lsn: 0,
                error: e.to_string(),
            })),
        }
    }

    pub(crate) async fn handle_run_retention(
        &self,
        request: Request<RunRetentionRequest>,
    ) -> Result<Response<RunRetentionResponse>, Status> {
        let req = request.into_inner();
        let sync_api = require_sync(self)?;
        let grace = req.grace_lsn_distance.unwrap_or(10_000);
        let max_age = req.max_age_ms.unwrap_or(86_400_000 * 30);
        match sync_api.run_retention_once(grace, max_age) {
            Ok((pruned, archived, retention_lsn)) => Ok(Response::new(RunRetentionResponse {
                pruned,
                archived,
                retention_lsn,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(RunRetentionResponse {
                pruned: 0,
                archived: 0,
                retention_lsn: 0,
                error: e.to_string(),
            })),
        }
    }

    pub(crate) async fn handle_list_degraded_ranges(
        &self,
        request: Request<ListDegradedRangesRequest>,
    ) -> Result<Response<ListDegradedRangesResponse>, Status> {
        let req = request.into_inner();
        let sync_api = require_sync(self)?;
        let target = req
            .target
            .filter(|s| !s.is_empty())
            .map(|s| {
                graphdb_core::types::TargetId::new(s)
                    .map_err(|e| Status::invalid_argument(e.to_string()))
            })
            .transpose()?;
        match sync_api.list_degraded_ranges(target.as_ref(), req.index_id, req.generation) {
            Ok(rows) => Ok(Response::new(ListDegradedRangesResponse {
                degraded_ranges_json: serde_json::to_string(&rows)
                    .unwrap_or_else(|_| "[]".to_string()),
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(ListDegradedRangesResponse {
                degraded_ranges_json: "[]".to_string(),
                error: e,
            })),
        }
    }

    pub(crate) async fn handle_clear_degraded_range(
        &self,
        request: Request<ClearDegradedRangeRequest>,
    ) -> Result<Response<ClearDegradedRangeResponse>, Status> {
        let req = request.into_inner();
        if req.target.is_empty() {
            return Err(Status::invalid_argument("target is required"));
        }
        let sync_api = require_sync(self)?;
        let target = graphdb_core::types::TargetId::new(req.target)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        match sync_api.clear_degraded_range(
            &target,
            req.index_id,
            req.generation,
            graphdb_core::types::CommitLsn::new(req.start_lsn),
            graphdb_core::types::CommitLsn::new(req.end_lsn),
        ) {
            Ok(cleared) => Ok(Response::new(ClearDegradedRangeResponse {
                cleared,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(ClearDegradedRangeResponse {
                cleared: false,
                error: e,
            })),
        }
    }
}
