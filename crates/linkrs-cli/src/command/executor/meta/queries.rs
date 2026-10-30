use crate::command::executor::CommandExecutor;
use crate::command::parser::types::QueriesAction;
use crate::session::SessionManager;
use crate::utils::error::Result;

/// `\queries list` / `\queries kill <session_id> <query_id>` — running-query
/// observability without opening a query shell.
pub async fn execute_queries(
    executor: &mut CommandExecutor,
    action: &QueriesAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    session_mgr.require_http("\\queries")?;
    let client = session_mgr.client();
    match action {
        QueriesAction::List => {
            let queries = client.list_queries().await?;
            executor.write_output(&crate::command::executor::meta::stats::pretty(&queries))?;
        }
        QueriesAction::Kill {
            session_id,
            query_id,
        } => {
            let result = client.kill_query(*session_id, *query_id).await?;
            executor.write_output(&crate::command::executor::meta::stats::pretty(&result))?;
        }
    }
    Ok(true)
}
