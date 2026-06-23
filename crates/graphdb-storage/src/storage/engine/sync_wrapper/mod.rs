//! Storage Layer Synchronous Wrapper
//!
//! Decorator pattern implementation that wraps any StorageClient to automatically
//! synchronize storage operations with external index systems (fulltext, vector).

use crate::core::metadata::SchemaManager;
use crate::core::types::{EdgeTypeInfo, TagInfo, VertexId};
use crate::core::{Edge, StorageError, Value, Vertex};
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageClient, StorageGcOps, StoragePersistenceOps,
    StorageReader, StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps,
    StorageSnapshotOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use std::fmt::Debug;
use std::sync::Arc;

/// Decorator that wraps a StorageClient to provide automatic index synchronization.
#[derive(Clone, Debug)]
pub struct SyncWrapper<S: StorageClient + Debug> {
    inner: S,
    sync_manager: Option<Arc<crate::sync::SyncManager>>,
    enabled: bool,
}

impl<S: StorageClient> SyncWrapper<S> {
    /// Create a new wrapper without synchronization.
    pub fn new(storage: S) -> Self {
        Self {
            inner: storage,
            sync_manager: None,
            enabled: false,
        }
    }

    /// Create a new wrapper with a SyncManager for index synchronization.
    pub fn with_sync_manager(storage: S, sync_manager: Arc<crate::sync::SyncManager>) -> Self {
        Self {
            inner: storage,
            sync_manager: Some(sync_manager),
            enabled: true,
        }
    }

    /// Enable or disable synchronization.
    pub fn enable_sync(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Check if synchronization is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get reference to the sync manager.
    pub fn get_sync_manager(&self) -> Option<Arc<crate::sync::SyncManager>> {
        self.sync_manager.clone()
    }

    /// Get reference to the inner storage client.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Get mutable reference to the inner storage client.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl<S: StorageClient + StorageTransactionContextOps> SyncWrapper<S> {
    /// Get the current transaction ID from storage context.
    fn get_current_txn_id(&self) -> Option<crate::core::types::TransactionId> {
        self.inner.get_transaction_context().map(|ctx| ctx.id)
    }
}

#[cfg(test)]
mod tests;
mod write;
mod write_edge;
mod write_vertex;

impl<S: crate::transaction::UndoTarget + StorageClient> crate::transaction::UndoTarget
    for SyncWrapper<S>
{
    fn delete_vertex_type(
        &self,
        label: crate::core::types::LabelId,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_vertex_type(label)
    }

    fn delete_edge_type(
        &self,
        edge_key: crate::core::types::EdgeKey,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_edge_type(edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_vertex(vertex, ts)
    }

    fn delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.delete_edge(edge_ctx)
    }

    fn undo_update_vertex_property(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .undo_update_vertex_property(vertex, col_id, value, ts)
    }

    fn undo_update_edge_property(
        &self,
        edge_id: crate::core::types::EdgeIdentifier,
        oe_offset: i32,
        ie_offset: i32,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .undo_update_edge_property(edge_id, oe_offset, ie_offset, col_id, value, ts)
    }

    fn revert_delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_vertex(vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_edge(edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_vertex_properties(label_name, prop_names)
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_edge_properties(src_label, dst_label, edge_label, prop_names)
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_delete_vertex_label(label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_delete_edge_label(src_label, dst_label, edge_label)
    }

    fn revert_rename_vertex_properties(
        &self,
        label_name: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner
            .revert_rename_vertex_properties(label_name, current_names, original_names)
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        self.inner.revert_rename_edge_properties(
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}

macro_rules! forward_storage_methods {
    ($field:ident; $(fn $name:ident(&self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $name(&self, $($arg: $ty),*) {
                self.$field.$name($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $name:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?);)+) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) {
                self.$field.$name($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $name:ident(&self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $name(&self, $($arg: $ty),*) -> $ret {
                self.$field.$name($($arg),*)
            }
        )+
    };
    ($field:ident; $(fn $name:ident(&mut self $(, $arg:ident : $ty:ty)* $(,)?) -> $ret:ty;)+) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) -> $ret {
                self.$field.$name($($arg),*)
            }
        )+
    };
}

impl<S: StorageClient + 'static> StorageReader for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;
        fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
        fn scan_vertices_by_prop(
            &self,
            space: &str,
            tag: &str,
            prop: &str,
            value: &Value,
        ) -> Result<Vec<Vertex>, StorageError>;
        fn get_edge(
            &self,
            space: &str,
            src: &VertexId,
            dst: &VertexId,
            edge_type: &str,
            rank: i64,
        ) -> Result<Option<Edge>, StorageError>;
        fn get_node_edges(
            &self,
            space: &str,
            node_id: &VertexId,
            direction: crate::core::EdgeDirection,
        ) -> Result<Vec<Edge>, StorageError>;
        fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError>;
        fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
        fn lookup_index(
            &self,
            space: &str,
            index: &str,
            value: &Value,
        ) -> Result<Vec<Value>, StorageError>;
        fn get_vertex_with_schema(
            &self,
            space: &str,
            tag: &str,
            id: &Value,
        ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError>;
        fn get_edge_with_schema(
            &self,
            space: &str,
            edge_type: &str,
            src: &Value,
            dst: &Value,
        ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn scan_vertices_with_schema(
            &self,
            space: &str,
            tag: &str,
        ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError>;
        fn scan_edges_with_schema(
            &self,
            space: &str,
            edge_type: &str,
        ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError>;
        fn get_space(
            &self,
            space: &str,
        ) -> Result<Option<crate::core::types::SpaceInfo>, StorageError>;
        fn get_space_by_id(
            &self,
            space_id: u64,
        ) -> Result<Option<crate::core::types::SpaceInfo>, StorageError>;
        fn list_spaces(&self) -> Result<Vec<crate::core::types::SpaceInfo>, StorageError>;
        fn get_space_id(&self, space: &str) -> Result<u64, StorageError>;
        fn space_exists(&self, space: &str) -> bool;
        fn get_tag(
            &self,
            space: &str,
            tag: &str,
        ) -> Result<Option<crate::core::types::TagInfo>, StorageError>;
        fn list_tags(&self, space: &str) -> Result<Vec<crate::core::types::TagInfo>, StorageError>;
        fn get_edge_type(
            &self,
            space: &str,
            edge: &str,
        ) -> Result<Option<crate::core::types::EdgeTypeInfo>, StorageError>;
        fn list_edge_types(
            &self,
            space: &str,
        ) -> Result<Vec<crate::core::types::EdgeTypeInfo>, StorageError>;
        fn get_tag_index(
            &self,
            space: &str,
            index: &str,
        ) -> Result<Option<crate::core::types::Index>, StorageError>;
        fn list_tag_indexes(
            &self,
            space: &str,
        ) -> Result<Vec<crate::core::types::Index>, StorageError>;
    );
}

impl<S: StorageClient + 'static> StorageSchemaOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn create_space(&mut self, space: &mut crate::core::types::SpaceInfo) -> Result<bool, StorageError>;
        fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn alter_space_comment(&mut self, space_id: u64, comment: String) -> Result<bool, StorageError>;
        fn create_tag(&mut self, space: &str, tag: &crate::core::types::TagInfo) -> Result<u32, StorageError>;
        fn alter_tag(
            &mut self,
            space: &str,
            tag: &str,
            additions: Vec<crate::core::types::PropertyDef>,
            deletions: Vec<String>,
        ) -> Result<bool, StorageError>;
        fn rename_vertex_property(
            &mut self,
            label: crate::core::types::LabelId,
            old_name: &str,
            new_name: &str,
        ) -> Result<(), StorageError>;
        fn rename_tag_property(
            &mut self,
            space: &str,
            tag: &str,
            old_name: &str,
            new_name: &str,
        ) -> Result<bool, StorageError>;
        fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;
        fn create_edge_type(
            &mut self,
            space: &str,
            edge: &crate::core::types::EdgeTypeInfo,
        ) -> Result<u32, StorageError>;
        fn alter_edge_type(
            &mut self,
            space: &str,
            edge_type: &str,
            additions: Vec<crate::core::types::PropertyDef>,
            deletions: Vec<String>,
        ) -> Result<bool, StorageError>;
        fn drop_edge_type(&mut self, space: &str, edge: &str) -> Result<bool, StorageError>;
        fn create_tag_index(
            &mut self,
            space: &str,
            info: &crate::core::types::Index,
        ) -> Result<bool, StorageError>;
        fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    );
}

impl<S: StorageClient + 'static> StorageAuthOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn change_password(&mut self, info: &crate::core::types::PasswordInfo) -> Result<bool, StorageError>;
        fn create_user(&mut self, info: &crate::core::types::UserInfo) -> Result<bool, StorageError>;
        fn alter_user(&mut self, info: &crate::core::types::UserAlterInfo) -> Result<bool, StorageError>;
        fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
        fn grant_role(
            &mut self,
            username: &str,
            space_id: u64,
            role: crate::core::RoleType,
        ) -> Result<bool, StorageError>;
        fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError>;
    );

    fn user_exists(&self, username: &str) -> bool {
        self.inner.user_exists(username)
    }
}

impl<S: StorageClient + 'static> StorageAdmin for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn load_from_disk(&mut self) -> Result<(), StorageError>;
        fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;
    );

    forward_storage_methods!(inner;
        fn save_to_disk(&self) -> Result<(), StorageError>;
        fn get_storage_stats(&self) -> crate::storage::StorageStats;
        fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
        fn get_db_path(&self) -> &str;
    );
}

impl<S: StorageClient + 'static> StoragePersistenceOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn flush(&self) -> Result<(), StorageError>;
        fn save_data(&self) -> crate::core::StorageResult<()>;
        fn save_data_to_dir(&self, dir: &std::path::Path) -> crate::core::StorageResult<()>;
        fn create_checkpoint(&self) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>>;
        fn verify_snapshot(&self, snapshot_id: u64) -> crate::core::StorageResult<bool>;
        fn cleanup_snapshots(&self) -> crate::core::StorageResult<usize>;
        fn snapshot_stats(&self) -> crate::storage::SnapshotStats;
        fn compact(&self, config: &crate::core::types::CompactConfig) -> crate::core::StorageResult<()>;
        fn auto_flush_if_needed(&self) -> crate::core::StorageResult<bool>;
        fn auto_checkpoint_if_needed(&self) -> crate::core::StorageResult<Option<crate::storage::CheckpointStats>>;
        fn should_flush(&self) -> bool;
        fn should_checkpoint(&self) -> bool;
    );
}

impl<S: StorageClient + StorageSchemaContextOps + 'static> StorageSchemaContextOps
    for SyncWrapper<S>
{
    forward_storage_methods!(inner;
        fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
    );
}

impl<S: StorageClient + StorageTransactionContextOps + 'static> StorageTransactionContextOps
    for SyncWrapper<S>
{
    forward_storage_methods!(inner;
        fn get_transaction_context(&self) -> Option<Arc<crate::core::types::TransactionContextInfo>>;
    );

    forward_storage_methods!(inner;
        fn set_transaction_context(&self, context: Option<Arc<crate::core::types::TransactionContextInfo>>);
    );
}

impl<S: StorageClient + 'static> StorageSyncContextOps for SyncWrapper<S> {
    fn get_sync_manager(&self) -> Option<Arc<crate::sync::SyncManager>> {
        self.sync_manager.clone()
    }
}

impl<S: StorageClient + 'static> StorageRecoveryOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn needs_recovery(&self) -> bool;
        fn recover_from_wal(&self) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats>;
        fn recover_from_wal_with_config(
            &self,
            config: crate::transaction::wal::recovery::RecoveryConfig,
        ) -> crate::core::StorageResult<crate::transaction::wal::recovery::RecoveryStats>;
        fn init_with_recovery(&self) -> crate::core::StorageResult<Option<crate::transaction::wal::recovery::RecoveryStats>>;
    );
}

impl<S: StorageClient + 'static> StorageGcOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn is_index_gc_running(&self) -> bool;
        fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>>;
    );

    forward_storage_methods!(inner;
        fn stop_index_gc(&self);
    );
}

impl<S: crate::storage::client::StorageClient + StorageSnapshotOps + 'static> crate::storage::client::StorageSnapshotOps for SyncWrapper<S> {
    forward_storage_methods!(inner;
        fn export_snapshot(&self, ts: crate::core::types::Timestamp) -> crate::core::StorageResult<Vec<crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord>>;
        fn get_freeze_stats(&self) -> Option<crate::storage::engine::background_freeze::FreezeStats>;
    );

    forward_storage_methods!(inner;
        fn trigger_background_freeze(&self) -> crate::core::StorageResult<()>;
    );
}
