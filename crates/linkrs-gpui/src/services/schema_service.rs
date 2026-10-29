use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_wire::schema::PropertyDef;
use linkrs_wire::schema::{EdgeTypeInfo, SpaceInfo, TagInfo};

use crate::connection::SessionStore;
use crate::runtime;

/// List all graph spaces.
pub fn spaces(cx: &App) -> Task<Result<Vec<SpaceInfo>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.list_spaces().await
    })
}

/// List vertex tags of the session's current space with their field
/// definitions. Fails with `NoSpaceSelected` before a space is chosen.
pub fn tags(cx: &App) -> Task<Result<Vec<TagInfo>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.list_tags().await
    })
}

/// List edge types of the session's current space with their field
/// definitions. Fails with `NoSpaceSelected` before a space is chosen.
pub fn edge_types(cx: &App) -> Task<Result<Vec<EdgeTypeInfo>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.list_edge_types().await
    })
}

/// Create a new graph space.
pub fn create_space(
    cx: &App,
    name: &str,
    vid_type: Option<&str>,
    comment: Option<&str>,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    let vid_type = vid_type.map(str::to_string);
    let comment = comment.map(str::to_string);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_space(&name, vid_type.as_deref(), comment.as_deref()).await
    })
}

/// Drop a graph space.
pub fn drop_space(cx: &App, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().drop_space(&name).await
    })
}

/// Create a vertex tag in a space.
pub fn create_tag(
    cx: &App,
    space: &str,
    name: &str,
    properties: Vec<PropertyDef>,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_tag(&space, &name, properties).await
    })
}

/// Create an edge type in a space.
pub fn create_edge_type(
    cx: &App,
    space: &str,
    name: &str,
    properties: Vec<PropertyDef>,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_edge_type(&space, &name, properties).await
    })
}

/// Schema version history.
pub fn version_history(
    cx: &App,
    space: &str,
    label: &str,
    is_edge: bool,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_schema_version_history(&space, &label, is_edge).await
    })
}

/// Schema changes between versions.
pub fn changes(
    cx: &App,
    space: &str,
    label: &str,
    from_version: u64,
    to_version: u64,
    is_edge: bool,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_schema_changes(&space, &label, from_version, to_version, is_edge).await
    })
}

/// Detect breaking schema changes.
pub fn breaking_changes(
    cx: &App,
    space: &str,
    label: &str,
    from_version: u64,
    to_version: u64,
    is_edge: bool,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().detect_schema_breaking_changes(&space, &label, from_version, to_version, is_edge).await
    })
}
