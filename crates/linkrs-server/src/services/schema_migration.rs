//! Schema migration read/dry-run services shared by both transports.

use tokio::task;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::authz::require_admin_session;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

/// Execute a migration plan in dry-run mode and render the preview report.
pub(crate) async fn dry_run_migration_core<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
    session_id: i64,
    plan_json: &str,
) -> Result<linkrs_wire::migration::MigrationExecuteResponse, HttpError> {
    let mut plan: linkrs_migration::MigrationPlan =
        serde_json::from_str(plan_json).map_err(|e| HttpError::BadRequest(e.to_string()))?;
    plan.dry_run = true;

    require_admin_session(state, session_id)?;
    let state = state.clone();
    let result = task::spawn_blocking(move || {
        let storage = state.server.get_storage();
        let mut storage_write = storage.write();
        let report = linkrs_migration::execute_migration_plan(&mut *storage_write, &plan)
            .map_err(|e| HttpError::InternalError(e.to_string()))?;

        Ok::<_, HttpError>(linkrs_wire::migration::MigrationExecuteResponse {
            success: report.success,
            steps_completed: report.steps_completed,
            rows_migrated: report.rows_migrated,
            errors: report.errors,
            preview: Some(true),
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    result
}

/// List applied versions and migration history for one label.
pub(crate) async fn migration_history_core<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
    session_id: i64,
    space: String,
    label: String,
    is_edge: bool,
) -> Result<linkrs_wire::migration::MigrationHistoryResponse, HttpError> {
    require_admin_session(state, session_id)?;
    let state = state.clone();
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
            .map(|r| serde_json::to_value(&r))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| HttpError::InternalError(e.to_string()))?;
        Ok::<_, HttpError>(linkrs_wire::migration::MigrationHistoryResponse {
            space,
            label,
            is_edge,
            applied_versions: versions,
            history,
        })
    })
    .await
    .map_err(|e| HttpError::InternalError(format!("Task execution failed: {}", e)))?;

    result
}

/// Report the latest applied version and history count for one label.
pub(crate) async fn migration_status_core<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    state: &AppState<S>,
    session_id: i64,
    space: String,
    label: String,
    is_edge: bool,
) -> Result<linkrs_wire::migration::MigrationStatusResponse, HttpError> {
    require_admin_session(state, session_id)?;
    let state = state.clone();
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
        Ok::<_, HttpError>(linkrs_wire::migration::MigrationStatusResponse {
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

    result
}
