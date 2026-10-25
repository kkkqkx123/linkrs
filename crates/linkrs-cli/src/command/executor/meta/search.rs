use crate::command::executor::CommandExecutor;
use crate::command::parser::types::{FulltextAction, VectorAction};
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

/// `\fulltext <subcommand>` — fulltext index management and search.
pub async fn execute_fulltext(
    executor: &mut CommandExecutor,
    action: &FulltextAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let client = session_mgr.client();
    let output = match action {
        FulltextAction::Search {
            space_id,
            tag_name,
            field_name,
            query,
            limit,
        } => {
            let result = client
                .fulltext_search(*space_id, tag_name, field_name, query, *limit)
                .await?;
            crate::command::executor::meta::stats::pretty(&result)
        }
        FulltextAction::ListIndexes => {
            let indexes = client.list_fulltext_indexes().await?;
            crate::command::executor::meta::stats::pretty(&indexes)
        }
        FulltextAction::Rebuild {
            space_id,
            tag_name,
            field_name,
        } => {
            let status = client
                .rebuild_fulltext_index(*space_id, tag_name, field_name)
                .await?;
            let rebuild_id = status
                .get("rebuild_id")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
                .to_string();
            format!(
                "Fulltext rebuild started: {}\nPoll with \\fulltext status {}",
                crate::command::executor::meta::stats::pretty(&status),
                rebuild_id
            )
        }
        FulltextAction::RebuildStatus { rebuild_id } => {
            let status = client.fulltext_rebuild_status(rebuild_id).await?;
            crate::command::executor::meta::stats::pretty(&status)
        }
        FulltextAction::Clear {
            space_id,
            tag_name,
            field_name,
        } => {
            let result = client
                .clear_fulltext_index(*space_id, tag_name, field_name)
                .await?;
            format!(
                "Fulltext index cleared.\n{}",
                crate::command::executor::meta::stats::pretty(&result)
            )
        }
    };
    executor.write_output(&output)?;
    Ok(true)
}

/// `\vector <subcommand>` — vector index management and search.
pub async fn execute_vector(
    executor: &mut CommandExecutor,
    action: &VectorAction,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }
    let client = session_mgr.client();
    let output = match action {
        VectorAction::Create {
            space_id,
            tag_name,
            field_name,
            vector_size,
        } => {
            let result = client
                .create_vector_index(*space_id, tag_name, field_name, *vector_size)
                .await?;
            crate::command::executor::meta::stats::pretty(&result)
        }
        VectorAction::ListIndexes => {
            let indexes = client.list_vector_indexes().await?;
            crate::command::executor::meta::stats::pretty(&indexes)
        }
        VectorAction::Info {
            space_id,
            tag_name,
            field_name,
        } => {
            let info = client
                .get_vector_index_info(*space_id, tag_name, field_name)
                .await?;
            crate::command::executor::meta::stats::pretty(&info)
        }
        VectorAction::Drop {
            space_id,
            tag_name,
            field_name,
        } => {
            client
                .drop_vector_index(*space_id, tag_name, field_name)
                .await?;
            format!(
                "Vector index dropped: space {} tag '{}' field '{}'.",
                space_id, tag_name, field_name
            )
        }
        VectorAction::Search {
            space_id,
            tag_name,
            field_name,
            vector,
            limit,
        } => {
            let result = client
                .vector_search(*space_id, tag_name, field_name, vector, *limit)
                .await?;
            crate::command::executor::meta::stats::pretty(&result)
        }
        VectorAction::Count {
            space_id,
            tag_name,
            field_name,
        } => {
            let result = client.vector_count(*space_id, tag_name, field_name).await?;
            crate::command::executor::meta::stats::pretty(&result)
        }
        VectorAction::Rebuild {
            space_id,
            tag_name,
            field_name,
        } => {
            let status = client
                .rebuild_vector_index(*space_id, tag_name, field_name)
                .await?;
            let rebuild_id = status
                .get("rebuild_id")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown)")
                .to_string();
            format!(
                "Vector rebuild started: {}\nPoll with \\vector status {}",
                crate::command::executor::meta::stats::pretty(&status),
                rebuild_id
            )
        }
        VectorAction::RebuildStatus { rebuild_id } => {
            let status = client.vector_rebuild_status(rebuild_id).await?;
            crate::command::executor::meta::stats::pretty(&status)
        }
        VectorAction::Clear {
            space_id,
            tag_name,
            field_name,
        } => {
            let result = client
                .clear_vector_index(*space_id, tag_name, field_name)
                .await?;
            format!(
                "Vector index cleared.\n{}",
                crate::command::executor::meta::stats::pretty(&result)
            )
        }
    };
    executor.write_output(&output)?;
    Ok(true)
}
