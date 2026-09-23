use crate::storage::QueryStorage;
use graphdb_core::error::QueryError;
use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::{Edge, EdgeDirection, Vertex};

/// Resolve the endpoint tag pair declared by an edge type schema.
///
/// Returns `None` when the edge type is unknown or its schema declares no
/// endpoint labels. The schema is the single source of truth for endpoint
/// labels; there is no label guessing and no empty-tag fabrication.
pub fn resolve_edge_endpoint_tags(
    storage: &dyn QueryStorage,
    space_name: &str,
    edge_type: &str,
) -> Option<(String, String)> {
    let info = storage.get_edge_type(space_name, edge_type).ok()??;
    if info.src_tag_name.is_empty() || info.dst_tag_name.is_empty() {
        return None;
    }
    Some((info.src_tag_name, info.dst_tag_name))
}

/// Resolve the tag used to materialize the neighbor on one side of `edge`.
///
/// A plan-provided tag wins when present. Otherwise the tag is derived from
/// the edge type schema by the neighbor's endpoint side. Returns `None` when
/// no tag can be determined; callers skip that hop instead of fabricating a
/// vertex.
pub fn resolve_neighbor_tag(
    storage: &dyn QueryStorage,
    space_name: &str,
    edge: &Edge,
    neighbor_id: &VertexId,
    override_tag: &str,
) -> Option<String> {
    if !override_tag.is_empty() {
        return Some(override_tag.to_string());
    }
    let (src_tag, dst_tag) = resolve_edge_endpoint_tags(storage, space_name, edge.edge_type())?;
    if neighbor_id == edge.dst() {
        Some(dst_tag)
    } else {
        Some(src_tag)
    }
}

/// Resolve the seed label for a bare-id traversal seed.
///
/// A plan-provided label wins. Otherwise every listed edge type must agree
/// on the seed-side endpoint label (the source label for `Out`, the
/// destination label for `In`, a single shared label for `Both`): the edge
/// schemas then determine the seed domain instead of guessing it. Errors on
/// unknown edge types, untyped schemas, or ambiguous domains.
pub fn resolve_seed_tag(
    storage: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    override_tag: &str,
) -> Result<String, QueryError> {
    if !override_tag.is_empty() {
        return Ok(override_tag.to_string());
    }
    if edge_types.is_empty() {
        return Err(QueryError::execution(
            "Traversal from a bare id requires an edge type to determine the seed label"
                .to_string(),
        ));
    }
    let mut seed_tags: Vec<String> = Vec::new();
    for edge_type in edge_types {
        let info = storage
            .get_edge_type(space_name, edge_type)
            .map_err(|e| QueryError::execution(format!("Storage error: {e}")))?
            .ok_or_else(|| QueryError::execution(format!("Edge type '{edge_type}' not found")))?;
        let side_tags: Vec<&str> = match direction {
            EdgeDirection::Out => vec![info.src_tag_name.as_str()],
            EdgeDirection::In => vec![info.dst_tag_name.as_str()],
            EdgeDirection::Both => {
                if info.src_tag_name != info.dst_tag_name {
                    return Err(QueryError::execution(format!(
                        "Bidirectional traversal over '{edge_type}' cannot determine the seed label: endpoint labels differ"
                    )));
                }
                vec![info.src_tag_name.as_str()]
            }
        };
        for tag in side_tags {
            if !tag.is_empty() && !seed_tags.iter().any(|t| t == tag) {
                seed_tags.push(tag.to_string());
            }
        }
    }
    match seed_tags.as_slice() {
        [tag] => Ok(tag.clone()),
        _ => Err(QueryError::execution(
            "Traversal from a bare id requires exactly one seed label".to_string(),
        )),
    }
}

pub struct TraversalGraphReader<'a> {
    storage: &'a dyn QueryStorage,
}

impl<'a> TraversalGraphReader<'a> {
    pub fn new(storage: &'a dyn QueryStorage) -> Self {
        Self { storage }
    }

    pub fn get_vertex(&self, space_name: &str, tag: &str, vertex_id: &VertexId) -> Option<Vertex> {
        if tag.is_empty() {
            return None;
        }
        self.storage.get_vertex(space_name, tag, vertex_id).ok()?
    }

    /// Fetch the neighbor vertex on one side of `edge`, deriving its tag
    /// from the edge type schema when the plan carries none.
    pub fn get_neighbor_vertex(
        &self,
        space_name: &str,
        edge: &Edge,
        neighbor_id: &VertexId,
        override_tag: &str,
    ) -> Option<Vertex> {
        let tag = resolve_neighbor_tag(self.storage, space_name, edge, neighbor_id, override_tag)?;
        self.get_vertex(space_name, &tag, neighbor_id)
    }

    pub fn get_edges(
        &self,
        space_name: &str,
        vertex_id: &VertexId,
        direction: EdgeDirection,
    ) -> Vec<Edge> {
        self.storage
            .get_node_edges(space_name, vertex_id, direction)
            .unwrap_or_default()
    }

    pub fn filter_edges<'b>(&self, edges: &'b [Edge], edge_types: &[String]) -> Vec<&'b Edge> {
        if edge_types.is_empty() {
            edges.iter().collect()
        } else {
            edges
                .iter()
                .filter(|e| edge_types.contains(&e.edge_type))
                .collect()
        }
    }

    pub fn get_neighbor_id(
        &self,
        edge: &Edge,
        current_id: &VertexId,
        direction: EdgeDirection,
    ) -> VertexId {
        match direction {
            EdgeDirection::Out => *edge.dst(),
            EdgeDirection::In => *edge.src(),
            EdgeDirection::Both => {
                if edge.src() == current_id {
                    *edge.dst()
                } else {
                    *edge.src()
                }
            }
        }
    }

    pub fn read_neighbors(
        &self,
        space_name: &str,
        tag: &str,
        vertex_id: &VertexId,
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Vec<(Vertex, Edge)> {
        let edges = self.get_edges(space_name, vertex_id, direction);
        let filtered = self.filter_edges(&edges, edge_types);
        let mut result = Vec::with_capacity(filtered.len());
        for edge in filtered {
            let neighbor_id = self.get_neighbor_id(edge, vertex_id, direction);
            if let Ok(Some(vertex)) = self.storage.get_vertex(space_name, tag, &neighbor_id) {
                result.push((vertex, edge.clone()));
            }
        }
        result
    }
}
