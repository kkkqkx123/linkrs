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

/// Known configuration sections: the module vocabulary for the
/// `SHOW CONFIGS` / `UPDATE CONFIGS` query statements.
pub(crate) const CONFIG_SECTIONS: &[&str] = &[
    "database",
    "transaction",
    "log",
    "auth",
    "bootstrap",
    "optimizer",
    "monitoring",
];

/// Settable keys per section, mirroring the read/write tables above.
pub(crate) fn section_keys(section: &str) -> Option<&'static [&'static str]> {
    match section {
        "database" => Some(&["host", "port", "storage_path", "max_connections"]),
        "transaction" => Some(&[
            "default_timeout",
            "max_concurrent_transactions",
            "auto_commit",
        ]),
        "log" => Some(&["level", "dir", "file", "max_file_size", "max_files"]),
        "auth" => Some(&[
            "enable_authorize",
            "failed_login_attempts",
            "session_idle_timeout_secs",
            "force_change_default_password",
            "default_username",
            "bcrypt_cost",
        ]),
        "bootstrap" => Some(&[
            "auto_create_default_space",
            "default_space_name",
            "single_user_mode",
        ]),
        "optimizer" => Some(&[
            "max_iteration_rounds",
            "max_exploration_rounds",
            "enable_cost_model",
            "enable_multi_plan",
            "enable_property_pruning",
            "enable_adaptive_iteration",
            "stable_threshold",
            "min_iteration_rounds",
            "statistics_sample_limit",
            "statistics_min_epoch_delta",
            "storage_cost_profile",
            "space_cost_profiles",
        ]),
        "monitoring" => Some(&["enabled", "memory_cache_size", "slow_query_threshold_ms"]),
        _ => None,
    }
}

/// Normalize a statement module to a configuration section.
///
/// `None` means all sections. Unknown modules fail loudly with the valid
/// vocabulary instead of silently matching nothing.
pub(crate) fn resolve_config_section(module: Option<&str>) -> Result<Option<String>, String> {
    match module {
        None => Ok(None),
        Some(raw) => {
            let normalized = raw.trim().to_lowercase();
            if CONFIG_SECTIONS.contains(&normalized.as_str()) {
                Ok(Some(normalized))
            } else {
                Err(format!(
                    "unknown configuration module '{raw}': expected one of {}",
                    CONFIG_SECTIONS.join(", ")
                ))
            }
        }
    }
}

/// Resolve the target section/key of an `UPDATE CONFIGS` intent.
///
/// An explicit module must name a known section. Without a module the key
/// is looked up across sections: exactly one owner is required, so unknown
/// keys and ambiguous keys both fail with an actionable message.
fn resolve_config_key(module: Option<&str>, name: &str) -> Result<(String, String), String> {
    if let Some(section) = resolve_config_section(module)? {
        return Ok((section, name.to_string()));
    }
    let mut owners = Vec::new();
    for section in CONFIG_SECTIONS {
        if section_keys(section).is_some_and(|keys| keys.contains(&name)) {
            owners.push(*section);
        }
    }
    match owners.as_slice() {
        [] => Err(format!(
            "unknown configuration key '{name}': qualify it with a module (one of {})",
            CONFIG_SECTIONS.join(", ")
        )),
        [section] => Ok((section.to_string(), name.to_string())),
        _ => Err(format!(
            "ambiguous configuration key '{name}': present in {} — qualify it with a module",
            owners.join(", ")
        )),
    }
}

/// Render a configuration value for display rows: plain strings stay bare,
/// anything else renders as compact JSON.
fn display_config_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        _ => value.to_string(),
    }
}

/// Resolve a `SHOW CONFIGS` intent against live configuration.
///
/// Returns `section / key / value / requires_restart` rows, restricted to
/// one section when the statement names a module.
pub(crate) fn resolve_show_configs(
    config: &crate::config::Config,
    module: Option<&str>,
) -> Result<graphdb_core::DataSet, String> {
    let section = resolve_config_section(module)?;
    let sections: Vec<&str> = match section.as_deref() {
        Some(name) => vec![name],
        None => CONFIG_SECTIONS.to_vec(),
    };
    let mut rows = Vec::new();
    for section in sections {
        let keys = section_keys(section).ok_or_else(|| {
            format!("unknown configuration module '{section}'")
        })?;
        for key in keys {
            rows.push(vec![
                graphdb_core::Value::string(section),
                graphdb_core::Value::string(*key),
                graphdb_core::Value::string(display_config_value(&get_config_value(
                    config, section, key,
                ))),
                graphdb_core::Value::Bool(is_restart_required(section, key)),
            ]);
        }
    }
    Ok(graphdb_core::DataSet::from_rows(
        rows,
        vec![
            "section".to_string(),
            "key".to_string(),
            "value".to_string(),
            "requires_restart".to_string(),
        ],
    ))
}

/// Apply an `UPDATE CONFIGS` intent to live configuration.
///
/// Reuses the typed validation, restart detection, persistence, and
/// rollback of the management endpoints instead of a second write path.
/// Returns a one-row receipt (`updated / requires_restart / persisted`).
pub(crate) fn apply_config_update_intent(
    store: &parking_lot::RwLock<crate::config::Config>,
    config_path: Option<&std::path::Path>,
    module: Option<&str>,
    name: &str,
    value: &graphdb_core::Value,
) -> Result<graphdb_core::DataSet, String> {
    let (section, key) = resolve_config_key(module, name)?;
    let json = crate::value::to_json(value.clone());
    let (requires_restart, persisted) =
        apply_config_update(store, config_path, &section, &key, &json)?;
    Ok(graphdb_core::DataSet::from_rows(
        vec![vec![
            graphdb_core::Value::string(format!("{section}.{key}")),
            graphdb_core::Value::Bool(requires_restart),
            graphdb_core::Value::Bool(persisted),
        ]],
        vec![
            "updated".to_string(),
            "requires_restart".to_string(),
            "persisted".to_string(),
        ],
    ))
}

/// Resolve configuration intents carried by a query result.
///
/// `ConfigUpdate` intents are applied to the live store and replaced by
/// their receipt; `ShowConfigs` intents are replaced by live rows. Any
/// other result passes through untouched, preserving metadata.
pub(crate) fn resolve_query_config_intent(
    result: graphdb_api::api_core::QueryResult,
    store: &parking_lot::RwLock<crate::config::Config>,
    config_path: Option<&std::path::Path>,
) -> Result<graphdb_api::api_core::QueryResult, String> {
    use graphdb_api::api_core::QueryResult;
    let graphdb_api::api_core::QueryResult {
        execution,
        metadata,
    } = result;
    let execution = match execution {
        graphdb_query::executor::base::ExecutionResult::ConfigUpdate {
            module,
            name,
            value,
        } => graphdb_query::executor::base::ExecutionResult::DataSet {
            data: apply_config_update_intent(
                store,
                config_path,
                module.as_deref(),
                &name,
                &value,
            )?,
        },
        graphdb_query::executor::base::ExecutionResult::ShowConfigs { module } => {
            let config = store.read();
            graphdb_query::executor::base::ExecutionResult::DataSet {
                data: resolve_show_configs(&config, module.as_deref())?,
            }
        }
        other => other,
    };
    Ok(QueryResult::new(execution, metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn test_store() -> Arc<RwLock<crate::config::Config>> {
        Arc::new(RwLock::new(crate::config::Config::default()))
    }

    #[test]
    fn test_resolve_config_section_vocabulary() {
        assert_eq!(resolve_config_section(None).unwrap(), None);
        assert_eq!(
            resolve_config_section(Some("DATABASE")).unwrap(),
            Some("database".to_string())
        );
        let error = resolve_config_section(Some("storage")).unwrap_err();
        assert!(
            error.contains("unknown configuration module"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_apply_intent_updates_live_config() {
        let store = test_store();
        let receipt = apply_config_update_intent(
            &store,
            None,
            Some("database"),
            "max_connections",
            &graphdb_core::Value::Int(512),
        )
        .expect("valid update should apply");
        assert_eq!(
            receipt.col_names,
            vec![
                "updated".to_string(),
                "requires_restart".to_string(),
                "persisted".to_string()
            ]
        );
        assert_eq!(
            store.read().common.database.max_connections, 512,
            "live config should reflect the applied intent"
        );
        // Unqualified keys resolve when exactly one section owns them.
        apply_config_update_intent(
            &store,
            None,
            None,
            "max_connections",
            &graphdb_core::Value::Int(256),
        )
        .expect("unqualified unique key should resolve");
        assert_eq!(store.read().common.database.max_connections, 256);
    }

    #[test]
    fn test_apply_intent_rejects_unknown_key() {
        let store = test_store();
        let error = apply_config_update_intent(
            &store,
            None,
            Some("database"),
            "no_such_key",
            &graphdb_core::Value::Int(1),
        )
        .unwrap_err();
        assert!(
            error.contains("unknown configuration key"),
            "unexpected error: {error}"
        );
        let error = apply_config_update_intent(
            &store,
            None,
            Some("no_such_module"),
            "max_connections",
            &graphdb_core::Value::Int(1),
        )
        .unwrap_err();
        assert!(
            error.contains("unknown configuration module"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_apply_intent_rejects_type_mismatch() {
        let store = test_store();
        let before = store.read().common.database.max_connections;
        let error = apply_config_update_intent(
            &store,
            None,
            Some("database"),
            "max_connections",
            &graphdb_core::Value::string("not-a-number"),
        )
        .unwrap_err();
        assert!(error.contains("invalid value"), "unexpected error: {error}");
        assert_eq!(
            store.read().common.database.max_connections, before,
            "failed update must not mutate live config"
        );
    }

    #[test]
    fn test_resolve_show_configs_lists_live_values() {
        let store = test_store();
        store.write().common.database.max_connections = 777;
        let rows = resolve_show_configs(&store.read(), None).expect("listing should succeed");
        assert_eq!(
            rows.col_names,
            vec![
                "section".to_string(),
                "key".to_string(),
                "value".to_string(),
                "requires_restart".to_string()
            ]
        );
        let entry = rows
            .rows
            .iter()
            .find(|row| {
                row.first().map(|v| v == &graphdb_core::Value::string("database"))
                    .unwrap_or(false)
                    && row.get(1).map(|v| v == &graphdb_core::Value::string("max_connections"))
                        .unwrap_or(false)
            })
            .expect("database.max_connections row should be listed");
        assert_eq!(
            entry.get(2),
            Some(&graphdb_core::Value::string("777")),
            "listing should reflect live configuration"
        );

        let scoped = resolve_show_configs(&store.read(), Some("database"))
            .expect("scoped listing should succeed");
        assert!(
            scoped
                .rows
                .iter()
                .all(|row| row.first() == Some(&graphdb_core::Value::string("database"))),
            "scoped listing should only contain the requested section"
        );
        let error = resolve_show_configs(&store.read(), Some("storage")).unwrap_err();
        assert!(
            error.contains("unknown configuration module"),
            "unexpected error: {error}"
        );
    }
}
