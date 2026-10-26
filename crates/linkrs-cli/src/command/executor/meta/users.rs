use crate::command::executor::CommandExecutor;
use crate::command::parser::types::{SessionsAction, UsersAction};
use crate::session::SessionManager;
use crate::utils::error::Result;

/// `\show_users` / `\du` — list server users (admin only).
pub async fn execute_show_users(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let action = crate::command::parser::types::UsersAction::List;
    execute_users(executor, &action, session_mgr).await
}

/// `\users <subcommand>` — user management.
pub async fn execute_users(
    executor: &mut CommandExecutor,
    action: &UsersAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let client = session_mgr.client();
    match action {
        UsersAction::List => {
            let users = client.list_users().await?;
            let mut out = String::from("Users:\n");
            for u in &users.users {
                out.push_str(&format!(
                    "  {}  role={} status={}\n",
                    u.username,
                    u.role.as_deref().unwrap_or("(none)"),
                    u.status.as_deref().unwrap_or("(unknown)")
                ));
            }
            if users.users.is_empty() {
                out.push_str("  (none)\n");
            }
            executor.write_output(out.trim_end())?;
        }
        UsersAction::Create { username, password } => {
            client.create_user(username, password).await?;
            executor.write_output(&format!("User '{}' created.", username))?;
        }
        UsersAction::Drop { username } => {
            client.drop_user(username).await?;
            executor.write_output(&format!("User '{}' dropped.", username))?;
        }
        UsersAction::ResetPassword { username } => {
            let new_password = rpassword::prompt_password("New password: ").map_err(|e| {
                crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
            })?;
            let confirm = rpassword::prompt_password("Confirm new password: ").map_err(|e| {
                crate::utils::error::CliError::auth(format!("Failed to read password: {}", e))
            })?;
            if new_password != confirm {
                return Err(crate::utils::error::CliError::auth(
                    "Passwords do not match".to_string(),
                ));
            }
            client.reset_user_password(username, &new_password).await?;
            executor.write_output(&format!("Password for '{}' reset.", username))?;
        }
        UsersAction::Enable { username } => {
            client.enable_user(username).await?;
            executor.write_output(&format!("User '{}' enabled.", username))?;
        }
        UsersAction::Disable { username } => {
            client.disable_user(username).await?;
            executor.write_output(&format!("User '{}' disabled.", username))?;
        }
        UsersAction::Grant {
            username,
            role,
            space,
        } => {
            client.grant_user_role(username, role, space).await?;
            executor.write_output(&format!(
                "Granted role '{}' on space '{}' to '{}'.",
                role, space, username
            ))?;
        }
        UsersAction::Revoke { username, space } => {
            client.revoke_user_role(username, space).await?;
            executor.write_output(&format!(
                "Revoked roles on space '{}' from '{}'.",
                space, username
            ))?;
        }
    }
    Ok(true)
}

/// `\sessions <subcommand>` — server session management.
pub async fn execute_sessions(
    executor: &mut CommandExecutor,
    action: &SessionsAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let client = session_mgr.client();
    match action {
        SessionsAction::List => {
            let sessions = client.list_server_sessions().await?;
            executor.write_output(&crate::command::executor::meta::stats::pretty(
                &serde_json::Value::Array(sessions),
            ))?;
        }
        SessionsAction::Show { session_id } => {
            let session = client.get_server_session(*session_id).await?;
            executor.write_output(&crate::command::executor::meta::stats::pretty(&session))?;
        }
        SessionsAction::Kill { session_id } => {
            client.kill_server_session(*session_id).await?;
            executor.write_output(&format!("Session {} killed.", session_id))?;
        }
    }
    Ok(true)
}
