//! Configuration Management HTTP Processor

use axum::{
    extract::{Json, Path, State},
    response::Json as JsonResponse,
};
use serde::Deserialize;
use serde_json;

use crate::http::{error::HttpError, state::AppState};
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
            "auto_commit": config.common.transaction.auto_commit,
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
    Json(request): Json<serde_json::Value>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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
    Path((section, key)): Path<(String, String)>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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
    Path((section, key)): Path<(String, String)>,
) -> Result<JsonResponse<serde_json::Value>, HttpError> {
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

/// Getting configuration values
pub(crate) fn get_config_value(
    config: &crate::config::Config,
    section: &str,
    key: &str,
) -> serde_json::Value {
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
            "auto_commit" => serde_json::json!(config.common.transaction.auto_commit),
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
            "bcrypt_cost" => serde_json::json!(config.server.auth.bcrypt_cost),
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
            "statistics_sample_limit" => {
                serde_json::json!(config.common.optimizer.statistics_sample_limit)
            }
            "statistics_min_epoch_delta" => {
                serde_json::json!(config.common.optimizer.statistics_min_epoch_delta)
            }
            "storage_cost_profile" => {
                serde_json::json!(config.common.optimizer.storage_cost_profile)
            }
            "space_cost_profiles" => {
                serde_json::json!(config.common.optimizer.space_cost_profiles)
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
///
/// Only keys read live per use could apply without restart. Every other
/// section is snapshotted at construction (listener/storage bindings,
/// TransactionManager, logging init, PasswordAuthenticator/session timeouts,
/// bootstrap flags, optimizer engine, StatsManager), so updates to those
/// keys are persisted and visible to readers but take effect on restart.
fn is_restart_required(section: &str, key: &str) -> bool {
    match section {
        "database" => matches!(key, "host" | "port" | "storage_path" | "max_connections"),
        "transaction" => true,
        "log" => true,
        "auth" => true,
        "bootstrap" => true,
        "optimizer" => true,
        "monitoring" => true,
        _ => false,
    }
}

fn parse_value<T>(key_desc: &str, value: &serde_json::Value) -> Result<T, String>
where
    T: for<'de> serde::Deserialize<'de>,
{
    serde_json::from_value(value.clone())
        .map_err(|e| format!("invalid value for '{key_desc}': {e}"))
}

/// Apply one section/key/value to live config with type checking.
///
/// Returns whether a restart is required for the change to take effect.
/// Unknown keys are rejected; every known key is settable.
pub(crate) fn set_config_value(
    config: &mut crate::config::Config,
    section: &str,
    key: &str,
    value: &serde_json::Value,
) -> Result<bool, String> {
    let full_key = format!("{section}.{key}");
    match section {
        "database" => match key {
            "host" => config.common.database.host = parse_value(&full_key, value)?,
            "port" => config.common.database.port = parse_value(&full_key, value)?,
            "storage_path" => config.common.database.storage_path = parse_value(&full_key, value)?,
            "max_connections" => {
                config.common.database.max_connections = parse_value(&full_key, value)?
            }
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "transaction" => match key {
            "default_timeout" => {
                config.common.transaction.default_timeout = parse_value(&full_key, value)?
            }
            "max_concurrent_transactions" => {
                config.common.transaction.max_concurrent_transactions =
                    parse_value(&full_key, value)?
            }
            "auto_commit" => config.common.transaction.auto_commit = parse_value(&full_key, value)?,
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "log" => match key {
            "level" => config.common.log.level = parse_value(&full_key, value)?,
            "dir" => config.common.log.dir = parse_value(&full_key, value)?,
            "file" => config.common.log.file = parse_value(&full_key, value)?,
            "max_file_size" => config.common.log.max_file_size = parse_value(&full_key, value)?,
            "max_files" => config.common.log.max_files = parse_value(&full_key, value)?,
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "auth" => match key {
            "enable_authorize" => {
                config.server.auth.enable_authorize = parse_value(&full_key, value)?
            }
            "failed_login_attempts" => {
                config.server.auth.failed_login_attempts = parse_value(&full_key, value)?
            }
            "session_idle_timeout_secs" => {
                config.server.auth.session_idle_timeout_secs = parse_value(&full_key, value)?
            }
            "force_change_default_password" => {
                config.server.auth.force_change_default_password = parse_value(&full_key, value)?
            }
            "default_username" => {
                config.server.auth.default_username = parse_value(&full_key, value)?
            }
            "bcrypt_cost" => {
                let cost: u32 = parse_value(&full_key, value)?;
                if !(4..=31).contains(&cost) {
                    return Err(format!(
                        "invalid value for '{full_key}': out of range 4..=31"
                    ));
                }
                config.server.auth.bcrypt_cost = cost;
            }
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "bootstrap" => match key {
            "auto_create_default_space" => {
                config.server.bootstrap.auto_create_default_space = parse_value(&full_key, value)?
            }
            "default_space_name" => {
                config.server.bootstrap.default_space_name = parse_value(&full_key, value)?
            }
            "single_user_mode" => {
                config.server.bootstrap.single_user_mode = parse_value(&full_key, value)?
            }
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "optimizer" => match key {
            "max_iteration_rounds" => {
                config.common.optimizer.max_iteration_rounds = parse_value(&full_key, value)?
            }
            "max_exploration_rounds" => {
                config.common.optimizer.max_exploration_rounds = parse_value(&full_key, value)?
            }
            "enable_cost_model" => {
                config.common.optimizer.enable_cost_model = parse_value(&full_key, value)?
            }
            "enable_multi_plan" => {
                config.common.optimizer.enable_multi_plan = parse_value(&full_key, value)?
            }
            "enable_property_pruning" => {
                config.common.optimizer.enable_property_pruning = parse_value(&full_key, value)?
            }
            "enable_adaptive_iteration" => {
                config.common.optimizer.enable_adaptive_iteration = parse_value(&full_key, value)?
            }
            "stable_threshold" => {
                config.common.optimizer.stable_threshold = parse_value(&full_key, value)?
            }
            "min_iteration_rounds" => {
                config.common.optimizer.min_iteration_rounds = parse_value(&full_key, value)?
            }
            "statistics_sample_limit" => {
                config.common.optimizer.statistics_sample_limit = parse_value(&full_key, value)?
            }
            "statistics_min_epoch_delta" => {
                config.common.optimizer.statistics_min_epoch_delta = parse_value(&full_key, value)?
            }
            "storage_cost_profile" => {
                config.common.optimizer.storage_cost_profile = parse_value(&full_key, value)?
            }
            "space_cost_profiles" => {
                config.common.optimizer.space_cost_profiles = parse_value(&full_key, value)?
            }
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        "monitoring" => match key {
            "enabled" => config.common.monitoring.enabled = parse_value(&full_key, value)?,
            "memory_cache_size" => {
                config.common.monitoring.memory_cache_size = parse_value(&full_key, value)?
            }
            "slow_query_threshold_ms" => {
                config.common.monitoring.slow_query_threshold_ms = parse_value(&full_key, value)?
            }
            _ => return Err(format!("unknown configuration key '{full_key}'")),
        },
        _ => return Err(format!("unknown configuration key '{full_key}'")),
    }
    Ok(is_restart_required(section, key))
}

/// Apply one update to live config and persist it to the retained config
/// file when one exists.
///
/// Returns `(requires_restart, persisted)`. If persistence fails the live
/// change is rolled back so memory and disk never diverge.
pub(crate) fn apply_config_update(
    store: &parking_lot::RwLock<crate::config::Config>,
    config_path: Option<&std::path::Path>,
    section: &str,
    key: &str,
    value: &serde_json::Value,
) -> Result<(bool, bool), String> {
    let mut guard = store.write();
    let old = get_config_value(&guard, section, key);
    let requires_restart = set_config_value(&mut guard, section, key, value)?;
    let Some(path) = config_path else {
        return Ok((requires_restart, false));
    };
    if let Err(e) = guard.save(path) {
        let _ = set_config_value(&mut guard, section, key, &old);
        return Err(format!("failed to persist configuration: {e}"));
    }
    Ok((requires_restart, true))
}
