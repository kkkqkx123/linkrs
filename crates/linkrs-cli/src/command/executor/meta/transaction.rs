use crate::command::executor::CommandExecutor;
use crate::session::manager::SessionManager;
use crate::transaction::IsolationLevel;
use crate::utils::error::{CliError, Result};

pub async fn execute_begin(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor.tx_manager_mut().begin(session_mgr).await?;
    executor.write_output("Transaction started.")?;
    Ok(true)
}

pub async fn execute_commit(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor.tx_manager_mut().commit(session_mgr).await?;
    executor.write_output("Transaction committed.")?;
    Ok(true)
}

pub async fn execute_rollback(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor.tx_manager_mut().rollback(session_mgr).await?;
    executor.write_output("Transaction rolled back.")?;
    Ok(true)
}

pub async fn execute_autocommit(
    executor: &mut CommandExecutor,
    value: Option<String>,
) -> Result<bool> {
    if let Some(v) = value {
        let enabled = match v.to_lowercase().as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            _ => {
                return Err(CliError::InvalidValue(format!(
                    "Invalid autocommit value: {}",
                    v
                )))
            }
        };

        if !enabled && executor.tx_manager().is_active() {
            return Err(CliError::CannotChangeAutocommit);
        }

        executor.tx_manager_mut().set_autocommit(enabled);
        executor.write_output(&format!(
            "Autocommit {}.",
            if enabled { "enabled" } else { "disabled" }
        ))?;
    } else {
        executor.write_output(&format!(
            "Autocommit is {}.",
            if executor.tx_manager().autocommit() {
                "on"
            } else {
                "off"
            }
        ))?;
    }
    Ok(true)
}

pub async fn execute_isolation(
    executor: &mut CommandExecutor,
    level: Option<String>,
) -> Result<bool> {
    if let Some(l) = level {
        let isolation = l
            .parse::<IsolationLevel>()
            .map_err(|_| CliError::InvalidValue(format!("Invalid isolation level: {}", l)))?;

        if executor.tx_manager().is_active() {
            return Err(CliError::TransactionAlreadyActive);
        }

        executor.tx_manager_mut().set_isolation_level(isolation);
        executor.write_output(&format!("Isolation level set to: {}", isolation.as_str()))?;
    } else {
        executor.write_output(&format!(
            "Current isolation level: {}",
            executor.tx_manager().isolation_level().as_str()
        ))?;
    }
    Ok(true)
}

pub async fn execute_savepoint(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor
        .tx_manager_mut()
        .create_savepoint(name, session_mgr)
        .await?;
    executor.write_output(&format!("Savepoint '{}' created.", name))?;
    Ok(true)
}

pub async fn execute_rollback_to(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor
        .tx_manager_mut()
        .rollback_to_savepoint(name, session_mgr)
        .await?;
    executor.write_output(&format!("Rolled back to savepoint '{}'.", name))?;
    Ok(true)
}

pub async fn execute_release_savepoint(
    executor: &mut CommandExecutor,
    name: &str,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    executor
        .tx_manager_mut()
        .release_savepoint(name, session_mgr)
        .await?;
    executor.write_output(&format!("Savepoint '{}' released.", name))?;
    Ok(true)
}

pub fn execute_txstatus(executor: &mut CommandExecutor) -> Result<bool> {
    let info = executor.tx_manager().info();
    executor.write_output(&info.format_status())?;
    Ok(true)
}
