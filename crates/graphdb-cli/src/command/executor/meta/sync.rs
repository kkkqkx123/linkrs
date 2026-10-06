use crate::command::executor::CommandExecutor;
use crate::command::parser::types::SyncAction;
use crate::outbox::client_ext::{DegradedClearPayload, RequeuePayload};
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub async fn execute_sync(
    executor: &mut CommandExecutor,
    action: &SyncAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
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
                .outbox_dead_letters(
                    target.as_deref(),
                    *index_id,
                    *generation,
                    *limit,
                    *offset,
                )
                .await?;
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::Requeue {
            target,
            index_id,
            generation,
            limit,
        } => {
            let payload = RequeuePayload {
                target: target.clone(),
                index_id: *index_id,
                generation: *generation,
                limit: Some(*limit),
                event_ids: None,
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
            executor.write_output(&pretty(&value))?;
        }
        SyncAction::DegradedClear {
            target,
            index_id,
            generation,
            start_lsn,
            end_lsn,
        } => {
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
