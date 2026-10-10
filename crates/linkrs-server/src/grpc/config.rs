//! Live configuration handlers and wire mapping.

use std::collections::HashMap;

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
    pub(crate) async fn handle_get_config(
        &self,
        _request: Request<GetConfigRequest>,
    ) -> Result<Response<GetConfigResponse>, Status> {
        let config = self.app_state.server.get_config();
        Ok(Response::new(GetConfigResponse {
            config: build_config_map(&config),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_update_config(
        &self,
        request: Request<UpdateConfigRequest>,
    ) -> Result<Response<UpdateConfigResponse>, Status> {
        let req = request.into_inner();
        let value = proto_config_value_to_json(req.value);
        let store = self.app_state.server.config_store();
        let config_path = self.app_state.server.get_config_path();
        let (requires_restart, persisted) = crate::http::handlers::config::apply_config_update(
            &store,
            config_path.as_deref(),
            &req.section,
            &req.key,
            &value,
        )
        .map_err(Status::invalid_argument)?;
        Ok(Response::new(UpdateConfigResponse {
            success: true,
            error: String::new(),
            requires_restart,
            persisted,
        }))
    }

    pub(crate) async fn handle_get_config_key(
        &self,
        request: Request<GetConfigKeyRequest>,
    ) -> Result<Response<GetConfigKeyResponse>, Status> {
        let req = request.into_inner();
        let config = self.app_state.server.get_config();
        let value =
            crate::http::handlers::config::get_config_value(&config, &req.section, &req.key);
        Ok(Response::new(GetConfigKeyResponse {
            section: req.section,
            key: req.key,
            value_json: serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_bulk_update_config(
        &self,
        request: Request<BulkUpdateConfigRequest>,
    ) -> Result<Response<BulkUpdateConfigResponse>, Status> {
        let req = request.into_inner();
        let updates: serde_json::Value = serde_json::from_str(&req.updates_json)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let sections = updates.as_object().ok_or_else(|| {
            Status::invalid_argument("updates_json must be an object of section objects")
        })?;
        let store = self.app_state.server.config_store();
        let config_path = self.app_state.server.get_config_path();
        let mut updated = Vec::new();
        let mut requires_restart = Vec::new();
        let mut persisted = true;
        for (section, values) in sections {
            let values_obj = values.as_object().ok_or_else(|| {
                Status::invalid_argument(format!(
                    "section '{section}' must map to an object of key/value pairs"
                ))
            })?;
            for (key, value) in values_obj {
                match crate::http::handlers::config::apply_config_update(
                    &store,
                    config_path.as_deref(),
                    section,
                    key,
                    value,
                ) {
                    Ok((restart, wrote)) => {
                        updated.push(format!("{section}.{key}"));
                        if restart {
                            requires_restart.push(format!("{section}.{key}"));
                        }
                        persisted = persisted && wrote;
                    }
                    Err(e) => return Err(Status::invalid_argument(e)),
                }
            }
        }
        Ok(Response::new(BulkUpdateConfigResponse {
            success: true,
            updated_json: serde_json::to_string(&updated).unwrap_or_else(|_| "[]".to_string()),
            requires_restart_json: serde_json::to_string(&requires_restart)
                .unwrap_or_else(|_| "[]".to_string()),
            persisted,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_reset_config(
        &self,
        request: Request<ResetConfigRequest>,
    ) -> Result<Response<ResetConfigResponse>, Status> {
        let req = request.into_inner();
        let default_config = crate::config::Config::default();
        let value = crate::http::handlers::config::get_config_value(
            &default_config,
            &req.section,
            &req.key,
        );
        let store = self.app_state.server.config_store();
        let config_path = self.app_state.server.get_config_path();
        let (requires_restart, persisted) = crate::http::handlers::config::apply_config_update(
            &store,
            config_path.as_deref(),
            &req.section,
            &req.key,
            &value,
        )
        .map_err(Status::invalid_argument)?;
        Ok(Response::new(ResetConfigResponse {
            success: true,
            error: String::new(),
            requires_restart,
            persisted,
        }))
    }
}

/// Build the proto config map from the live server configuration.
///
/// The section/key shape mirrors the HTTP config endpoint; values convert
/// through JSON so numeric widths and enums never need manual casting.
pub(crate) fn build_config_map(
    config: &crate::config::Config,
) -> HashMap<String, super::proto::ConfigSection> {
    let snapshot = serde_json::json!({
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
        "http": {
            "cors_enabled": config.server.http.cors_enabled,
            "cors_allowed_origins": config.server.http.cors_allowed_origins,
        },
        "security": {
            "password_min_length": config.server.security.password_policy.min_length,
            "password_require_uppercase": config.server.security.password_policy.require_uppercase,
            "password_require_lowercase": config.server.security.password_policy.require_lowercase,
            "password_require_digit": config.server.security.password_policy.require_digit,
            "password_require_special": config.server.security.password_policy.require_special,
            "password_max_age_days": config.server.security.password_policy.max_age_days,
            "password_history_size": config.server.security.password_policy.history_size,
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
            "storage_cost_profile": format!("{:?}", config.common.optimizer.storage_cost_profile),
            "space_cost_profiles": format!("{:?}", config.common.optimizer.space_cost_profiles),
        },
        "monitoring": {
            "enabled": config.common.monitoring.enabled,
            "memory_cache_size": config.common.monitoring.memory_cache_size,
            "slow_query_threshold_ms": config.common.monitoring.slow_query_threshold_ms,
        },
    });
    snapshot
        .as_object()
        .map(|sections| {
            sections
                .iter()
                .map(|(section, values)| {
                    let entries = values
                        .as_object()
                        .map(|keys| {
                            keys.iter()
                                .map(|(key, value)| (key.clone(), json_to_config_value(value)))
                                .collect()
                        })
                        .unwrap_or_default();
                    (
                        section.clone(),
                        super::proto::ConfigSection { values: entries },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn json_to_config_value(value: &serde_json::Value) -> super::proto::ConfigValue {
    use super::proto::config_value::Value as ConfigPrimitive;
    let primitive = match value {
        serde_json::Value::String(s) => Some(ConfigPrimitive::StringValue(s.clone())),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(ConfigPrimitive::IntValue(i))
            } else if let Some(u) = n.as_u64() {
                Some(ConfigPrimitive::IntValue(u as i64))
            } else {
                n.as_f64().map(ConfigPrimitive::DoubleValue)
            }
        }
        serde_json::Value::Bool(b) => Some(ConfigPrimitive::BoolValue(*b)),
        _ => None,
    };
    super::proto::ConfigValue { value: primitive }
}

/// Proto config value back to JSON for the typed config setter.
pub(crate) fn proto_config_value_to_json(
    value: Option<super::proto::ConfigValue>,
) -> serde_json::Value {
    use super::proto::config_value::Value as ConfigPrimitive;
    match value.and_then(|v| v.value) {
        Some(ConfigPrimitive::StringValue(s)) => serde_json::Value::String(s),
        Some(ConfigPrimitive::IntValue(i)) => serde_json::Value::from(i),
        Some(ConfigPrimitive::DoubleValue(f)) => serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Some(ConfigPrimitive::BoolValue(b)) => serde_json::Value::Bool(b),
        None => serde_json::Value::Null,
    }
}
