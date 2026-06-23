use super::SyncWrapper;
use crate::core::types::{InsertEdgeInfo, InsertVertexInfo, UpdateInfo, VertexId};
use crate::core::{Edge, StorageError, Vertex};
use crate::storage::{StorageClient, StorageTransactionContextOps, StorageWriter};

impl<S: StorageClient + StorageTransactionContextOps + 'static> StorageWriter for SyncWrapper<S> {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        let result = self.inner.insert_vertex(space, vertex.clone())?;
        self.sync_insert_vertex(space, &vertex)?;
        Ok(result)
    }

    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        let old_vertex = self
            .inner
            .get_vertex(space, &vertex.vid)?
            .ok_or_else(|| StorageError::node_not_found(vertex.vid))?;

        self.inner.update_vertex(space, vertex.clone())?;
        self.sync_update_vertex(space, &old_vertex, &vertex)?;
        Ok(())
    }

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex(&mut self.inner, space, id)?;
        self.sync_delete_vertex(space, id, &vertex)?;
        Ok(())
    }

    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex_with_edges(&mut self.inner, space, id)?;
        self.sync_delete_vertex(space, id, &vertex)?;
        Ok(())
    }

    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        let results = self.inner.batch_insert_vertices(space, vertices.clone())?;
        self.sync_batch_insert_vertices(space, &vertices)?;
        Ok(results)
    }

    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError> {
        self.inner.delete_tags(space, vertex_id, tag_names)
    }

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let result = self.inner.insert_edge(space, edge.clone());
        if result.is_ok() {
            self.sync_insert_edge(space, &edge)?;
        }
        result
    }

    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        let result = StorageWriter::delete_edge(&mut self.inner, space, src, dst, edge_type, rank);
        if result.is_ok() {
            self.sync_delete_edge(space, src, dst, edge_type)?;
        }
        result
    }

    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError> {
        let result = self.inner.batch_insert_edges(space, edges.clone());
        if result.is_ok() {
            self.sync_batch_insert_edges(space, &edges)?;
        }
        result
    }

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        self.inner.insert_vertex_data(space, info)
    }

    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError> {
        self.inner.delete_vertex_data(space, vertex_id)
    }

    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        self.inner.insert_edge_data(space, info)
    }

    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError> {
        self.inner.delete_edge_data(space, src, dst, rank)
    }

    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError> {
        self.inner.update_data(space, space_id, info)
    }
}
