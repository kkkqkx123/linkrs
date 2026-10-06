use crate::command::executor::CommandExecutor;
use crate::command::parser::types::ConfigAction;
use crate::session::manager::SessionManager;
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
    }
    Ok(true)
}
