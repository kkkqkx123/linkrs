use crate::error::StorageResult;
use crate::types::{LabelId, Timestamp, VertexId};
use crate::Value;

use super::redo::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CreateEdgeIndexRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateTagIndexRedo, CreateVertexTypeRedo,
    DeleteEdgePropRedo, DeleteEdgeRedo, DeleteEdgeTypeRedo, DeleteVertexPropRedo,
    DeleteVertexTypeRedo, DropEdgeIndexRedo, DropSpaceRedo, DropTagIndexRedo, InsertEdgeRedo,
    RenameEdgePropRedo, RenameVertexPropRedo, UpdateEdgePropRedo, UpdateSequenceRedo,
};
use super::types::{WalOpType, WalResult};

pub trait WalWriter: Send + Sync {
    fn open(&mut self) -> WalResult<()>;
    fn close(&mut self);
    fn append(&mut self, data: &[u8]) -> WalResult<()>;

    fn append_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> WalResult<()>;
    fn sync(&self) -> WalResult<()>;

    fn wait_for_durable(&self, _appended_lsn: u64) -> WalResult<()> {
        self.sync()
    }
}

pub trait RecoveryApplier {
    // Data Operations
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

    // Schema Operations
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

    // System Operations
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
        Ok(())
    }

    fn replay_update_sequence(
        &self,
        redo: &UpdateSequenceRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let _ = (redo, ts);
        Ok(())
    }
}
