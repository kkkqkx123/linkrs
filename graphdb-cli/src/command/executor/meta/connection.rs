use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub async fn execute_connect(
    executor: &mut CommandExecutor,
    space: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.switch_space(space).await?;
    executor.write_output(&format!("Connected to space '{}'", space))?;
    Ok(true)
}

pub async fn execute_disconnect(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.disconnect().await?;
    executor.write_output("Disconnected.")?;
    Ok(true)
}

pub fn execute_conninfo(
    executor: &mut CommandExecutor,
    session_mgr: &SessionManager,
) -> Result<bool> {
    let info = session_mgr
        .session()
        .map(|s| s.conninfo())
        .unwrap_or_else(|| "Not connected".to_string());
    executor.write_output(&info)?;
    Ok(true)
}
