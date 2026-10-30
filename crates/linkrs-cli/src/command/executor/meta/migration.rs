use crate::command::executor::CommandExecutor;
use crate::command::parser::types::MigrationAction;
use crate::session::SessionManager;
use crate::utils::error::{CliError, Result};
use linkrs_wire::migration::MigrationExecuteResponse;
use std::io::IsTerminal;

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn read_plan_file(path: &str) -> Result<String> {
    std::fs::read_to_string(path).map_err(CliError::IoError)
}

/// Ask the user to confirm a destructive migration operation. Non-interactive
/// stdin must pass `--yes` explicitly.
fn confirm(action: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        return Err(CliError::InvalidValue(format!(
            "\\migration {} modifies schema data; pass --yes to confirm in non-interactive mode",
            action
        )));
    }
    eprint!("Proceed with migration {}? (y/N) ", action);
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(CliError::IoError)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

fn describe_execute_result(action: &str, resp: &MigrationExecuteResponse) -> String {
    let mut lines = vec![format!(
        "Migration {}: success={}, steps_completed={}, rows_migrated={}",
        action, resp.success, resp.steps_completed, resp.rows_migrated
    )];
    if resp.preview == Some(true) {
        lines.push("(dry-run: no data was modified)".to_string());
    }
    for err in &resp.errors {
        lines.push(format!("error: {}", err));
    }
    lines.join("\n")
}

fn status_line(status: &linkrs_wire::migration::MigrationStatusResponse) -> String {
    format!(
        "space={} label={} is_edge={} latest_applied_version={} applied_versions={:?} history_count={}",
        status.space.as_deref().unwrap_or("-"),
        status.label.as_deref().unwrap_or("-"),
        status.is_edge.unwrap_or(false),
        status.latest_applied_version.unwrap_or(0),
        status.applied_versions,
        status.history_count,
    )
}

pub async fn execute_migration(
    executor: &mut CommandExecutor,
    action: &MigrationAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\migration")?;
    let client = session_mgr.client();
    match action {
        MigrationAction::Plan {
            space,
            label,
            from,
            to,
            is_edge,
            expand_contract,
            output,
        } => {
            let resp = client
                .create_migration_plan(space, label, *from, *to, *is_edge, *expand_contract)
                .await?;
            if let Some(path) = output {
                std::fs::write(path, &resp.plan_json).map_err(CliError::IoError)?;
                executor.write_output(&format!(
                    "Migration plan written to {} ({} bytes)",
                    path,
                    resp.plan_json.len()
                ))?;
            } else {
                executor.write_output(&pretty(&resp.plan))?;
                executor.write_output(
                    "Hint: re-run with -f <file> to save the plan for dry-run/execute/rollback",
                )?;
            }
        }
        MigrationAction::DryRun { file } => {
            let plan_json = read_plan_file(file)?;
            let resp = client.dry_run_migration(&plan_json).await?;
            executor.write_output(&describe_execute_result("dry-run", &resp))?;
        }
        MigrationAction::Execute { file, yes } => {
            if !confirm("execute", *yes)? {
                executor.write_output("Aborted")?;
                return Ok(true);
            }
            let plan_json = read_plan_file(file)?;
            let resp = client.execute_migration(&plan_json).await?;
            executor.write_output(&describe_execute_result("execute", &resp))?;
        }
        MigrationAction::Rollback { file, yes } => {
            if !confirm("rollback", *yes)? {
                executor.write_output("Aborted")?;
                return Ok(true);
            }
            let plan_json = read_plan_file(file)?;
            let resp = client.rollback_migration(&plan_json).await?;
            executor.write_output(&describe_execute_result("rollback", &resp))?;
        }
        MigrationAction::Status {
            space,
            label,
            is_edge,
        } => {
            let resp = client.migration_status(space, label, *is_edge).await?;
            executor.write_output(&status_line(&resp))?;
        }
        MigrationAction::History {
            space,
            label,
            is_edge,
        } => {
            let resp = client.migration_history(space, label, *is_edge).await?;
            let json = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        MigrationAction::Watch {
            space,
            label,
            is_edge,
        } => {
            // Poll status every second until it stops changing between two
            // consecutive polls, then report the final state.
            let mut last: Option<String> = None;
            let mut stable_polls = 0u32;
            loop {
                let resp = client.migration_status(space, label, *is_edge).await?;
                let line = status_line(&resp);
                if Some(&line) == last.as_ref() {
                    stable_polls += 1;
                } else {
                    stable_polls = 0;
                }
                last = Some(line.clone());
                executor.write_output(&line)?;
                if stable_polls >= 2 {
                    executor.write_output("Migration status settled")?;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
    Ok(true)
}
