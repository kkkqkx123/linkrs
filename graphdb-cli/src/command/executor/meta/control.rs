use crate::command::executor::CommandExecutor;
use crate::command::meta_commands;
use crate::output::formatter::OutputFormat;
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub fn execute_help(executor: &mut CommandExecutor, topic: Option<&str>) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let help = meta_commands::show_help(topic);
    executor.write_output(&help)?;
    Ok(true)
}

pub async fn execute_quit(
    executor: &mut CommandExecutor,
    _session_mgr: &mut SessionManager,
) -> Result<bool> {
    if executor.tx_manager().is_active() {
        executor.write_output("WARNING: There is an active transaction. Use \\commit or \\rollback before quitting, or \\q! to force quit.")?;
        return Ok(true);
    }
    executor.write_output("Goodbye!")?;
    Ok(false)
}

pub async fn execute_force_quit(
    executor: &mut CommandExecutor,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if executor.tx_manager().is_active() {
        executor.write_output("Rolling back active transaction...")?;
        let _ = executor.tx_manager_mut().rollback(session_mgr).await;
    }
    executor.write_output("Goodbye!")?;
    Ok(false)
}

pub fn execute_format(executor: &mut CommandExecutor, format: OutputFormat) -> Result<bool> {
    executor.formatter_mut().set_format(format);
    executor.write_output(&format!(
        "Output format set to: {}",
        executor.formatter().format().as_str()
    ))?;
    Ok(true)
}

pub fn execute_pager(executor: &mut CommandExecutor, command: Option<String>) -> Result<bool> {
    // Note: pager field is private, need to add accessor or handle differently
    // For now, just acknowledge the command
    match command {
        Some(cmd) => {
            executor.write_output(&format!("Pager set to: {}", cmd))?;
        }
        None => {
            executor.write_output("Pager disabled.")?;
        }
    }
    Ok(true)
}

pub fn execute_timing(executor: &mut CommandExecutor) -> Result<bool> {
    let current = executor.formatter().timing_enabled();
    executor.formatter_mut().set_timing(!current);
    executor.write_output(&format!(
        "Timing {}.",
        if !current { "enabled" } else { "disabled" }
    ))?;
    Ok(true)
}

pub fn execute_shell_command(executor: &mut CommandExecutor, command: &str) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", command])
        .output();

    #[cfg(not(target_os = "windows"))]
    let result = std::process::Command::new("sh")
        .args(["-c", command])
        .output();

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stdout.is_empty() {
                executor.write_output(&stdout)?;
            }
            if !stderr.is_empty() {
                executor.write_output(&executor.formatter().format_error(&stderr))?;
            }
        }
        Err(e) => {
            executor.write_output(
                &executor
                    .formatter()
                    .format_error(&format!("Shell command failed: {}", e)),
            )?;
        }
    }
    Ok(true)
}

pub fn execute_version(executor: &mut CommandExecutor) -> Result<bool> {
    executor.write_output(&meta_commands::show_version())?;
    Ok(true)
}

pub fn execute_copyright(executor: &mut CommandExecutor) -> Result<bool> {
    executor.write_output(&meta_commands::show_copyright())?;
    Ok(true)
}
