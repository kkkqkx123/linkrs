use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::{CliError, Result};

fn session_id(session_mgr: &SessionManager) -> Result<i64> {
    session_mgr
        .session()
        .map(|s| s.session_id)
        .ok_or(CliError::NotConnected)
}

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub async fn execute_statistics(
    executor: &mut CommandExecutor,
    target: Option<&str>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let normalized = target.map(|t| {
        if t == "db" {
            "database"
        } else if t == "query" {
            "queries"
        } else {
            t
        }
    });
    match normalized {
        None | Some("overview") => {
            let overview = session_mgr.client().get_overview().await?;
            executor.write_output(&pretty(&overview))?;
        }
        Some("system") => {
            let system = session_mgr.client().get_system_statistics().await?;
            executor.write_output(&pretty(&system))?;
        }
        Some("database") => {
            let db = session_mgr.client().get_database_statistics().await?;
            let json = serde_json::to_value(&db).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        Some("queries") => {
            let qs = session_mgr.client().get_query_statistics().await?;
            let json = serde_json::to_value(&qs).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        Some("session") => {
            let sid = session_id(session_mgr)?;
            let stats = session_mgr.client().get_session_statistics(sid).await?;
            let json = serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        Some(other) => {
            return Err(CliError::InvalidValue(format!(
                "Unknown statistics target: {}",
                other
            )));
        }
    }
    Ok(true)
}

pub async fn execute_status(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    execute_statistics(executor, Some("overview"), session_mgr).await
}
