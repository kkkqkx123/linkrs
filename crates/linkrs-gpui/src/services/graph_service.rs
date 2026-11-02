use std::collections::{HashMap, HashSet, VecDeque};

use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// One graph node as shown in the graph view.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub vid: String,
    pub tag: String,
    pub properties: serde_json::Value,
}

/// One graph edge as shown in the graph view.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub src: String,
    pub dst: String,
    pub edge_type: String,
    pub rank: i64,
}

/// Fetch one vertex detail in the session's current space.
pub fn vertex_detail(cx: &App, vid: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let vid = vid.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.get_vertex(&vid).await
    })
}

/// Discovered neighborhood returned by [`expand_neighborhood`].
#[derive(Debug, Clone)]
pub struct Neighborhood {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Expand the neighborhood of `seed` breadth-first up to `depth` hops.
/// Single-hop endpoint calls run sequentially on the tokio runtime; visited
/// vids are skipped so cyclic graphs terminate. Returns the discovered
/// nodes (including the seed) and the traversed edges.
pub fn expand_neighborhood(
    cx: &App,
    seed: &str,
    depth: usize,
    direction: &str,
    edge_type: Option<String>,
) -> Task<Result<Neighborhood, ClientError>> {
    let manager = SessionStore::manager(cx);
    let seed = seed.to_string();
    let direction = direction.to_string();
    runtime::spawn(cx, async move {
        let mut nodes: HashMap<String, GraphNode> = HashMap::new();
        let mut edges: Vec<GraphEdge> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::from([(seed.clone(), 0)]);
        seen.insert(seed.clone());

        // The seed itself may not resolve (typed id missing); still show it
        // so the canvas never comes back empty after an expansion.
        nodes.insert(
            seed.clone(),
            GraphNode {
                vid: seed.clone(),
                tag: String::new(),
                properties: serde_json::Value::Null,
            },
        );

        while let Some((vid, level)) = queue.pop_front() {
            if level >= depth {
                continue;
            }
            let guard = manager.lock().await;
            let space = guard.require_space()?;
            let raw = guard
                .client()
                .get_neighbors(&vid, &space, &direction, edge_type.as_deref())
                .await?;
            drop(guard);
            for item in &raw {
                let node = normalize_neighbor(item);
                let (edge_type_name, rank) = neighbor_edge(item);
                if !node.vid.is_empty() {
                    edges.push(GraphEdge {
                        src: vid.clone(),
                        dst: node.vid.clone(),
                        edge_type: edge_type_name,
                        rank,
                    });
                    if seen.insert(node.vid.clone()) {
                        queue.push_back((node.vid.clone(), level + 1));
                        nodes.entry(node.vid.clone()).or_insert(node);
                    }
                }
            }
        }

        // Fill in the seed detail when it resolved as a neighbor of itself
        // through a cycle; otherwise keep the placeholder.
        let mut ordered: Vec<GraphNode> = nodes.into_values().collect();
        ordered.sort_by(|a, b| a.vid.cmp(&b.vid));
        Ok(Neighborhood { nodes: ordered, edges })
    })
}

/// Degree of one vertex: out/in/total 1-hop neighbor counts. The server has
/// no dedicated degree endpoint, so three directed neighbor calls are
/// issued; counts come from the returned list lengths.
pub fn degree(cx: &App, vid: &str) -> Task<Result<(usize, usize, usize), ClientError>> {
    let manager = SessionStore::manager(cx);
    let vid = vid.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let space = guard.require_space()?;
        let out = guard
            .client()
            .get_neighbors(&vid, &space, "OUT", None)
            .await?
            .len();
        let inn = guard
            .client()
            .get_neighbors(&vid, &space, "IN", None)
            .await?
            .len();
        let both = guard
            .client()
            .get_neighbors(&vid, &space, "BOTH", None)
            .await?
            .len();
        Ok((out, inn, both))
    })
}

/// Normalize one raw neighbor payload into a [`GraphNode`]. Accepts the
/// nested `{ vertex: { vid, tag } }` shape the server emits plus flat
/// degenerate rows; unparseable rows keep an empty vid so the caller can
/// drop them.
pub fn normalize_neighbor(raw: &serde_json::Value) -> GraphNode {
    let wrapper = raw.as_object();
    let vertex = wrapper
        .and_then(|object| object.get("vertex"))
        .filter(|value| value.is_object())
        .unwrap_or(raw);
    let vid = vertex
        .get("vid")
        .or_else(|| raw.get("vid"))
        .map(value_text)
        .unwrap_or_default();
    let tag_object = vertex.get("tag").filter(|value| value.is_object());
    let tag = tag_object
        .and_then(|tag| tag.get("name"))
        .and_then(|name| name.as_str())
        .map(str::to_string)
        .or_else(|| {
            vertex
                .get("tag")
                .and_then(|tag| tag.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let properties = tag_object
        .and_then(|tag| tag.get("properties"))
        .or_else(|| vertex.get("properties"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    GraphNode {
        vid,
        tag,
        properties,
    }
}

fn neighbor_edge(raw: &serde_json::Value) -> (String, i64) {
    let edge_type = raw
        .get("edge_type")
        .or_else(|| raw.get("edgeType"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    let rank = raw
        .get("rank")
        .or_else(|| raw.get("ranking"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    (edge_type, rank)
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
    fn normalizes_nested_neighbor_shape() {
        let raw = serde_json::json!({
            "vertex": {"vid": "2", "tag": {"name": "Person", "properties": {"age": 36}}},
            "edge_type": "Knows",
            "rank": 1,
        });
        let node = normalize_neighbor(&raw);
        assert_eq!(node.vid, "2");
        assert_eq!(node.tag, "Person");
        let (edge_type, rank) = neighbor_edge(&raw);
        assert_eq!(edge_type, "Knows");
        assert_eq!(rank, 1);
    }

    #[test]
    fn normalizes_flat_neighbor_shape() {
        let raw = serde_json::json!({"vid": "3", "tag": "City"});
        let node = normalize_neighbor(&raw);
        assert_eq!(node.vid, "3");
        assert_eq!(node.tag, "City");
    }
}
