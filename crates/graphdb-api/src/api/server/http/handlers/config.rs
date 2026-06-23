//! Configuration Management HTTP Processor

use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::api::server::http::{error::HttpError, state::AppState};
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};

/// Get current configuration
pub async fn get<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let config = state.server.get_config();

    Ok(JsonResponse(serde_json::json!({
        "database": {
            "host": config.common.database.host,
            "port": config.common.database.port,
            "storage_path": config.common.database.storage_path,
            "max_connections": config.common.database.max_connections,
        },
        "transaction": {
            "default_timeout": config.common.transaction.default_timeout,
            "max_concurrent_transactions": config.common.transaction.max_concurrent_transactions,
        },
        "log": {
            "level": config.common.log.level,
            "dir": config.common.log.dir,
            "file": config.common.log.file,
            "max_file_size": config.common.log.max_file_size,
            "max_files": config.common.log.max_files,
        },
        "auth": {
            "enable_authorize": config.server.auth.enable_authorize,
            "failed_login_attempts": config.server.auth.failed_login_attempts,
            "session_idle_timeout_secs": config.server.auth.session_idle_timeout_secs,
            "force_change_default_password": config.server.auth.force_change_default_password,
            "default_username": config.server.auth.default_username,
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
        },
        "monitoring": {
            "enabled": config.common.monitoring.enabled,
            "memory_cache_size": config.common.monitoring.memory_cache_size,
            "slow_query_threshold_ms": config.common.monitoring.slow_query_threshold_ms,
        },
    })))
}

/// Update configuration (hot update)
pub async fn update<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Json(request): Json<serde_json::Value>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let mut updated = Vec::new();
    let mut requires_restart = Vec::new();

    if let Some(sections) = request.as_object() {
        for (section, values) in sections {
            if let Some(values_obj) = values.as_object() {
                for (key, _value) in values_obj {
                    let full_key = format!("{}.{}", section, key);

                    if is_restart_required(section, key) {
                        requires_restart.push(full_key);
                    } else {
                        updated.push(full_key);
                    }
                }
            }
        }
    }

    Ok(JsonResponse(serde_json::json!({
        "updated": updated,
        "requires_restart": requires_restart,
        "message": "Configuration update received, some changes may require restart to take effect",
    })))
}

/// Getting Configuration Items
pub async fn get_key<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(state): State<AppState<S>>,
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let config = state.server.get_config();
    let value = get_config_value(config, &section, &key);

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": value,
    })))
}

/// Updating Configuration Items
pub async fn update_key<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Path((section, key)): Path<(String, String)>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let requires_restart = is_restart_required(&section, &key);

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": request.value,
        "requires_restart": requires_restart,
        "message": if requires_restart {
            "Configuration item updated, but restart required to take effect"
        } else {
            "Configuration item updated"
        },
    })))
}

/// Reset configuration items to default values
pub async fn reset_key<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + Send
        + Sync
        + 'static,
>(
    State(_state): State<AppState<S>>,
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
    let default_config = crate::config::Config::default();
    let default_value = get_config_value(&default_config, &section, &key);

    Ok(JsonResponse(serde_json::json!({
        "section": section,
        "key": key,
        "value": default_value,
        "message": "Configuration reset to default value",
    })))
}

/// Update Configuration Request
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    pub value: serde_json::Value,
}

/// Getting configuration values
fn get_config_value(config: &crate::config::Config, section: &str, key: &str) -> serde_json::Value {
    match section {
        "database" => match key {
            "host" => serde_json::json!(config.common.database.host),
            "port" => serde_json::json!(config.common.database.port),
            "storage_path" => serde_json::json!(config.common.database.storage_path),
            "max_connections" => serde_json::json!(config.common.database.max_connections),
            _ => serde_json::Value::Null,
        },
        "transaction" => match key {
            "default_timeout" => serde_json::json!(config.common.transaction.default_timeout),
            "max_concurrent_transactions" => {
                serde_json::json!(config.common.transaction.max_concurrent_transactions)
            }
            _ => serde_json::Value::Null,
        },
        "log" => match key {
            "level" => serde_json::json!(config.common.log.level),
            "dir" => serde_json::json!(config.common.log.dir),
            "file" => serde_json::json!(config.common.log.file),
            "max_file_size" => serde_json::json!(config.common.log.max_file_size),
            "max_files" => serde_json::json!(config.common.log.max_files),
            _ => serde_json::Value::Null,
        },
        "auth" => match key {
            "enable_authorize" => serde_json::json!(config.server.auth.enable_authorize),
            "failed_login_attempts" => serde_json::json!(config.server.auth.failed_login_attempts),
            "session_idle_timeout_secs" => {
                serde_json::json!(config.server.auth.session_idle_timeout_secs)
            }
            "force_change_default_password" => {
                serde_json::json!(config.server.auth.force_change_default_password)
            }
            "default_username" => serde_json::json!(config.server.auth.default_username),
            _ => serde_json::Value::Null,
        },
        "bootstrap" => match key {
            "auto_create_default_space" => {
                serde_json::json!(config.server.bootstrap.auto_create_default_space)
            }
            "default_space_name" => serde_json::json!(config.server.bootstrap.default_space_name),
            "single_user_mode" => serde_json::json!(config.server.bootstrap.single_user_mode),
            _ => serde_json::Value::Null,
        },
        "optimizer" => match key {
            "max_iteration_rounds" => {
                serde_json::json!(config.common.optimizer.max_iteration_rounds)
            }
            "max_exploration_rounds" => {
                serde_json::json!(config.common.optimizer.max_exploration_rounds)
            }
            "enable_cost_model" => serde_json::json!(config.common.optimizer.enable_cost_model),
            "enable_multi_plan" => serde_json::json!(config.common.optimizer.enable_multi_plan),
            "enable_property_pruning" => {
                serde_json::json!(config.common.optimizer.enable_property_pruning)
            }
            "enable_adaptive_iteration" => {
                serde_json::json!(config.common.optimizer.enable_adaptive_iteration)
            }
            "stable_threshold" => serde_json::json!(config.common.optimizer.stable_threshold),
            "min_iteration_rounds" => {
                serde_json::json!(config.common.optimizer.min_iteration_rounds)
            }
            _ => serde_json::Value::Null,
        },
        "monitoring" => match key {
            "enabled" => serde_json::json!(config.common.monitoring.enabled),
            "memory_cache_size" => serde_json::json!(config.common.monitoring.memory_cache_size),
            "slow_query_threshold_ms" => {
                serde_json::json!(config.common.monitoring.slow_query_threshold_ms)
            }
            _ => serde_json::Value::Null,
        },
        _ => serde_json::Value::Null,
    }
}

/// Check if the configuration item requires a reboot to take effect
fn is_restart_required(section: &str, key: &str) -> bool {
    match section {
        "database" => matches!(key, "host" | "port" | "storage_path" | "max_connections"),
        "transaction" => false,
        "log" => matches!(key, "dir" | "file"),
        "auth" => matches!(key, "default_username"),
        "bootstrap" => true,
        "optimizer" => false,
        "monitoring" => false,
        _ => false,
    }
}
