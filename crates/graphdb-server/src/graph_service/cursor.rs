use crate::client::cursor_context::CursorPage;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::GraphServiceError;
use super::GraphService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    /// Open a forward-only cursor over a single statement's result.
    pub async fn open_cursor(
        &self,
        session_id: i64,
        stmt: &str,
    ) -> Result<(u64, Vec<String>), String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;
        if Self::is_command_like(stmt) {
            return Err(
                "Cursors support single data statements only; commands run on the materialized path"
                    .to_string(),
            );
        }
        if Self::has_multiple_statements(stmt) {
            return Err(
                "Cursors support single data statements only; run multi-statement scripts on the materialized path"
                    .to_string(),
            );
        }
        let result = self
            .execute_stream(session_id, stmt)
            .await
            .map_err(|e: GraphServiceError| e.message().to_string())?;
        let columns = result.column_names().unwrap_or_default();
        let cursor_id = session.open_cursor(stmt.to_string(), result, columns.clone())?;
        Ok((cursor_id, columns))
    }

    /// Fetch one page from a cursor. Chunk pulls run off the async
    /// runtime; a failed execution drops the cursor and reports the error.
    pub async fn fetch_cursor(
        &self,
        session_id: i64,
        cursor_id: u64,
        page_size: usize,
    ) -> Result<CursorPage, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;
        tokio::task::spawn_blocking(move || session.fetch_cursor_page(cursor_id, page_size))
            .await
            .map_err(|e| format!("Cursor fetch task failed: {e}"))?
    }

    /// Release a cursor. Unknown ids report `closed: false`, never an error.
    pub async fn close_cursor(&self, session_id: i64, cursor_id: u64) -> Result<bool, String> {
        let session = self
            .session_manager
            .find_session(session_id)
            .ok_or_else(|| format!("Invalid session ID: {}", session_id))?;
        Ok(session.close_cursor(cursor_id))
    }
}
