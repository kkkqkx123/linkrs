use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// Fulltext search.
pub fn search(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    query: &str,
    limit: usize,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    let query = query.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().fulltext_search(space_id, &tag_name, &field_name, &query, limit).await
    })
}

/// List fulltext indexes.
pub fn list_indexes(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_fulltext_indexes().await
    })
}

/// List inconsistent fulltext indexes.
pub fn list_inconsistent(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_inconsistent_fulltext_indexes().await
    })
}

/// Start a fulltext index rebuild.
pub fn rebuild(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().rebuild_fulltext_index(space_id, &tag_name, &field_name).await
    })
}

/// Poll a fulltext rebuild.
pub fn rebuild_status(cx: &App, rebuild_id: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let rebuild_id = rebuild_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().fulltext_rebuild_status(&rebuild_id).await
    })
}

/// Clear a fulltext index.
pub fn clear(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    force: bool,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().clear_fulltext_index(space_id, &tag_name, &field_name, force).await
    })
}
