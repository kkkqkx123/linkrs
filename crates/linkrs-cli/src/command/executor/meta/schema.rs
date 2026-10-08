use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub async fn execute_show_spaces(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let spaces = session_mgr.list_spaces().await?;
    let output = executor.formatter().format_spaces(&spaces);
    executor.write_output(&output)?;
    Ok(true)
}

pub async fn execute_show_tags(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let tags = session_mgr.list_tags().await?;
    let output = executor.formatter().format_tags(&tags);
    executor.write_output(&output)?;
    Ok(true)
}

pub async fn execute_show_edges(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let edge_types = session_mgr.list_edge_types().await?;
    let output = executor.formatter().format_edge_types(&edge_types);
    executor.write_output(&output)?;
    Ok(true)
}

pub async fn execute_show_functions(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let functions = session_mgr.list_functions().await?;
    let output = executor.formatter().format_functions(&functions);
    executor.write_output(&output)?;
    Ok(true)
}

pub async fn execute_describe(
    executor: &mut CommandExecutor,
    object: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let tags = session_mgr.list_tags().await?;
    if let Some(tag) = tags.iter().find(|t| t.name == object) {
        let output = executor.formatter().format_describe_tag(tag);
        executor.write_output(&output)?;
    } else {
        executor.write_output(
            &executor
                .formatter()
                .format_error(&format!("Tag '{}' not found", object)),
        )?;
    }
    Ok(true)
}

pub async fn execute_describe_edge(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let edge_types = session_mgr.list_edge_types().await?;
    if let Some(et) = edge_types.iter().find(|e| e.name == name) {
        let output = executor.formatter().format_describe_edge(et);
        executor.write_output(&output)?;
    } else {
        executor.write_output(
            &executor
                .formatter()
                .format_error(&format!("Edge type '{}' not found", name)),
        )?;
    }
    Ok(true)
}
