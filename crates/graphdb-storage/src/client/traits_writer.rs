//! Mutation and commit operations.

use graphdb_core::types::{InsertEdgeInfo, InsertVertexInfo, TransactionId, UpdateInfo, VertexId};
use graphdb_core::{Edge, EdgeDeleteKey, StorageError, StorageResult, Vertex};

/// Write operations for vertex and edge data.
pub trait StorageWriter: Send + Sync + std::fmt::Debug {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError>;
    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError>;
    fn update_vertex_replace(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        let _ = (space, vertex);
        Err(StorageError::not_supported(
            "Row replacement is not supported by this storage implementation",
        ))
    }
    /// Delete one vertex without touching its incident edges. The orphaned
    /// edge rows stay visible until repaired: prefer
    /// `delete_vertex_with_edges` for cascade deletes, or run
    /// `repair_dangling_edges` afterwards.
    fn delete_vertex(&mut self, space: &str, tag: &str, id: &VertexId) -> Result<(), StorageError>;
    fn delete_vertex_with_edges(
        &mut self,
        space: &str,
        tag: &str,
        id: &VertexId,
    ) -> Result<(), StorageError>;
    fn batch_delete_vertices_with_edges(
        &mut self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
    ) -> Result<usize, StorageError>;
    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError>;
    /// Minimal default: delegates when splitting is on and enforces the
    /// single-request limit when off. The engine override chunks by label.
    fn batch_insert_vertices_with_split(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
        auto_split: bool,
    ) -> Result<Vec<VertexId>, StorageError> {
        if auto_split {
            return self.batch_insert_vertices(space, vertices);
        }
        if vertices.len() > crate::vertex::MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::new(
                graphdb_core::error::storage::StorageErrorKind::CapacityExceeded,
                format!(
                    "batch holds {} rows above the single-request limit {}: enable split batching by label into smaller chunks instead of growing one request",
                    vertices.len(),
                    crate::vertex::MAX_WRITE_SCOPE_KEYS,
                ),
            ));
        }
        self.batch_insert_vertices(space, vertices)
    }
    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
    fn update_edge_replace(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let _ = (space, edge);
        Err(StorageError::not_supported(
            "Row replacement is not supported by this storage implementation",
        ))
    }
    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError>;
    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError>;
    fn batch_delete_edges(
        &mut self,
        space: &str,
        deletes: &[EdgeDeleteKey],
    ) -> Result<usize, StorageError>;

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError>;
    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError>;
    fn delete_vertex_data(
        &mut self,
        space: &str,
        tag: &str,
        vertex_id: &str,
    ) -> Result<bool, StorageError>;
    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError>;
    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError>;
}

pub trait StorageCommitOps: Send + Sync + std::fmt::Debug {
    fn commit_staged_writes(
        &self,
        transaction_id: TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> StorageResult<graphdb_core::types::CommitLsn>;

    fn abort_staged_writes(&self, transaction_id: TransactionId) -> StorageResult<()>;

    fn commit_staged_writes_with_durability(
        &self,
        transaction_id: TransactionId,
        intents: &[graphdb_core::wal::OutboxIntent],
        _durability: graphdb_core::types::DurabilityLevel,
    ) -> StorageResult<graphdb_core::types::CommitLsn> {
        self.commit_staged_writes(transaction_id, intents)
    }

    fn recover_outbox_projection(
        &self,
        sync_manager: &graphdb_sync::SyncManager,
    ) -> StorageResult<usize>;
}
