use crate::command::executor::CommandExecutor;
use crate::command::parser::types::ConfigAction;
use crate::session::SessionManager;
use crate::utils::error::Result;

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn parse_value(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return serde_json::Value::String(String::new());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

pub async fn execute_config(
    executor: &mut CommandExecutor,
    action: &ConfigAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\config")?;
    match action {
        ConfigAction::Show { section } => {
            let config = session_mgr.client().get_config().await?;
            let output = match section {
                Some(name) => config.get(name).cloned().unwrap_or(serde_json::Value::Null),
                None => config,
            };
            executor.write_output(&pretty(&output))?;
        }
        ConfigAction::Get { section, key } => {
            let item = session_mgr.client().get_config_key(section, key).await?;
            executor.write_output(&pretty(&item))?;
        }
        ConfigAction::Set {
            section,
            key,
            value,
        } => {
            let json = parse_value(value);
            let receipt = session_mgr
                .client()
                .update_config(section, key, json)
                .await?;
            executor.write_output(&pretty(&receipt))?;
        }
        ConfigAction::Reset { section, key } => {
            let receipt = session_mgr.client().reset_config(section, key).await?;
            executor.write_output(&pretty(&receipt))?;
        }
        ConfigAction::Bulk { payload } => {
            let raw = payload.trim();
            let json_text = if let Some(path) = raw.strip_prefix('@') {
                std::fs::read_to_string(path.trim()).map_err(|e| {
                    crate::utils::error::CliError::command(format!(
                        "failed to read bulk config file '{}': {e}",
                        path.trim()
                    ))
                })?
            } else {
                raw.to_string()
            };
            let updates: serde_json::Value = serde_json::from_str(&json_text).map_err(|e| {
                crate::utils::error::CliError::command(format!(
                    "invalid bulk config JSON object: {e}"
                ))
            })?;
            if !updates.is_object() {
                return Err(crate::utils::error::CliError::command(
                    "bulk config payload must be a JSON object of section objects".to_string(),
                ));
            }
            let receipt = session_mgr.client().bulk_update_config(updates).await?;
            executor.write_output(&pretty(&receipt))?;
        }
    }
    Ok(true)
}
