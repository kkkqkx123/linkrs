use std::io::{self, Write};

use crate::command::executor::CommandExecutor;
use crate::command::parser::types::SyncAction;
use crate::outbox::{DegradedClearPayload, RequeuePayload};
use crate::session::SessionManager;
use crate::utils::error::Result;

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn cell(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => "-".to_string(),
        Some(other) => other.to_string(),
    }
}

fn render_rows(value: &serde_json::Value, key: &str, columns: &[&str]) -> String {
    let empty = Vec::new();
    let rows = value.get(key).and_then(|v| v.as_array()).unwrap_or(&empty);
    if rows.is_empty() {
        return format!("{}: (none)", key);
    }
    let mut out = String::new();
    out.push_str(&format!("{} ({}):\n", key, rows.len()));
    out.push_str(&format!("  {}\n", columns.join("  ")));
    for row in rows {
        let cells: Vec<String> = columns.iter().map(|c| cell(row, c)).collect();
        out.push_str(&format!("  {}\n", cells.join("  ")));
    }
    out.trim_end().to_string()
}

/// Confirm a destructive sync operation. Script/force mode skips the prompt
/// with a warning line so automation never hangs on stdin.
fn confirm_destructive(executor: &mut CommandExecutor, prompt: &str) -> Result<bool> {
    if executor.is_force_mode() {
        executor.write_output(&format!("WARNING: {} (auto-confirmed, force mode)", prompt))?;
        return Ok(true);
    }
    print!("{} [y/N] ", prompt);
    let _ = io::stdout().flush();
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(crate::utils::error::CliError::IoError)?;
    let answer = input.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

pub async fn execute_sync(
    executor: &mut CommandExecutor,
    action: &SyncAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\sync")?;
    let client = session_mgr.client();
    match action {
        SyncAction::Status => {
            let value = client.sync_status().await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::Diagnostics => {
            let value = client.outbox_diagnostics().await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::DeadLetters {
            target,
            index_id,
            generation,
            limit,
            offset,
        } => {
            let value = client
                .outbox_dead_letters(target.as_deref(), *index_id, *generation, *limit, *offset)
                .await?;
            executor.write_output(&render_rows(
                &value,
                "dead_letters",
                &[
                    "event_id",
                    "target",
                    "index_id",
                    "generation",
                    "lsn",
                    "state",
                ],
            ))?;
        }
        SyncAction::Requeue {
            target,
            index_id,
            generation,
            limit,
            event_ids,
        } => {
            let is_bulk = event_ids.is_none();
            if is_bulk && !confirm_destructive(executor, "Requeue dead letters in bulk?")? {
                executor.write_output("Cancelled.")?;
                return Ok(true);
            }
            let payload = RequeuePayload {
                target: target.clone(),
                index_id: *index_id,
                generation: *generation,
                limit: Some(*limit),
                event_ids: event_ids.clone(),
            };
            let value = client.outbox_requeue(&payload).await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::Retry => {
            let value = client.outbox_retry().await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::DegradedRanges {
            target,
            index_id,
            generation,
        } => {
            let value = client
                .outbox_degraded_ranges(target.as_deref(), *index_id, *generation)
                .await?;
            executor.write_output(&render_rows(
                &value,
                "degraded_ranges",
                &["target", "index_id", "generation", "start_lsn", "end_lsn"],
            ))?;
        }
        SyncAction::DegradedClear {
            target,
            index_id,
            generation,
            start_lsn,
            end_lsn,
        } => {
            if !confirm_destructive(
                executor,
                &format!(
                    "Clear degraded range {} {}/{}/{}-{}?",
                    target, index_id, generation, start_lsn, end_lsn
                ),
            )? {
                executor.write_output("Cancelled.")?;
                return Ok(true);
            }
            let payload = DegradedClearPayload {
                target: target.clone(),
                index_id: *index_id,
                generation: *generation,
                start_lsn: *start_lsn,
                end_lsn: *end_lsn,
            };
            let value = client.outbox_degraded_clear(&payload).await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::RetentionStatus => {
            let value = client.retention_status().await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::RetentionRun {
            grace_lsn_distance,
            max_age_ms,
        } => {
            let value = client
                .retention_run(*grace_lsn_distance, *max_age_ms)
                .await?;
            executor.write_output(&pretty(&value))?;
        }
    }
    Ok(true)
}
