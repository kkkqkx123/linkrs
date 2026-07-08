//! Undo Operation Types
//!
//! Provides the core trait and error types for transaction undo/rollback operations.

use super::property_value::PropertyValue;
use super::storage_ids::{
    ColumnId, EdgeDeletionContext, EdgeId, EdgeIdentifier, EdgeKey, LabelId, Timestamp, VertexId,
    VertexIdentifier,
};

/// Undo log error
#[derive(Debug, Clone, thiserror::Error)]
pub enum UndoLogError {
    #[error("Undo operation failed: {0}")]
    UndoFailed(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Label not found: {0}")]
    LabelNotFound(LabelId),

    #[error("Vertex not found: {0}")]
    VertexNotFound(VertexId),

    #[error("Edge not found: {0}")]
    EdgeNotFound(EdgeId),

    #[error("Property not found: {0}")]
    PropertyNotFound(String),
}

/// Undo log result type
pub type UndoLogResult<T> = Result<T, UndoLogError>;

/// Target for undo operations
pub trait UndoTarget: Send + Sync {
    fn delete_vertex_type(&self, label: LabelId) -> UndoLogResult<()>;
    fn delete_edge_type(&self, edge_key: EdgeKey) -> UndoLogResult<()>;
    fn delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()>;
    fn delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()>;
    fn undo_update_vertex_property(
        &self,
        vertex: VertexIdentifier,
        col_id: ColumnId,
        value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()>;
    fn undo_update_edge_property(
        &self,
        edge_id: EdgeIdentifier,
        oe_offset: i32,
        ie_offset: i32,
        col_id: ColumnId,
        value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()>;
    fn revert_delete_vertex(&self, vertex: VertexIdentifier, ts: Timestamp) -> UndoLogResult<()>;
    fn revert_delete_edge(&self, edge_ctx: EdgeDeletionContext) -> UndoLogResult<()>;
    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()>;
    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()>;
    fn revert_delete_vertex_label(&self, label_name: &str) -> UndoLogResult<()>;
    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> UndoLogResult<()>;
    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()>;
    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()>;
}
