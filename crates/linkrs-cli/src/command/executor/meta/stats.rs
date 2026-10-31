use crate::command::executor::CommandExecutor;
use crate::session::SessionManager;
use crate::utils::error::{CliError, Result};

fn session_id(session_mgr: &SessionManager) -> Result<i64> {
    session_mgr
        .session()
        .map(|s| s.session_id)
        .ok_or(CliError::NotConnected)
}

pub(crate) fn pretty(value: &serde_json::Value) -> String {
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
    session_mgr.require_http("\\statistics")?;
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
        Some("search") => {
            let stats = session_mgr.client().get_search_statistics().await?;
            executor.write_output(&pretty(&stats))?;
        }
        Some("freeze") => {
            let stats = session_mgr.client().get_freeze_statistics().await?;
            executor.write_output(&pretty(&stats))?;
        }
        Some("freeze trigger") | Some("freeze run") => {
            let result = session_mgr.client().trigger_freeze().await?;
            executor.write_output(&pretty(&result))?;
        }
        Some("migration") => {
            let stats = session_mgr.client().get_migration_statistics().await?;
            executor.write_output(&pretty(&stats))?;
        }
        Some(other) if other.starts_with("profile ") => {
            let trace_id = other["profile ".len()..].trim();
            if trace_id.is_empty() {
                return Err(CliError::InvalidValue(
                    "Usage: \\statistics profile <trace_id>".to_string(),
                ));
            }
            let profile = session_mgr
                .client()
                .get_query_profile_detail(trace_id)
                .await?;
            let json = serde_json::to_value(&profile).unwrap_or(serde_json::Value::Null);
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

/// Remote liveness/readiness probe (`\\health`).
///
/// Unlike `\\status` (database overview), this hits the HTTP health
/// endpoints directly: `/v1/health` plus `/v1/health/live` and
/// `/v1/health/ready` for k8s-style tri-state. Embedded mode short-circuits
/// to healthy since there is no network service to probe.
pub async fn execute_health(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    if session_mgr.is_embedded() {
        let health = session_mgr.embedded_health()?;
        executor.write_output(&pretty(&health))?;
        return Ok(true);
    }
    session_mgr.require_http("\\health")?;
    let client = session_mgr.client();
    let health = client.health_detail().await?;
    let live = client.live_check().await?;
    let ready = client.ready_check().await?;
    let report = serde_json::json!({
        "health": health,
        "live": live,
        "ready": ready,
    });
    executor.write_output(&pretty(&report))?;
    Ok(true)
}

/// Prometheus metrics dump (`\\metrics`).
///
/// Prints the raw text exposition: embedded mode renders it in-process,
/// remote mode fetches `/metrics` over HTTP.
pub async fn execute_metrics(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let text = if session_mgr.is_embedded() {
        session_mgr.embedded_metrics()?
    } else {
        session_mgr.require_http("\\metrics")?;
        session_mgr.client().metrics().await?
    };
    executor.write_output(&text)?;
    Ok(true)
}
