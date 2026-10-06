use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::{CliError, Result};

fn session_id(session_mgr: &SessionManager) -> Result<i64> {
    session_mgr
        .session()
        .map(|s| s.session_id)
        .ok_or(CliError::NotConnected)
}

pub async fn execute_stream(
    executor: &mut CommandExecutor,
    query: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let sid = session_id(session_mgr)?;
    let substituted = session_mgr
        .session()
        .map(|s| s.substitute_variables(query))
        .transpose()?
        .unwrap_or_else(|| query.to_string());
    let result = session_mgr
        .client()
        .execute_query_stream(&substituted, sid)
        .await?;
    let output = executor.formatter().format_result(&result);
    executor.write_output(&output)?;
    executor.write_output(&format!("({} rows, stream mode)", result.row_count))?;
    Ok(true)
}

pub async fn execute_cursor_open(
    executor: &mut CommandExecutor,
    query: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let sid = session_id(session_mgr)?;
    let substituted = session_mgr
        .session()
        .map(|s| s.substitute_variables(query))
        .transpose()?
        .unwrap_or_else(|| query.to_string());
    let (cursor_id, columns) = session_mgr.client().open_cursor(&substituted, sid).await?;
    executor.set_active_cursor(Some(cursor_id));
    executor.write_output(&format!(
        "Cursor {} opened ({}). Use \\cursor fetch [page_size] to read pages.",
        cursor_id,
        columns.join(", ")
    ))?;
    Ok(true)
}

pub async fn execute_cursor_fetch(
    executor: &mut CommandExecutor,
    cursor_id: Option<u64>,
    page_size: Option<usize>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let sid = session_id(session_mgr)?;
    let cid = cursor_id
        .or_else(|| executor.active_cursor())
        .ok_or_else(|| {
            CliError::InvalidValue("No active cursor. Use \\cursor open <query> first.".to_string())
        })?;
    let size = page_size.unwrap_or(500);
    let (result, has_more) = session_mgr
        .client()
        .fetch_cursor_page(cid, size, sid)
        .await?;
    let output = executor.formatter().format_result(&result);
    executor.write_output(&output)?;
    executor.write_output(&format!(
        "({} rows, cursor {} has_more={})",
        result.row_count, cid, has_more
    ))?;
    if !has_more {
        executor.set_active_cursor(None);
    }
    Ok(true)
}

pub async fn execute_cursor_close(
    executor: &mut CommandExecutor,
    cursor_id: Option<u64>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let sid = session_id(session_mgr)?;
    let cid = cursor_id
        .or_else(|| executor.active_cursor())
        .ok_or_else(|| {
            CliError::InvalidValue("No active cursor. Use \\cursor open <query> first.".to_string())
        })?;
    let closed = session_mgr.client().close_cursor(cid, sid).await?;
    if executor.active_cursor() == Some(cid) {
        executor.set_active_cursor(None);
    }
    executor.write_output(&format!(
        "Cursor {} {}.",
        cid,
        if closed { "closed" } else { "already gone" }
    ))?;
    Ok(true)
}
