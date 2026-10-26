use crate::analysis::timing::QueryTimer;
use crate::command::executor::CommandExecutor;
use crate::session::SessionManager;
use crate::utils::error::{CliError, Result};

pub async fn execute_explain(
    executor: &mut CommandExecutor,
    query: &str,
    analyze: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let session_id = require_session_id(session_mgr)?;
    if analyze {
        // Analyze mode reports per-stage timings after actually running the
        // statement, so it goes through the normal execute path instead of
        // the plan-only endpoint.
        let result = session_mgr.execute_query(query).await?;
        let output = executor.formatter().format_result(&result);
        executor.write_output(&output)?;
        print_trace_portrait(executor, session_mgr, result.trace_id.as_deref()).await?;
    } else {
        session_mgr.require_http("\\explain")?;
        let result = session_mgr
            .client()
            .explain_query(query, session_id)
            .await?;
        let output = executor.formatter().format_result(&result);
        executor.write_output(&output)?;
    }
    Ok(true)
}

pub async fn execute_profile(
    executor: &mut CommandExecutor,
    query: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let mut timer = QueryTimer::new();
    let result = session_mgr.execute_query(query).await?;
    timer.record_phase("execution");

    let output = executor.formatter().format_result(&result);
    executor.write_output(&output)?;
    executor.write_output(&timer.format_phases())?;

    print_trace_portrait(executor, session_mgr, result.trace_id.as_deref()).await?;
    Ok(true)
}

/// Print the server-side query portrait for `trace_id` when available.
async fn print_trace_portrait(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
    trace_id: Option<&str>,
) -> Result<()> {
    let Some(trace_id) = trace_id else {
        return Ok(());
    };
    if session_mgr.is_embedded() {
        return Ok(());
    }
    let profile = session_mgr
        .client()
        .get_query_profile_detail(trace_id)
        .await?;
    let json = serde_json::to_value(&profile).unwrap_or(serde_json::Value::Null);
    executor.write_output(&crate::command::executor::meta::stats::pretty(&json))?;
    Ok(())
}

fn require_session_id(session_mgr: &SessionManager) -> Result<i64> {
    session_mgr
        .client()
        .session_id()
        .ok_or_else(|| CliError::session("Not connected".to_string()))
}

/// `\validate <query>` — server-side parse/binder check without execution.
pub async fn execute_validate(
    executor: &mut CommandExecutor,
    query: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\validate")?;
    let session_id = require_session_id(session_mgr)?;
    let response = session_mgr
        .client()
        .validate_query(query, session_id)
        .await?;
    let valid = response
        .get("valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if valid {
        let estimate = response.get("estimated_rows");
        if let Some(rows) = estimate.filter(|v| !v.is_null()) {
            executor.write_output(&format!("Valid. Estimated rows: {}", rows))?;
        } else {
            executor.write_output("Valid.")?;
        }
    } else {
        let message = response
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(no message)");
        executor.write_output(&format!("Invalid: {}", message))?;
    }
    Ok(true)
}
