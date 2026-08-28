use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::{Edge, EdgeDirection, Vertex};
use crate::storage::QueryStorage;

pub struct TraversalGraphReader<'a> {
    storage: &'a dyn QueryStorage,
}

impl<'a> TraversalGraphReader<'a> {
    pub fn new(storage: &'a dyn QueryStorage) -> Self {
        Self { storage }
    }

    pub fn get_vertex(&self, space_name: &str, vertex_id: &VertexId) -> Option<Vertex> {
        self.storage.get_vertex(space_name, vertex_id).ok()?
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
        vertex_id: &VertexId,
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Vec<(Vertex, Edge)> {
        let edges = self.get_edges(space_name, vertex_id, direction);
        let filtered = self.filter_edges(&edges, edge_types);
        let mut result = Vec::with_capacity(filtered.len());
        for edge in filtered {
            let neighbor_id = self.get_neighbor_id(edge, vertex_id, direction);
            if let Ok(Some(vertex)) = self.storage.get_vertex(space_name, &neighbor_id) {
                result.push((vertex, edge.clone()));
            }
        }
        result
    }
}
