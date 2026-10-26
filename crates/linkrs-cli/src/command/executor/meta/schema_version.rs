use crate::command::executor::CommandExecutor;
use crate::session::SessionManager;
use crate::utils::error::Result;

pub async fn execute_schema_versions(
    executor: &mut CommandExecutor,
    space: &str,
    label: &str,
    is_edge: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\schema versions")?;
    let result = session_mgr
        .client()
        .get_schema_version_history(space, label, is_edge)
        .await?;
    executor.write_output(&crate::command::executor::meta::stats::pretty(&result))?;
    Ok(true)
}

pub async fn execute_schema_changes(
    executor: &mut CommandExecutor,
    space: &str,
    label: &str,
    from: u64,
    to: u64,
    is_edge: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\schema changes")?;
    let result = session_mgr
        .client()
        .get_schema_changes(space, label, from, to, is_edge)
        .await?;
    executor.write_output(&crate::command::executor::meta::stats::pretty(&result))?;
    Ok(true)
}

pub async fn execute_schema_breaking(
    executor: &mut CommandExecutor,
    space: &str,
    label: &str,
    from: u64,
    to: u64,
    is_edge: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\schema breaking")?;
    let result = session_mgr
        .client()
        .detect_schema_breaking_changes(space, label, from, to, is_edge)
        .await?;
    executor.write_output(&crate::command::executor::meta::stats::pretty(&result))?;
    Ok(true)
}
