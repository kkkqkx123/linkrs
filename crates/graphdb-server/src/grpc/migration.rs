//! Schema migration plan, execution and progress streaming.

use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::*;
use super::service::{GraphDBService, StreamMigrationProgressStream};

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
    pub(crate) async fn handle_migrate_plan(
        &self,
        request: Request<MigratePlanRequest>,
    ) -> Result<Response<MigratePlanResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let storage_read = storage.read();

        let plan = if req.is_edge {
            graphdb_migration::generate_edge_plan(
                &*storage_read,
                &req.space,
                &req.label,
                req.from_version,
                req.to_version,
            )
        } else {
            graphdb_migration::generate_vertex_plan(
                &*storage_read,
                &req.space,
                &req.label,
                req.from_version,
                req.to_version,
            )
        }
        .map_err(|e| Status::internal(e.to_string()))?;

        let plan_json =
            serde_json::to_string(&plan).map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigratePlanResponse {
            plan_json,
            safety_level: format!("{:?}", plan.overall_safety),
            estimated_rows: plan.estimated_rows,
            steps: plan
                .steps
                .iter()
                .map(|s| MigrationStep {
                    step_type: format!("{:?}", s),
                    description: s.description(),
                    safety_level: format!("{:?}", s.safety_level()),
                    is_data_modifying: s.is_data_modifying(),
                })
                .collect(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_migrate_execute(
        &self,
        request: Request<MigrateExecuteRequest>,
    ) -> Result<Response<MigrateExecuteResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let stats = self.app_state.server.get_stats_manager();
        let start = std::time::Instant::now();
        stats.record_migration_start();
        let mut storage_write = storage.write();

        let plan: graphdb_migration::MigrationPlan = serde_json::from_str(&req.plan_json)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let sender = crate::http::handlers::migration_progress::get_or_create_sender(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        );
        let registry = graphdb_core::event_dispatch::EventSubscriptions::<
            graphdb_migration::MigrationEvent,
        >::new();
        registry.add(crate::http::handlers::migration_progress::event_bridge(
            sender,
        ));
        let registry = std::sync::Arc::new(registry);
        let report = graphdb_migration::execute_migration_plan_with_options(
            &mut *storage_write,
            &plan,
            graphdb_migration::ExecuteOptions {
                event_registry: Some(&registry),
                ..Default::default()
            },
        );
        let elapsed = start.elapsed().as_millis() as u64;
        match &report {
            Ok(r) if r.success => stats.record_migration_success(r.rows_migrated, elapsed),
            Ok(_) => stats.record_migration_failure(elapsed),
            Err(_) => stats.record_migration_failure(elapsed),
        }
        let report = report.map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigrateExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed as u64,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_migrate_rollback(
        &self,
        request: Request<MigrateRollbackRequest>,
    ) -> Result<Response<MigrateRollbackResponse>, Status> {
        let req = request.into_inner();
        let storage = self.app_state.server.get_storage();
        let mut storage_write = storage.write();

        let plan: graphdb_migration::MigrationPlan = serde_json::from_str(&req.plan_json)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;

        let report = graphdb_migration::rollback_migration(&mut *storage_write, &plan)
            .map_err(|e| Status::internal(e.to_string()))?;

        Ok(Response::new(MigrateRollbackResponse {
            success: report.success,
            steps_completed: report.steps_completed as u64,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_dry_run_migration(
        &self,
        request: Request<DryRunMigrationRequest>,
    ) -> Result<Response<DryRunMigrationResponse>, Status> {
        let req = request.into_inner();
        let wire = graphdb_wire::migration::MigrationExecuteRequest {
            plan_json: req.plan_json,
        };
        match crate::http::handlers::schema::migration::dry_run_migration(
            axum::extract::State(self.app_state.clone()),
            axum::Json(wire),
        )
        .await
        {
            Ok(axum::Json(resp)) => Ok(Response::new(DryRunMigrationResponse {
                success: resp.success,
                steps_completed: resp.steps_completed as u64,
                rows_migrated: resp.rows_migrated,
                errors: resp.errors,
                error: String::new(),
            })),
            Err(e) => Err(map_http_error(e)),
        }
    }

    pub(crate) async fn handle_get_migration_history(
        &self,
        request: Request<GetMigrationHistoryRequest>,
    ) -> Result<Response<GetMigrationHistoryResponse>, Status> {
        let req = request.into_inner();
        let query =
            std::collections::HashMap::from([("is_edge".to_string(), req.is_edge.to_string())]);
        match crate::http::handlers::schema::migration::migration_history(
            axum::extract::State(self.app_state.clone()),
            axum::extract::Path((req.space, req.label)),
            axum::extract::Query(query),
        )
        .await
        {
            Ok(axum::Json(resp)) => Ok(Response::new(GetMigrationHistoryResponse {
                history_json: serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string()),
                error: String::new(),
            })),
            Err(e) => Err(map_http_error(e)),
        }
    }

    pub(crate) async fn handle_get_migration_status(
        &self,
        request: Request<GetMigrationStatusRequest>,
    ) -> Result<Response<GetMigrationStatusResponse>, Status> {
        let req = request.into_inner();
        let query =
            std::collections::HashMap::from([("is_edge".to_string(), req.is_edge.to_string())]);
        match crate::http::handlers::schema::migration::migration_status(
            axum::extract::State(self.app_state.clone()),
            axum::extract::Path((req.space, req.label)),
            axum::extract::Query(query),
        )
        .await
        {
            Ok(axum::Json(resp)) => Ok(Response::new(GetMigrationStatusResponse {
                status_json: serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string()),
                error: String::new(),
            })),
            Err(e) => Err(map_http_error(e)),
        }
    }

    pub(crate) async fn handle_stream_migration_progress(
        &self,
        request: Request<StreamMigrationProgressRequest>,
    ) -> Result<Response<StreamMigrationProgressStream>, Status> {
        let req = request.into_inner();
        let rx = crate::http::handlers::migration_progress::subscribe(
            &req.space,
            &req.label,
            req.is_edge,
        );
        let rx_stream = tokio_stream::wrappers::BroadcastStream::new(rx);
        let stream = rx_stream.filter_map(|res| match res {
            Ok(ev) => {
                let proto_ev = match ev {
                    graphdb_migration::MigrationEvent::Started { plan } => MigrationProgressEvent {
                        event_type: "started".to_string(),
                        message: plan.plan_hash.clone(),
                        step_idx: 0,
                        rows: 0,
                        success: false,
                        error: String::new(),
                    },
                    graphdb_migration::MigrationEvent::StepStarted { step_idx } => {
                        MigrationProgressEvent {
                            event_type: "step_started".to_string(),
                            message: String::new(),
                            step_idx: step_idx as u64,
                            rows: 0,
                            success: false,
                            error: String::new(),
                        }
                    }
                    graphdb_migration::MigrationEvent::StepCompleted { step_idx, rows } => {
                        MigrationProgressEvent {
                            event_type: "step_completed".to_string(),
                            message: String::new(),
                            step_idx: step_idx as u64,
                            rows,
                            success: true,
                            error: String::new(),
                        }
                    }
                    graphdb_migration::MigrationEvent::Completed { report } => {
                        MigrationProgressEvent {
                            event_type: "completed".to_string(),
                            message: format!(
                                "{} steps, {} rows",
                                report.steps_completed, report.rows_migrated
                            ),
                            step_idx: 0,
                            rows: report.rows_migrated,
                            success: report.success,
                            error: report.errors.join("; "),
                        }
                    }
                    graphdb_migration::MigrationEvent::Failed { error } => MigrationProgressEvent {
                        event_type: "failed".to_string(),
                        message: String::new(),
                        step_idx: 0,
                        rows: 0,
                        success: false,
                        error,
                    },
                    graphdb_migration::MigrationEvent::RolledBack { report } => {
                        MigrationProgressEvent {
                            event_type: "rolled_back".to_string(),
                            message: String::new(),
                            step_idx: 0,
                            rows: report.rows_migrated,
                            success: report.success,
                            error: String::new(),
                        }
                    }
                };
                Some(Ok(proto_ev))
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => None,
        });
        let boxed: StreamMigrationProgressStream = Box::pin(stream);
        Ok(Response::new(boxed))
    }
}

fn map_http_error(error: crate::http::error::HttpError) -> Status {
    use crate::http::error::HttpError;
    match error {
        HttpError::BadRequest(message) => Status::invalid_argument(message),
        HttpError::NotFound(message) => Status::not_found(message),
        HttpError::Conflict(message) => Status::already_exists(message),
        HttpError::Unauthorized(message) => Status::unauthenticated(message),
        HttpError::Forbidden(message) => Status::permission_denied(message),
        HttpError::InternalError(message) => Status::internal(message),
    }
}
