use crate::command::executor::CommandExecutor;
use crate::session::SessionManager;
use crate::utils::error::Result;

pub async fn execute_show_function(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\functions info")?;
    let info = session_mgr.client().get_function_info(name).await?;
    executor.write_output(&crate::command::executor::meta::stats::pretty(&info))?;
    Ok(true)
}

pub async fn execute_register_function(
    executor: &mut CommandExecutor,
    name: &str,
    implementation: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\functions register")?;
    let result = session_mgr
        .client()
        .register_function(name, implementation)
        .await?;
    executor.write_output(&crate::command::executor::meta::stats::pretty(&result))?;
    Ok(true)
}

pub async fn execute_unregister_function(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\functions unregister")?;
    session_mgr.client().unregister_function(name).await?;
    executor.write_output(&format!("Function '{}' unregistered.", name))?;
    Ok(true)
}
