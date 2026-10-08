//! Schema migration handlers.
//!
//! Covers migration plan generation, execution, rollback, dry run,
//! history and status queries.

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use linkrs_core::event_dispatch::EventSubscriptions;
use linkrs_wire::migration::{
    MigrationExecuteRequest, MigrationExecuteResponse, MigrationHistoryResponse,
    MigrationPlanQuery, MigrationPlanResponse, MigrationRollbackRequest, MigrationStatusResponse,
};
use tokio::task;

use super::version::parse_is_edge_param;
use crate::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use linkrs_migration::{
    generate_edge_plan_with_expand, generate_vertex_plan_with_expand, MigrationEvent,
};

// ==================== Migration ====================

#[utoipa::path(
    post,
    path = "/v1/migration/plan/{space}/{label}",
    tag = "Migration",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("from_version" = Option<u64>, Query, description = "Start schema version"),
        ("to_version" = Option<u64>, Query, description = "End schema version"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type"),
        ("expand_contract" = Option<bool>, Query, description = "Use expand-contract plan")
    ),
    responses(
        (status = 200, body = MigrationPlanResponse, description = "Migration plan"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn create_migration_plan<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<MigrationPlanQuery>,
) -> Result<JsonResponse<MigrationPlanResponse>, HttpError> {
    let from_version = query
        .require_from_version()
        .map_err(HttpError::BadRequest)?;
    let to_version = query.require_to_version().map_err(HttpError::BadRequest)?;
    let is_edge = query.is_edge.unwrap_or(false);
    let expand_contract = query.expand_contract.unwrap_or(false);

    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let plan = if is_edge {
            if expand_contract {
                generate_edge_plan_with_expand(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                    true,
                )
            } else {
                linkrs_migration::generate_edge_plan(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                )
            }
        } else {
            if expand_contract {
                generate_vertex_plan_with_expand(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                    true,
                )
            } else {
                linkrs_migration::generate_vertex_plan(
                    &*storage_read,
                    &space,
                    &label,
                    from_version,
                    to_version,
                )
            }
        }
        .map_err(|e| HttpError::InternalError(e.to_string()))?;

        let plan_json = serde_json::to_string(&plan).unwrap();
        let plan_value = serde_json::to_value(&plan).unwrap();
        Ok::<_, HttpError>(MigrationPlanResponse {
            plan: plan_value,
            plan_json,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    post,
    path = "/v1/migration/execute",
    tag = "Migration",
    request_body = MigrationExecuteRequest,
    responses(
        (status = 200, body = MigrationExecuteResponse, description = "Migration execution report"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn execute_migration<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(req): Json<MigrationExecuteRequest>,
) -> Result<JsonResponse<MigrationExecuteResponse>, HttpError> {
    let plan: linkrs_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;

    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let stats = state.server.get_stats_manager();
        let migration_config = state.server.migration_config();
        let gate = state.server.get_txn_manager().checkpoint_gate().clone();
        let start = std::time::Instant::now();
        stats.record_migration_start();
        let mut storage_write = storage.write();
        let sender = crate::http::handlers::migration_progress::get_or_create_sender(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        );
        let registry = EventSubscriptions::<MigrationEvent>::new();
        registry.add(crate::http::handlers::migration_progress::event_bridge(
            sender,
        ));
        let registry = std::sync::Arc::new(registry);
        let report = linkrs_api::migration_online::execute_online_migration(
            &mut *storage_write,
            &plan,
            &migration_config,
            &gate,
            Some(&registry),
        )
        .map_err(|e| HttpError::InternalError(e.to_string()));
        let elapsed = start.elapsed().as_millis() as u64;
        match &report {
            Ok(r) if r.success => {
                stats.record_migration_success(r.rows_migrated, elapsed);
            }
            Ok(_) => stats.record_migration_failure(elapsed),
            Err(_) => stats.record_migration_failure(elapsed),
        }
        let report = report?;
        Ok::<_, HttpError>(MigrationExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            preview: None,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    post,
    path = "/v1/migration/rollback",
    tag = "Migration",
    request_body = MigrationRollbackRequest,
    responses(
        (status = 200, body = MigrationExecuteResponse, description = "Migration rollback report"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn rollback_migration<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(req): Json<MigrationRollbackRequest>,
) -> Result<JsonResponse<MigrationExecuteResponse>, HttpError> {
    let plan: linkrs_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;

    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let migration_config = state.server.migration_config();
        let mut storage_write = storage.write();
        let sender = crate::http::handlers::migration_progress::get_or_create_sender(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        );
        let registry = EventSubscriptions::<MigrationEvent>::new();
        registry.add(crate::http::handlers::migration_progress::event_bridge(
            sender,
        ));
        let registry = std::sync::Arc::new(registry);
        let report = linkrs_migration::rollback_migration_with_options(
            &mut *storage_write,
            &plan,
            linkrs_migration::ExecuteOptions {
                config: Some(&migration_config),
                event_registry: Some(&registry),
                ..Default::default()
            },
        )
        .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(MigrationExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            preview: None,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    post,
    path = "/v1/migration/dry-run",
    tag = "Migration",
    request_body = MigrationExecuteRequest,
    responses(
        (status = 200, body = MigrationExecuteResponse, description = "Migration dry-run preview"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn dry_run_migration<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Json(req): Json<MigrationExecuteRequest>,
) -> Result<JsonResponse<MigrationExecuteResponse>, HttpError> {
    let mut plan: linkrs_migration::MigrationPlan =
        serde_json::from_str(&req.plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;
    plan.dry_run = true;

    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let mut storage_write = storage.write();
        let report = linkrs_migration::execute_migration_plan(&mut *storage_write, &plan)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(MigrationExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            preview: Some(true),
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    path = "/v1/migration/history/{space}/{label}",
    tag = "Migration",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, body = MigrationHistoryResponse, description = "Migration history"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn migration_history<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<MigrationHistoryResponse>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;
    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let versions = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let history: Vec<serde_json::Value> = history
            .into_iter()
            .map(|r| serde_json::to_value(&r).unwrap())
            .collect();
        Ok::<_, HttpError>(MigrationHistoryResponse {
            space,
            label,
            is_edge,
            applied_versions: versions,
            history,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}

#[utoipa::path(
    get,
    path = "/v1/migration/status/{space}/{label}",
    tag = "Migration",
    params(
        ("space" = String, Path, description = "Space name"),
        ("label" = String, Path, description = "Tag or edge type name"),
        ("is_edge" = Option<bool>, Query, description = "Whether the label is an edge type")
    ),
    responses(
        (status = 200, body = MigrationStatusResponse, description = "Migration status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn migration_status<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Extension(session_id): Extension<i64>,
    Path((space, label)): Path<(String, String)>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<JsonResponse<MigrationStatusResponse>, HttpError> {
    let is_edge = parse_is_edge_param(&query)?;
    super::super::authz::require_admin_session(&state, session_id)?;
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let storage_read = storage.read();
        let applied = storage_read
            .get_applied_versions(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let history = storage_read
            .list_migration_history(&space, &label, is_edge)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        let latest = applied.iter().max().copied().unwrap_or(0);
        Ok::<_, HttpError>(MigrationStatusResponse {
            space: Some(space),
            label: Some(label),
            is_edge: Some(is_edge),
            latest_applied_version: Some(latest),
            applied_versions: applied,
            history_count: history.len(),
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    Ok(JsonResponse(result?))
}
