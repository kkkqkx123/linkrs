use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// Create a vector index.
pub fn create_index(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    vector_size: usize,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_vector_index(space_id, &tag_name, &field_name, vector_size).await
    })
}

/// List vector indexes.
pub fn list_indexes(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_vector_indexes().await
    })
}

/// Get one vector index.
pub fn index_info(
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
        guard.client().get_vector_index_info(space_id, &tag_name, &field_name).await
    })
}

/// Drop a vector index.
pub fn drop_index(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().drop_vector_index(space_id, &tag_name, &field_name).await
    })
}

/// Vector similarity search.
pub fn search(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    query_vector: Vec<f32>,
    limit: usize,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().vector_search(space_id, &tag_name, &field_name, &query_vector, limit).await
    })
}

/// Count vectors.
pub fn count(
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
        guard.client().vector_count(space_id, &tag_name, &field_name).await
    })
}

/// Start a vector index rebuild.
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
        guard.client().rebuild_vector_index(space_id, &tag_name, &field_name).await
    })
}

/// Poll a vector rebuild.
pub fn rebuild_status(cx: &App, rebuild_id: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let rebuild_id = rebuild_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().vector_rebuild_status(&rebuild_id).await
    })
}

/// Clear a vector index.
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
        guard.client().clear_vector_index(space_id, &tag_name, &field_name, force).await
    })
}

/// Get one vector point.
pub fn get_point(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    point_id: &str,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    let point_id = point_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_vector_point(space_id, &tag_name, &field_name, &point_id).await
    })
}

/// Scroll vector points.
pub fn scroll(
    cx: &App,
    space_id: u64,
    tag_name: &str,
    field_name: &str,
    limit: usize,
    offset: Option<&str>,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag_name = tag_name.to_string();
    let field_name = field_name.to_string();
    let offset = offset.map(str::to_string);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().scroll_vectors(space_id, &tag_name, &field_name, limit, offset.as_deref()).await
    })
}
