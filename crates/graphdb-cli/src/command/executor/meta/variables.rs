use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::utils::error::{CliError, Result};

pub async fn execute_set(
    executor: &mut CommandExecutor,
    name: String,
    value: Option<String>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let session = session_mgr.session_mut().ok_or(CliError::NotConnected)?;
    match value {
        Some(v) => {
            session.set_variable(name.clone(), v)?;
            let val = session.get_variable(&name).unwrap();
            executor.write_output(&format!("Set variable: {} = {}", name, val))?;
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
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let session = session_mgr.session_mut().ok_or(CliError::NotConnected)?;
    session.remove_variable(&name);
    executor.write_output(&format!("Unset variable: {}", name))?;
    Ok(true)
}

pub fn execute_show_variables(
    executor: &mut CommandExecutor,
    session_mgr: &SessionManager,
) -> Result<bool> {
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
    }
    Ok(true)
}
