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

/// Drop a vertex tag from a space.
pub fn drop_tag(cx: &App, space: &str, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().drop_tag(&space, &name).await
    })
}

/// Drop an edge type from a space.
pub fn drop_edge_type(cx: &App, space: &str, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().drop_edge_type(&space, &name).await
    })
}

/// Alter a vertex tag by adding properties and dropping existing ones.
/// Runs through an `ALTER TAG` statement in the session's current space.
pub fn alter_tag(
    cx: &App,
    name: &str,
    added: Vec<PropertyDef>,
    dropped: Vec<String>,
) -> Task<Result<(), ClientError>> {
    alter_entity(cx, "TAG", name, added, dropped)
}

/// Alter an edge type by adding properties and dropping existing ones.
/// Runs through an `ALTER EDGE` statement in the session's current space.
pub fn alter_edge_type(
    cx: &App,
    name: &str,
    added: Vec<PropertyDef>,
    dropped: Vec<String>,
) -> Task<Result<(), ClientError>> {
    alter_entity(cx, "EDGE", name, added, dropped)
}

fn alter_entity(
    cx: &App,
    kind: &str,
    name: &str,
    added: Vec<PropertyDef>,
    dropped: Vec<String>,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let statement = alter_statement(kind, name, &added, &dropped);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let outcome = guard.execute_query(&statement).await?;
        match outcome.error {
            Some(info) => Err(ClientError::QueryError(info.message)),
            None => Ok(()),
        }
    })
}

fn alter_statement(
    kind: &str,
    name: &str,
    added: &[PropertyDef],
    dropped: &[String],
) -> String {
    let mut parts = Vec::new();
    if !added.is_empty() {
        let fields = added
            .iter()
            .map(|field| {
                let mut text = format!("{} {}", field.name, field.data_type);
                if let Some(default) = field.default_value.as_ref() {
                    text.push_str(&format!(" DEFAULT {default}"));
                }
                text
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("ADD ({fields})"));
    }
    if !dropped.is_empty() {
        parts.push(format!("DROP ({})", dropped.join(", ")));
    }
    format!("ALTER {kind} {name} {}", parts.join(" "))
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
