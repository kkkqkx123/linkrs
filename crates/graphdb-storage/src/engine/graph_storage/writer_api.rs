use graphdb_core::types::{InsertEdgeInfo, InsertVertexInfo, UpdateInfo, VertexId};
use graphdb_core::{Edge, EdgeDeleteKey, StorageError, Vertex};

use crate::StorageWriter;

use super::writer;
use super::GraphStorage;

impl StorageWriter for GraphStorage {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::insert_vertex(&self.ctx, space, vertex)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::update_vertex(&self.ctx, space, vertex)?;
        self.commit_auto_if_needed()
    }

    fn delete_vertex(&mut self, space: &str, tag: &str, id: &VertexId) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::delete_vertex(&self.ctx, space, tag, id)?;
        self.commit_auto_if_needed()
    }

    fn delete_vertex_with_edges(
        &mut self,
        space: &str,
        tag: &str,
        id: &VertexId,
    ) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::delete_vertex_with_edges(&self.ctx, space, tag, id)?;
        self.commit_auto_if_needed()
    }

    fn batch_delete_vertices_with_edges(
        &mut self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
    ) -> Result<usize, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::batch_delete_vertices_with_edges(&self.ctx, space, tag, ids)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::batch_insert_vertices(&self.ctx, space, vertices)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::insert_edge(&self.ctx, space, edge)?;
        self.commit_auto_if_needed()
    }

    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::update_edge(&self.ctx, space, edge)?;
        self.commit_auto_if_needed()
    }

    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::delete_edge(&self.ctx, space, src, dst, edge_type, rank)?;
        self.commit_auto_if_needed()
    }

    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::batch_insert_edges(&self.ctx, space, edges)?;
        self.commit_auto_if_needed()
    }

    fn batch_delete_edges(
        &mut self,
        space: &str,
        deletes: &[EdgeDeleteKey],
    ) -> Result<usize, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::batch_delete_edges(&self.ctx, space, deletes)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::insert_vertex_data(&self.ctx, space, info)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::insert_edge_data(&self.ctx, space, info)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn delete_vertex_data(
        &mut self,
        space: &str,
        tag: &str,
        vertex_id: &str,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::delete_vertex_data(&self.ctx, space, tag, vertex_id)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::delete_edge_data(&self.ctx, space, src, dst, rank)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }

    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::update_data(&self.ctx, space, space_id, info)?;
        self.commit_auto_if_needed()?;
        Ok(result)
    }
}
