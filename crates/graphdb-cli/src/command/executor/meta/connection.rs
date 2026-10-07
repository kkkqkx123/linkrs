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

pub fn execute_whoami(
    executor: &mut CommandExecutor,
    session_mgr: &SessionManager,
) -> Result<bool> {
    match session_mgr.session() {
        Some(session) => {
            let role = session.display_role.as_deref().unwrap_or("(unknown)");
            executor.write_output(&format!(
                "{} (session {}, role {})",
                session.username, session.session_id, role
            ))?;
        }
        None => {
            executor.write_output("Not connected")?;
        }
    }
    Ok(true)
}

pub async fn execute_login(
    executor: &mut CommandExecutor,
    username: &str,
    password: Option<String>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let password = match password {
        Some(p) => p,
        None => rpassword::prompt_password("Password: ").map_err(|e| {
            crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
        })?,
    };
    session_mgr.connect(username, &password).await?;
    let info = session_mgr
        .session()
        .map(|s| s.conninfo())
        .unwrap_or_else(|| format!("Logged in as {}", username));
    executor.write_output(&info)?;
    Ok(true)
}

pub async fn execute_passwd(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let old_password = rpassword::prompt_password("Old password: ").map_err(|e| {
        crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
    })?;
    let new_password = rpassword::prompt_password("New password: ").map_err(|e| {
        crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
    })?;
    let confirm = rpassword::prompt_password("Confirm new password: ").map_err(|e| {
        crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
    })?;
    if new_password != confirm {
        return Err(crate::utils::error::CliError::auth(
            "New passwords do not match".to_string(),
        ));
    }
    let query = format!(
        "CHANGE PASSWORD '{}' TO '{}'",
        old_password.replace('\'', "''"),
        new_password.replace('\'', "''")
    );
    session_mgr.execute_query(&query).await?;
    executor.write_output("Password changed.")?;
    Ok(true)
}
