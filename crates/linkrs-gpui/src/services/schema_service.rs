use gpui_kit::{App, Task};
use linkrs_client::ClientError;
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
