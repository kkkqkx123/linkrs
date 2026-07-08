use crate::analysis::timing::QueryTimer;
use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub async fn execute_explain(
    executor: &mut CommandExecutor,
    query: &str,
    analyze: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    if analyze {
        executor.write_output("EXPLAIN ANALYZE is not yet implemented. Showing plan only.")?;
    }
    let result = session_mgr
        .execute_query(&format!("EXPLAIN {}", query))
        .await?;
    let output = executor.formatter().format_result(&result);
    executor.write_output(&output)?;
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
    Ok(true)
}
