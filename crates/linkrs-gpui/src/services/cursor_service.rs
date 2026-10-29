use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_client::QueryResult;

use crate::connection::SessionStore;
use crate::runtime;
use crate::state::AppState;

/// Open a cursor over a query.
pub fn open(
    cx: &App,
    query: &str,
) -> Task<Result<(u64, Vec<String>), ClientError>> {
    let manager = SessionStore::manager(cx);
    let query = query.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().open_cursor(&query, id).await
    })
}

/// Fetch a cursor page.
pub fn fetch(
    cx: &App,
    cursor_id: u64,
    page_size: usize,
) -> Task<Result<(QueryResult, bool), ClientError>> {
    let manager = SessionStore::manager(cx);
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().fetch_cursor_page(cursor_id, page_size, id).await
    })
}

/// Close a cursor.
pub fn close(cx: &App, cursor_id: u64) -> Task<Result<bool, ClientError>> {
    let manager = SessionStore::manager(cx);
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard.client().close_cursor(cursor_id, id).await
    })
}
