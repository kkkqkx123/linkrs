use crate::core::types::VertexId;
use crate::core::{Edge, Vertex};
use crate::storage::StorageReader;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct GraphStorageRef {
    pub storage: Arc<RwLock<dyn StorageReader>>,
    pub space: String,
}

impl GraphStorageRef {
    pub fn new(storage: Arc<RwLock<dyn StorageReader>>, space: String) -> Self {
        Self { storage, space }
    }

    pub fn get_neighbors(
        &self,
        node_id: &VertexId,
    ) -> Result<Vec<(VertexId, Edge)>, String> {
        use crate::core::types::EdgeDirection;
        let reader = self.storage.read();
        let edges = reader
            .get_node_edges(&self.space, node_id, EdgeDirection::Both)
            .map_err(|e| format!("Storage error: {}", e))?;
        let neighbors: Vec<(VertexId, Edge)> = edges
            .into_iter()
            .map(|e| {
                let neighbor_id = if e.src == *node_id { e.dst } else { e.src };
                (neighbor_id, e)
            })
            .collect();
        Ok(neighbors)
    }

    pub fn get_vertex(&self, id: &VertexId) -> Result<Option<Vertex>, String> {
        let reader = self.storage.read();
        reader
            .get_vertex(&self.space, id)
            .map_err(|e| format!("Storage error: {}", e))
    }
}
