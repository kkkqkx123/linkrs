use gpui_kit::{App, Task};
use linkrs_client::{ClientError, DataPage};

use crate::connection::SessionStore;
use crate::runtime;

/// Paginated vertex scan of one tag in the session's current space.
pub fn vertices(
    cx: &App,
    tag: &str,
    limit: usize,
    offset: usize,
    filter: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
) -> Task<Result<DataPage, ClientError>> {
    let manager = SessionStore::manager(cx);
    let tag = tag.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .list_vertices(
                &tag,
                limit,
                offset,
                filter.as_deref(),
                sort_by.as_deref(),
                sort_order.as_deref(),
            )
            .await
    })
}

/// Paginated edge scan of one edge type in the session's current space.
pub fn edges(
    cx: &App,
    edge_type: &str,
    limit: usize,
    offset: usize,
    filter: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
) -> Task<Result<DataPage, ClientError>> {
    let manager = SessionStore::manager(cx);
    let edge_type = edge_type.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .list_edges(
                &edge_type,
                limit,
                offset,
                filter.as_deref(),
                sort_by.as_deref(),
                sort_order.as_deref(),
            )
            .await
    })
}

/// Normalize one data-browser row into `(id, tag_or_type, properties)` for
/// the views. Accepts both the nested `{ vertex: {...} }` envelope the
/// server emits and flat degenerate rows.
pub fn normalize_row(item: &serde_json::Value) -> (String, String, serde_json::Value) {
    let inner = item
        .get("vertex")
        .or_else(|| item.get("edge"))
        .unwrap_or(item);
    let id = inner
        .get("id")
        .or_else(|| inner.get("vid"))
        .map(value_text)
        .unwrap_or_default();
    let kind = inner
        .get("tag")
        .or_else(|| inner.get("type"))
        .or_else(|| inner.get("edge_type"))
        .map(value_text)
        .unwrap_or_else(|| "unknown".to_string());
    let properties = inner.get("properties").cloned().unwrap_or_else(|| {
        let mut map = serde_json::Map::new();
        if let Some(object) = inner.as_object() {
            for (key, value) in object {
                if !matches!(
                    key.as_str(),
                    "id" | "vid"
                        | "tag"
                        | "type"
                        | "edge_type"
                        | "src"
                        | "dst"
                        | "rank"
                        | "properties"
                ) {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        serde_json::Value::Object(map)
    });
    (id, kind, properties)
}

fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_nested_vertex_rows() {
        let item = serde_json::json!({
            "vertex": {"id": "1", "tag": "Person", "properties": {"name": "ada"}}
        });
        let (id, kind, properties) = normalize_row(&item);
        assert_eq!(id, "1");
        assert_eq!(kind, "Person");
        assert_eq!(properties.get("name").and_then(|v| v.as_str()), Some("ada"));
    }

    #[test]
    fn normalizes_flat_edge_rows() {
        let item = serde_json::json!({"id": "e1", "type": "Knows", "since": 2020});
        let (id, kind, properties) = normalize_row(&item);
        assert_eq!(id, "e1");
        assert_eq!(kind, "Knows");
        assert_eq!(properties.get("since").and_then(|v| v.as_i64()), Some(2020));
    }
}
