use crate::command::executor::CommandExecutor;
use crate::command::parser::types::TransactionsAction;
use crate::session::SessionManager;
use crate::utils::error::Result;

fn cell(value: &serde_json::Value, key: &str) -> String {
    match value.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => "-".to_string(),
        Some(other) => other.to_string(),
    }
}

/// `\transactions ...` — server-side transaction observability.
pub async fn execute_transactions(
    executor: &mut CommandExecutor,
    action: &TransactionsAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\transactions")?;
    let client = session_mgr.client();
    match action {
        TransactionsAction::List => {
            let rows = client.list_transactions().await?;
            if rows.is_empty() {
                executor.write_output("No server-side transactions.")?;
            } else {
                let mut out = format!("Transactions ({}):\n", rows.len());
                out.push_str("  id  state  owner  elapsed_ms  staged  undo  blocking\n");
                for row in &rows {
                    out.push_str(&format!(
                        "  {}  {}  {}  {}  {}  {}  {}\n",
                        cell(row, "transaction_id"),
                        cell(row, "state"),
                        cell(row, "owner"),
                        cell(row, "elapsed_ms"),
                        cell(row, "staged_bytes"),
                        cell(row, "undo_bytes"),
                        cell(row, "blocking_reason"),
                    ));
                }
                executor.write_output(out.trim_end())?;
            }
        }
        TransactionsAction::Metrics => {
            let metrics = client.transaction_metrics().await?;
            executor.write_output(&crate::command::executor::meta::stats::pretty(&metrics))?;
        }
        TransactionsAction::Kill { id } => {
            let result = client.kill_transaction(*id).await?;
            executor.write_output(&format!(
                "Transaction {} killed.\n{}",
                id,
                crate::command::executor::meta::stats::pretty(&result)
            ))?;
        }
        TransactionsAction::RetryOutbox { id } => {
            let result = client.retry_transaction_outbox(*id).await?;
            executor.write_output(&format!(
                "Outbox retry requested for transaction {}.\n{}",
                id,
                crate::command::executor::meta::stats::pretty(&result)
            ))?;
        }
    }
    Ok(true)
}
