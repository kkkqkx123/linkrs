use crate::command::executor::CommandExecutor;
use crate::session::SessionManager;
use crate::utils::error::{CliError, Result};

pub async fn execute_set(
    executor: &mut CommandExecutor,
    name: String,
    value: Option<String>,
    remote: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    if remote {
        session_mgr.require_http("\\set --remote")?;
        let session_id = session_mgr
            .session()
            .map(|s| s.session_id)
            .ok_or(CliError::NotConnected)?;
        match value {
            Some(v) => {
                let json_value = serde_json::from_str::<serde_json::Value>(&v)
                    .unwrap_or(serde_json::Value::String(v.clone()));
                session_mgr
                    .client()
                    .set_session_variable(session_id, &name, json_value)
                    .await?;
                executor.write_output(&format!("Set server variable: {} = {}", name, v))?;
            }
            None => {
                let vars = session_mgr
                    .client()
                    .list_session_variables(session_id)
                    .await?;
                let rendered =
                    serde_json::to_string_pretty(&vars).unwrap_or_else(|_| vars.to_string());
                executor.write_output(&format!("Server variables:\n{}", rendered))?;
            }
        }
        return Ok(true);
    }
    let session = session_mgr.session_mut().ok_or(CliError::NotConnected)?;
    match value {
        Some(v) => {
            session.set_variable(name.clone(), v)?;
            let val = session.get_variable(&name).unwrap();
            executor.write_output(&format!("Set variable: {} = {}", name, val))?;
            if !session_mgr.is_embedded() {
                executor.write_output(
                    "(local-only; server $variables are separate, use \\set --remote or LET)",
                )?;
            }
        }
        None => {
            if let Some(v) = session.get_variable(&name) {
                executor.write_output(&format!("{} = {}", name, v))?;
            } else {
                executor.write_output(&format!("Variable '{}' is not set", name))?;
            }
        }
    }
    Ok(true)
}

pub async fn execute_unset(
    executor: &mut CommandExecutor,
    name: String,
    remote: bool,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    if remote {
        session_mgr.require_http("\\unset --remote")?;
        let session_id = session_mgr
            .session()
            .map(|s| s.session_id)
            .ok_or(CliError::NotConnected)?;
        session_mgr
            .client()
            .delete_session_variable(session_id, &name)
            .await?;
        executor.write_output(&format!("Unset server variable: {}", name))?;
        return Ok(true);
    }
    let session = session_mgr.session_mut().ok_or(CliError::NotConnected)?;
    session.remove_variable(&name);
    executor.write_output(&format!("Unset variable: {}", name))?;
    Ok(true)
}

pub async fn execute_show_variables(
    executor: &mut CommandExecutor,
    remote: bool,
    session_mgr: &SessionManager,
) -> Result<bool> {
    if remote {
        session_mgr.require_http("\\set --remote")?;
        let session_id = session_mgr
            .session()
            .map(|s| s.session_id)
            .ok_or(CliError::NotConnected)?;
        let vars = session_mgr
            .client()
            .list_session_variables(session_id)
            .await?;
        let rendered = serde_json::to_string_pretty(&vars).unwrap_or_else(|_| vars.to_string());
        executor.write_output(&rendered)?;
        return Ok(true);
    }
    let session = session_mgr.session().ok_or(CliError::NotConnected)?;
    let all_vars = session.variable_store.all_variables();
    if all_vars.is_empty() {
        executor.write_output("(no variables set)")?;
    } else {
        let mut output = String::new();
        for (name, value) in all_vars {
            let marker = if session.variable_store.is_special(name) {
                "*"
            } else {
                ""
            };
            output.push_str(&format!("{}{} = {}\n", marker, name, value));
        }
        executor.write_output(output.trim_end())?;
        if !session_mgr.is_embedded() {
            executor.write_output("(local-only; server $variables: \\set --remote)")?;
        }
    }
    Ok(true)
}
