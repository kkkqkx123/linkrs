use crate::command::executor::CommandExecutor;
use crate::command::parser::types::BatchAction;
use crate::session::manager::SessionManager;
use crate::utils::error::{CliError, Result};
use linkrs_wire::batch::BatchType;

fn parse_batch_type(raw: &str) -> Result<BatchType> {
    match raw.to_lowercase().as_str() {
        "vertex" | "vertices" => Ok(BatchType::Vertex),
        "edge" | "edges" => Ok(BatchType::Edge),
        "mixed" => Ok(BatchType::Mixed),
        other => Err(CliError::InvalidValue(format!(
            "Unknown batch type: {}, expected vertex|edge|mixed",
            other
        ))),
    }
}

fn pretty(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

pub async fn execute_batch(
    executor: &mut CommandExecutor,
    action: &BatchAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    match action {
        BatchAction::Create {
            space_id,
            batch_type,
            batch_size,
        } => {
            let kind = parse_batch_type(batch_type)?;
            let batch_id = session_mgr
                .client()
                .create_batch(*space_id, kind, *batch_size)
                .await?;
            executor.write_output(&format!("Batch created: {}", batch_id))?;
        }
        BatchAction::Add {
            batch_id,
            file_path,
        } => {
            let content = std::fs::read_to_string(file_path).map_err(CliError::IoError)?;
            let items: Vec<linkrs_wire::batch::BatchItem> = serde_json::from_str(&content)
                .map_err(|e| CliError::InvalidValue(format!("Invalid batch items file: {}", e)))?;
            let accepted = session_mgr
                .client()
                .add_batch_items(batch_id, items)
                .await?;
            executor.write_output(&format!("Accepted {} items", accepted))?;
        }
        BatchAction::Execute { batch_id } => {
            let resp = session_mgr.client().execute_batch(batch_id).await?;
            let json = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        BatchAction::Status { batch_id } => {
            let status = session_mgr.client().get_batch_status(batch_id).await?;
            let json = serde_json::to_value(&status).unwrap_or(serde_json::Value::Null);
            executor.write_output(&pretty(&json))?;
        }
        BatchAction::Cancel { batch_id } => {
            session_mgr.client().cancel_batch(batch_id).await?;
            executor.write_output(&format!("Batch {} cancelled", batch_id))?;
        }
        BatchAction::Delete { batch_id } => {
            session_mgr.client().delete_batch(batch_id).await?;
            executor.write_output(&format!("Batch {} deleted", batch_id))?;
        }
    }
    Ok(true)
}
