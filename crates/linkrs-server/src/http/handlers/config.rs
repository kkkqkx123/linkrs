//! Configuration Management HTTP Processor

use axum::{
    extract::{Extension, Json, Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::http::error::HttpError;
use crate::http::state::AppState;
use crate::services::config::{apply_config_update, get_config_value};
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

#[utoipa::path(
    get,
    path = "/v1/config",
    tag = "Config",
    responses(
        (status = 200, body = serde_json::Value, description = "Current configuration"),
        (status = 500, description = "Internal error")
    )
)]
/// Get current configuration
pub async fn get<
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
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let config = state.server.get_config();

    Ok(JsonResponse(serde_json::json!({
        "database": {
            "host": config.common.database.host,
            "port": config.common.database.port,
            "storage_path": config.common.database.storage_path,
            "max_sessions": config.common.database.max_sessions,
        },
        "transaction": {
            "default_timeout": config.common.transaction.default_timeout,
            "max_concurrent_transactions": config.common.transaction.max_concurrent_transactions,
            "auto_commit": config.common.transaction.auto_commit,
        },
        "log": {
            "level": config.common.log.level,
            "dir": config.common.log.dir,
            "basename": config.common.log.basename,
            "max_file_size_mb": config.common.log.max_file_size_mb,
            "max_files": config.common.log.max_files,
            "stdout": config.common.log.stdout,
            "json_format": config.common.log.json_format,
        },
        "auth": {
            "enable_authorize": config.server.auth.enable_authorize,
            "failed_login_attempts": config.server.auth.failed_login_attempts,
            "session_idle_timeout_secs": config.server.auth.session_idle_timeout_secs,
            "force_change_default_password": config.server.auth.force_change_default_password,
            "default_username": config.server.auth.default_username,
            "bcrypt_cost": config.server.auth.bcrypt_cost,
        },
        "bootstrap": {
            "auto_create_default_space": config.server.bootstrap.auto_create_default_space,
            "default_space_name": config.server.bootstrap.default_space_name,
            "single_user_mode": config.server.bootstrap.single_user_mode,
        },
        "optimizer": {
            "max_iteration_rounds": config.common.optimizer.max_iteration_rounds,
            "max_exploration_rounds": config.common.optimizer.max_exploration_rounds,
            "enable_cost_model": config.common.optimizer.enable_cost_model,
            "enable_multi_plan": config.common.optimizer.enable_multi_plan,
            "enable_property_pruning": config.common.optimizer.enable_property_pruning,
            "enable_adaptive_iteration": config.common.optimizer.enable_adaptive_iteration,
            "stable_threshold": config.common.optimizer.stable_threshold,
            "min_iteration_rounds": config.common.optimizer.min_iteration_rounds,
            "statistics_sample_limit": config.common.optimizer.statistics_sample_limit,
            "statistics_min_epoch_delta": config.common.optimizer.statistics_min_epoch_delta,
            "storage_cost_profile": config.common.optimizer.storage_cost_profile,
            "space_cost_profiles": config.common.optimizer.space_cost_profiles,
        },
        "monitoring": {
            "enabled": config.common.monitoring.enabled,
            "memory_cache_size": config.common.monitoring.memory_cache_size,
            "slow_query_threshold_ms": config.common.monitoring.slow_query_threshold_ms,
        },
    })))
}

#[utoipa::path(
    put,
    path = "/v1/config",
    tag = "Config",
    request_body = serde_json::Value,
    responses(
        (status = 200, body = serde_json::Value, description = "Configuration update receipt"),
        (status = 500, description = "Internal error")
    )
)]
/// Update configuration (hot update)
pub async fn update<
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
    Json(request): Json<serde_json::Value>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let store = state.server.config_store();
    let config_path = state.server.get_config_path();
    let mut updated = Vec::new();
    let mut requires_restart = Vec::new();
    let mut persisted = true;

    if let Some(sections) = request.as_object() {
        for (section, values) in sections {
            if let Some(values_obj) = values.as_object() {
                for (key, value) in values_obj {
                    let full_key = format!("{section}.{key}");
                    match apply_config_update(&store, config_path.as_deref(), section, key, value) {
                        Ok((restart, wrote)) => {
                            updated.push(full_key);
                            if restart {
                                requires_restart.push(format!("{section}.{key}"));
                            }
                            persisted = persisted && wrote;
                        }
                        Err(e) => return Err(HttpError::bad_request(e)),
                    }
                }
            }
        }
    }

    Ok(JsonResponse(serde_json::json!({
        "updated": updated,
        "requires_restart": requires_restart,
        "persisted": persisted,
        "message": if persisted {
            "Configuration updated and persisted; restart-required keys take effect after restart"
        } else {
            "Configuration updated in memory only (no config file retained); restart to reload from file"
        },
    })))
}

#[utoipa::path(
    get,
    path = "/v1/config/{section}/{key}",
    tag = "Config",
    params(
        ("section" = String, Path, description = "Configuration section"),
        ("key" = String, Path, description = "Configuration key")
    ),
    responses(
        (status = 200, body = serde_json::Value, description = "Configuration item"),
        (status = 500, description = "Internal error")
    )
)]
/// Getting Configuration Items
pub async fn get_key<
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
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let config = state.server.get_config();
    let value = get_config_value(&config, &section, &key);

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": value,
    })))
}

#[utoipa::path(
    put,
    path = "/v1/config/{section}/{key}",
    tag = "Config",
    params(
        ("section" = String, Path, description = "Configuration section"),
        ("key" = String, Path, description = "Configuration key")
    ),
    request_body = UpdateConfigRequest,
    responses(
        (status = 200, body = serde_json::Value, description = "Configuration item updated"),
        (status = 500, description = "Internal error")
    )
)]
/// Updating Configuration Items
pub async fn update_key<
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
    Path((section, key)): Path<(String, String)>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let store = state.server.config_store();
    let config_path = state.server.get_config_path();
    let (requires_restart, persisted) = apply_config_update(
        &store,
        config_path.as_deref(),
        &section,
        &key,
        &request.value,
    )
    .map_err(HttpError::bad_request)?;

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": request.value,
        "requires_restart": requires_restart,
        "persisted": persisted,
        "message": if requires_restart {
            "Configuration item updated; restart required to take effect"
        } else if persisted {
            "Configuration item updated and persisted"
        } else {
            "Configuration item updated in memory only (no config file retained)"
        },
    })))
}

#[utoipa::path(
    delete,
    path = "/v1/config/{section}/{key}",
    tag = "Config",
    params(
        ("section" = String, Path, description = "Configuration section"),
        ("key" = String, Path, description = "Configuration key")
    ),
    responses(
        (status = 200, body = serde_json::Value, description = "Configuration reset to default"),
        (status = 500, description = "Internal error")
    )
)]
/// Reset configuration items to default values
pub async fn reset_key<
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
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    super::authz::require_admin_session(&state, session_id)?;
    let default_config = crate::config::Config::default();
    let default_value = get_config_value(&default_config, &section, &key);
    let store = state.server.config_store();
    let config_path = state.server.get_config_path();
    let (requires_restart, persisted) = apply_config_update(
        &store,
        config_path.as_deref(),
        &section,
        &key,
        &default_value,
    )
    .map_err(HttpError::bad_request)?;

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": default_value,
        "requires_restart": requires_restart,
        "persisted": persisted,
        "message": "Configuration reset to default value",
    })))
}

/// Update Configuration Request
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateConfigRequest {
    pub value: serde_json::Value,
}
