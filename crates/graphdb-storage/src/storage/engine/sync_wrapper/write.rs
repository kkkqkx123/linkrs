use super::SyncWrapper;
use crate::core::types::{InsertEdgeInfo, InsertVertexInfo, UpdateInfo, VertexId};
use crate::core::{Edge, StorageError, Vertex};
use crate::storage::{StorageClient, StorageWriter};

impl<S: StorageClient + 'static> StorageWriter for SyncWrapper<S> {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        let result = self.inner.insert_vertex(space, vertex.clone())?;
        if let Err(error) = self.sync_insert_vertex(space, &vertex) {
            log::error!(
                "Index event delivery deferred after vertex insert: {}",
                error
            );
        }
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        let old_vertex = self
            .inner
            .get_vertex(space, &vertex.vid)?
            .ok_or_else(|| StorageError::node_not_found(vertex.vid))?;

        self.inner.update_vertex(space, vertex.clone())?;
        if let Err(error) = self.sync_update_vertex(space, &old_vertex, &vertex) {
            log::error!(
                "Index event delivery deferred after vertex update: {}",
                error
            );
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex(&mut self.inner, space, id)?;
        if let Err(error) = self.sync_delete_vertex(space, id, &vertex) {
            log::error!(
                "Index event delivery deferred after vertex delete: {}",
                error
            );
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        let vertex = self
            .inner
            .get_vertex(space, id)?
            .ok_or_else(|| StorageError::node_not_found(*id))?;

        StorageWriter::delete_vertex_with_edges(&mut self.inner, space, id)?;
        if let Err(error) = self.sync_delete_vertex(space, id, &vertex) {
            log::error!(
                "Index event delivery deferred after vertex delete: {}",
                error
            );
        }
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        let results = self.inner.batch_insert_vertices(space, vertices.clone())?;
        if let Err(error) = self.sync_batch_insert_vertices(space, &vertices) {
            log::error!(
                "Index event delivery deferred after vertex batch: {}",
                error
            );
        }
        self.commit_auto_transaction()?;
        Ok(results)
    }

    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError> {
        let result = self.inner.delete_tags(space, vertex_id, tag_names)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let result = self.inner.insert_edge(space, edge.clone());
        if result.is_ok() {
            if let Err(error) = self.sync_insert_edge(space, &edge) {
                log::error!("Index event delivery deferred after edge insert: {}", error);
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let result = self.inner.update_edge(space, edge.clone());
        if result.is_ok() {
            if let Err(error) = self.sync_delete_edge(space, &edge.src, &edge.dst, &edge.edge_type)
            {
                log::error!(
                    "Index event delivery deferred after edge update delete: {}",
                    error
                );
            }
            if let Err(error) = self.sync_insert_edge(space, &edge) {
                log::error!(
                    "Index event delivery deferred after edge update insert: {}",
                    error
                );
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
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
            if let Err(error) = self.sync_delete_edge(space, src, dst, edge_type) {
                log::error!("Index event delivery deferred after edge delete: {}", error);
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError> {
        let result = self.inner.batch_insert_edges(space, edges.clone());
        if result.is_ok() {
            if let Err(error) = self.sync_batch_insert_edges(space, &edges) {
                log::error!("Index event delivery deferred after edge batch: {}", error);
            }
        }
        result?;
        self.commit_auto_transaction()?;
        Ok(())
    }

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        let result = self.inner.insert_vertex_data(space, info)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError> {
        let result = self.inner.delete_vertex_data(space, vertex_id)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        let result = self.inner.insert_edge_data(space, info)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError> {
        let result = self.inner.delete_edge_data(space, src, dst, rank)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }

    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError> {
        let result = self.inner.update_data(space, space_id, info)?;
        self.commit_auto_transaction()?;
        Ok(result)
    }
}
