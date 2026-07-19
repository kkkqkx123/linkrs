//! WAL Traits
//!
//! Core traits for WAL writer and recovery applier.

use crate::core::error::StorageResult;
use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::Value;

use super::redo::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateVertexTypeRedo,
    DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo, DeleteVertexPropRedo,
    DeleteVertexTypeRedo, DropEdgeIndexRedo, DropSpaceRedo, DropTagIndexRedo, InsertEdgeRedo,
    RenameEdgePropRedo, RenameVertexPropRedo, UpdateEdgePropRedo,
};
use super::types::{WalOpType, WalResult};

/// WAL writer trait
pub trait WalWriter: Send + Sync {
    fn open(&mut self) -> WalResult<()>;
    fn close(&mut self);
    fn append(&mut self, data: &[u8]) -> WalResult<bool>;
    /// Append one logical WAL record and let the writer assign its LSN chain.
    fn append_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> WalResult<bool>;
    fn sync(&self) -> WalResult<()>;

    /// Block until the WAL record at `appended_lsn` is durable (fsynced).
    ///
    /// Default implementation calls `sync()`. Implementations that support
    /// group commit should override this to participate in the coordinated
    /// sync protocol.
    fn wait_for_durable(&self, _appended_lsn: u64) -> WalResult<()> {
        self.sync()
    }
}

/// Trait for applying recovered operations to the storage engine.
/// Implementors handle the actual data modifications during WAL replay.
pub trait RecoveryApplier {
    // ========================================================================
    // Data Operations
    // ========================================================================

    fn replay_insert_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_insert_edge(&self, redo: &InsertEdgeRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_update_vertex_prop(
        &self,
        label: LabelId,
        vid: VertexId,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_update_edge_prop(
        &self,
        redo: &UpdateEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_delete_vertex(
        &self,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_delete_edge(&self, redo: &DeleteEdgeRedo, ts: Timestamp) -> StorageResult<()>;

    // ========================================================================
    // Schema Operations
    // ========================================================================

    fn replay_create_space(&self, redo: &CreateSpaceRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_drop_space(&self, redo: &DropSpaceRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_clear_space(&self, redo: &ClearSpaceRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_alter_space_comment(
        &self,
        redo: &AlterSpaceCommentRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_create_vertex_type(
        &self,
        redo: &CreateVertexTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_create_edge_type(
        &self,
        redo: &CreateEdgeTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_delete_vertex_type(
        &self,
        redo: &DeleteVertexTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_delete_edge_type(
        &self,
        redo: &DeleteEdgeTypeRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_add_vertex_prop(&self, redo: &AddVertexPropRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_add_edge_prop(&self, redo: &AddEdgePropRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_delete_vertex_prop(
        &self,
        redo: &DeleteVertexPropRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_delete_edge_prop(
        &self,
        redo: &DeleteEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_rename_vertex_prop(
        &self,
        redo: &RenameVertexPropRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_rename_edge_prop(
        &self,
        redo: &RenameEdgePropRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    // ========================================================================
    // System Operations
    // ========================================================================

    fn replay_create_tag_index(
        &self,
        redo: &CreateTagIndexRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_drop_tag_index(&self, redo: &DropTagIndexRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_create_edge_index(
        &self,
        redo: &CreateEdgeIndexRedo,
        ts: Timestamp,
    ) -> StorageResult<()>;

    fn replay_drop_edge_index(&self, redo: &DropEdgeIndexRedo, ts: Timestamp) -> StorageResult<()>;

    fn replay_compact(&self, _ts: Timestamp) -> StorageResult<()> {
        log::info!("Compact WAL entry replayed (no-op)");
        Ok(())
    }
}
