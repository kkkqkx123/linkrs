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

/// Fetch statistics for one space.
pub fn space_statistics(cx: &App, space: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_space_statistics(&space).await
    })
}

/// Re-read one vertex for edit conflict detection.
pub fn vertex_detail(cx: &App, vid: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let vid = vid.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.get_vertex(&vid).await
    })
}

/// Re-read one edge for edit conflict detection.
pub fn edge_detail(
    cx: &App,
    edge_type: &str,
    src: &str,
    dst: &str,
    rank: i64,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let edge_type = edge_type.to_string();
    let src = src.to_string();
    let dst = dst.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let space = guard.require_space()?;
        guard
            .client()
            .get_edge(&space, &src, &dst, &edge_type, rank)
            .await
    })
}

/// Update vertex properties through an `UPDATE VERTEX` statement.
pub fn update_vertex(
    cx: &App,
    vid: &str,
    properties: &serde_json::Map<String, serde_json::Value>,
) -> Task<Result<(), ClientError>> {
    run_statement(cx, &build_vertex_update(vid, properties))
}

/// Delete a vertex together with its edges.
pub fn delete_vertex(cx: &App, vid: &str) -> Task<Result<(), ClientError>> {
    run_statement(cx, &format!("DELETE VERTEX {} WITH EDGE", quote_vid(vid)))
}

/// Update edge properties through an `UPDATE EDGE` statement.
pub fn update_edge(
    cx: &App,
    edge_type: &str,
    src: &str,
    dst: &str,
    rank: i64,
    properties: &serde_json::Map<String, serde_json::Value>,
) -> Task<Result<(), ClientError>> {
    run_statement(
        cx,
        &build_edge_update(edge_type, src, dst, rank, properties),
    )
}

/// Delete one edge.
pub fn delete_edge(
    cx: &App,
    edge_type: &str,
    src: &str,
    dst: &str,
    rank: i64,
) -> Task<Result<(), ClientError>> {
    let head = edge_head(edge_type, src, dst, rank);
    run_statement(cx, &format!("DELETE EDGE {head}"))
}

fn run_statement(cx: &App, statement: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let statement = statement.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let outcome = guard.execute_query(&statement).await?;
        match outcome.error {
            Some(info) => Err(ClientError::QueryError(info.message)),
            None => Ok(()),
        }
    })
}

/// Extract edge endpoints `(type, src, dst, rank)` from a raw browser row,
/// accepting both the nested `{edge: {...}}` envelope and flat rows.
pub fn edge_endpoints(item: &serde_json::Value) -> Option<(String, String, String, i64)> {
    let inner = item.get("edge").unwrap_or(item);
    let edge_type = inner
        .get("type")
        .or_else(|| inner.get("edge_type"))
        .and_then(|value| value.as_str())?;
    let properties = inner.get("properties");
    let src = inner
        .get("src")
        .and_then(|value| value.as_str())
        .or_else(|| {
            properties
                .and_then(|props| props.get("src"))
                .and_then(|value| value.as_str())
        })?;
    let dst = inner
        .get("dst")
        .and_then(|value| value.as_str())
        .or_else(|| {
            properties
                .and_then(|props| props.get("dst"))
                .and_then(|value| value.as_str())
        })?;
    let rank = inner
        .get("rank")
        .and_then(|value| value.as_i64())
        .or_else(|| {
            properties
                .and_then(|props| props.get("rank"))
                .and_then(|value| value.as_i64())
        })
        .unwrap_or(0);
    Some((
        edge_type.to_string(),
        src.to_string(),
        dst.to_string(),
        rank,
    ))
}

/// Identity keys never appear in a `SET` assignment list.
fn is_identity_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "vid" | "tag" | "type" | "edge_type" | "src" | "dst" | "rank" | "properties"
    )
}

fn assignments(properties: &serde_json::Map<String, serde_json::Value>) -> String {
    properties
        .iter()
        .filter(|(key, _)| !is_identity_key(key))
        .map(|(key, value)| format!("{key} = {}", format_value(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn build_vertex_update(
    vid: &str,
    properties: &serde_json::Map<String, serde_json::Value>,
) -> String {
    format!(
        "UPDATE VERTEX {} SET {}",
        quote_vid(vid),
        assignments(properties)
    )
}

fn edge_head(edge_type: &str, src: &str, dst: &str, rank: i64) -> String {
    format!(
        "{} -> {} @{} OF {}",
        quote_vid(src),
        quote_vid(dst),
        rank,
        edge_type
    )
}

fn build_edge_update(
    edge_type: &str,
    src: &str,
    dst: &str,
    rank: i64,
    properties: &serde_json::Map<String, serde_json::Value>,
) -> String {
    format!(
        "UPDATE EDGE {} SET {}",
        edge_head(edge_type, src, dst, rank),
        assignments(properties)
    )
}

fn quote_vid(vid: &str) -> String {
    if vid.trim().is_empty() {
        return "\"\"".to_string();
    }
    if vid.trim().parse::<i64>().is_ok() {
        return vid.to_string();
    }
    format_value(&serde_json::Value::String(vid.to_string()))
}

fn format_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(flag) => flag.to_string().to_uppercase(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => {
            let numeric = !text.trim().is_empty() && text.trim().parse::<f64>().is_ok();
            if numeric {
                text.clone()
            } else {
                format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
            }
        }
        other => format!(
            "\"{}\"",
            other.to_string().replace('\\', "\\\\").replace('"', "\\\"")
        ),
    }
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
