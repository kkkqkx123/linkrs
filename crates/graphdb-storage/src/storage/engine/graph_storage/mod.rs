//! Storage Interface Implementation
//!
//! Implements the StorageClient trait for the storage engine.
//! This module acts as an adapter layer between the high-level StorageClient API
//! and the low-level storage engine.

pub mod context;
mod index_engine;
mod index_manager;
mod ops;
mod persistence;
mod reader;
mod schema_writer;
mod schema_engine;
mod writer;

#[cfg(test)]
mod tests;

pub use context::GraphStorageContext;

use std::path::PathBuf;
use std::sync::Arc;

use crate::core::metadata::SchemaManager;
use crate::core::types::TransactionContextInfo;
use crate::core::types::{
    CompactConfig, EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo,
    PropertyDef, SpaceInfo, TagInfo, Timestamp, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex};
use crate::storage::engine::background_freeze::{BackgroundFreezeManager, FreezeStats};
use crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord;
use crate::core::stats::StatsManager;
use crate::storage::engine::PersistenceConfig;
use crate::storage::index::IndexGcConfig;
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageGcOps, StoragePersistenceOps, StorageReader,
    StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageStats,
    StorageSyncContextOps, StorageTransactionContextOps, StorageWriter,
};

#[derive(Clone)]
pub struct GraphStorage {
    ctx: Arc<GraphStorageContext>,
}

impl std::fmt::Debug for GraphStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStorage")
            .field("work_dir", &self.ctx.work_dir())
            .field("db_path", &self.ctx.db_path())
            .finish()
    }
}

impl GraphStorage {
    pub fn new() -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(GraphStorageContext::new()),
        })
    }

    pub fn new_with_path(path: PathBuf) -> StorageResult<Self> {
        GraphStorageContext::new_with_path(path).map(|ctx| Self { ctx: Arc::new(ctx) })
    }

    /// Open a persistent storage instance and load the on-disk state.
    ///
    /// This is the entry point for production usage. It loads the persisted
    /// data first and then replays any remaining WAL entries if recovery is needed.
    pub fn open(path: PathBuf) -> StorageResult<Self> {
        let config = PersistenceConfig::for_work_dir(&path);
        let storage = Self::new_with_persistence(path, config)?;
        let _ = persistence::initialize_with_recovery(&storage.ctx)?;
        Ok(storage)
    }

    pub fn new_with_persistence(path: PathBuf, config: PersistenceConfig) -> StorageResult<Self> {
        GraphStorageContext::new_with_persistence(path, config)
            .map(|ctx| Self { ctx: Arc::new(ctx) })
    }

    pub fn open_with_persistence(
        path: PathBuf,
        enable_wal: bool,
        sync_policy: Option<crate::transaction::wal::SyncPolicy>,
    ) -> StorageResult<Self> {
        let mut config = PersistenceConfig::for_work_dir(&path);
        config.enable_wal = enable_wal;
        config.sync_policy = sync_policy;
        let storage = Self::new_with_persistence(path, config)?;
        let _ = persistence::initialize_with_recovery(&storage.ctx)?;
        Ok(storage)
    }

    pub fn with_index_gc(mut self, config: IndexGcConfig) -> Self {
        let new_ctx = Arc::new((*self.ctx).clone().with_index_gc(config));
        self.ctx = new_ctx;
        self
    }

    /// Set the StatsManager for recording MVCC metrics.
    ///
    /// This injects the stats manager into the GraphStorageContext,
    /// which will then automatically pass it to all EdgeTable instances
    /// for automatic metrics recording.
    pub fn set_stats_manager(mut self, stats: Arc<StatsManager>) -> Self {
        let mut ctx = (*self.ctx).clone();
        ctx.set_stats_manager(stats);
        self.ctx = Arc::new(ctx);
        self
    }

    pub fn is_persistence_enabled(&self) -> bool {
        self.ctx.is_persistence_enabled()
    }

    pub fn with_background_freeze(mut self) -> Self {
        let freeze_config = self.ctx.get_freeze_config_full();
        let manager = Arc::new(BackgroundFreezeManager::from_config(freeze_config));
        let new_ctx = (*self.ctx).clone().with_background_freeze(Arc::clone(&manager));
        self.ctx = Arc::new(new_ctx);
        self
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>> {
        self.ctx.export_snapshot(ts)
    }

    pub fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.ctx.get_freeze_stats()
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        self.ctx.trigger_background_freeze()
    }
}

impl Default for GraphStorage {
    fn default() -> Self {
        Self::new().expect("Failed to create GraphStorage")
    }
}

impl StorageReader for GraphStorage {
    fn get_vertex(&self, space: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError> {
        reader::get_vertex(&self.ctx, space, id)
    }

    fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices(&self.ctx, space)
    }

    fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices_by_tag(&self.ctx, space, tag)
    }

    fn scan_vertices_by_prop(
        &self,
        space: &str,
        tag: &str,
        prop: &str,
        value: &Value,
    ) -> Result<Vec<Vertex>, StorageError> {
        reader::scan_vertices_by_prop(&self.ctx, space, tag, prop, value)
    }

    fn get_edge(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<Option<Edge>, StorageError> {
        reader::get_edge(&self.ctx, space, src, dst, edge_type, rank)
    }

    fn get_node_edges(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
    ) -> Result<Vec<Edge>, StorageError> {
        reader::get_node_edges(&self.ctx, space, node_id, direction)
    }

    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
        reader::scan_edges_by_type(&self.ctx, space, edge_type)
    }

    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError> {
        reader::scan_all_edges(&self.ctx, space)
    }

    fn lookup_index(
        &self,
        space: &str,
        index_name: &str,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        index_manager::lookup_index(&self.ctx, space, index_name, value)
    }

    fn get_vertex_with_schema(
        &self,
        space: &str,
        tag: &str,
        id: &Value,
    ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> {
        reader::get_vertex_with_schema(&self.ctx, space, tag, id)
    }

    fn get_edge_with_schema(
        &self,
        space: &str,
        edge_type: &str,
        src: &Value,
        dst: &Value,
    ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        reader::get_edge_with_schema(&self.ctx, space, edge_type, src, dst)
    }

    fn scan_vertices_with_schema(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> {
        reader::scan_vertices_with_schema(&self.ctx, space, tag)
    }

    fn scan_edges_with_schema(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        reader::scan_edges_with_schema(&self.ctx, space, edge_type)
    }

    fn get_space(&self, space: &str) -> Result<Option<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().get_space(space)
    }

    fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().get_space_by_id(space_id)
    }

    fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> {
        self.ctx.schema_manager().list_spaces()
    }

    fn get_space_id(&self, space: &str) -> Result<u64, StorageError> {
        self.ctx.schema_manager().get_space_id(space)
    }

    fn space_exists(&self, space: &str) -> bool {
        self.ctx
            .schema_manager()
            .get_space(space)
            .ok()
            .flatten()
            .is_some()
    }

    fn get_tag(&self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError> {
        self.ctx.schema_manager().get_tag(space, tag)
    }

    fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>, StorageError> {
        self.ctx.schema_manager().list_tags(space)
    }

    fn get_edge_type(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<EdgeTypeInfo>, StorageError> {
        self.ctx.schema_manager().get_edge_type(space, edge_type)
    }

    fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> {
        self.ctx.schema_manager().list_edge_types(space)
    }

    fn get_tag_index(&self, space: &str, index_name: &str) -> Result<Option<Index>, StorageError> {
        index_manager::get_tag_index(&self.ctx, space, index_name)
    }

    fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError> {
        index_manager::list_tag_indexes(&self.ctx, space)
    }
}

impl StorageWriter for GraphStorage {
    fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError> {
        writer::insert_vertex(&self.ctx, space, vertex)
    }

    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        writer::update_vertex(&self.ctx, space, vertex)
    }

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        writer::delete_vertex(&self.ctx, space, id)
    }

    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        writer::delete_vertex_with_edges(&self.ctx, space, id)
    }

    fn batch_insert_vertices(
        &mut self,
        space: &str,
        vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        writer::batch_insert_vertices(&self.ctx, space, vertices)
    }

    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError> {
        writer::delete_tags(&self.ctx, space, vertex_id, tag_names)
    }

    fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        writer::insert_edge(&self.ctx, space, edge)
    }

    fn delete_edge(
        &mut self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<(), StorageError> {
        writer::delete_edge(&self.ctx, space, src, dst, edge_type, rank)
    }

    fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError> {
        writer::batch_insert_edges(&self.ctx, space, edges)
    }

    fn insert_vertex_data(
        &mut self,
        space: &str,
        info: &InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        writer::insert_vertex_data(&self.ctx, space, info)
    }

    fn insert_edge_data(
        &mut self,
        space: &str,
        info: &InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        writer::insert_edge_data(&self.ctx, space, info)
    }

    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError> {
        writer::delete_vertex_data(&self.ctx, space, vertex_id)
    }

    fn delete_edge_data(
        &mut self,
        space: &str,
        src: &str,
        dst: &str,
        rank: i64,
    ) -> Result<bool, StorageError> {
        writer::delete_edge_data(&self.ctx, space, src, dst, rank)
    }

    fn update_data(
        &mut self,
        space: &str,
        space_id: u64,
        info: &UpdateInfo,
    ) -> Result<bool, StorageError> {
        writer::update_data(&self.ctx, space, space_id, info)
    }
}

impl StorageSchemaOps for GraphStorage {
    fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError> {
        schema_writer::create_space(&self.ctx, space)
    }

    fn drop_space(&mut self, space: &str) -> Result<bool, StorageError> {
        schema_writer::drop_space(&self.ctx, space)
    }

    fn clear_space(&mut self, space: &str) -> Result<bool, StorageError> {
        schema_writer::clear_space(&self.ctx, space)
    }

    fn alter_space_comment(
        &mut self,
        space_id: u64,
        comment: String,
    ) -> Result<bool, StorageError> {
        schema_writer::alter_space_comment(&self.ctx, space_id, comment)
    }

    fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError> {
        schema_writer::create_tag(&self.ctx, space, tag)
    }

    fn alter_tag(
        &mut self,
        space: &str,
        tag_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        schema_writer::alter_tag(&self.ctx, space, tag_name, additions, deletions)
    }

    fn rename_vertex_property(
        &mut self,
        label: LabelId,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), StorageError> {
        schema_engine::rename_vertex_property(&self.ctx, label, old_name, new_name)
    }

    fn rename_tag_property(
        &mut self,
        space: &str,
        tag: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<bool, StorageError> {
        self.ctx
            .schema_manager()
            .rename_tag_property(space, tag, old_name, new_name)
    }

    fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError> {
        schema_writer::drop_tag(&self.ctx, space, tag)
    }

    fn create_edge_type(
        &mut self,
        space: &str,
        edge_type: &EdgeTypeInfo,
    ) -> Result<u32, StorageError> {
        schema_writer::create_edge_type(&self.ctx, space, edge_type)
    }

    fn alter_edge_type(
        &mut self,
        space: &str,
        edge_type_name: &str,
        additions: Vec<PropertyDef>,
        deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        schema_writer::alter_edge_type(&self.ctx, space, edge_type_name, additions, deletions)
    }

    fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError> {
        schema_writer::drop_edge_type(&self.ctx, space, edge_type)
    }

    fn create_tag_index(&mut self, space: &str, index: &Index) -> Result<bool, StorageError> {
        index_manager::create_tag_index(&self.ctx, space, index)
    }

    fn drop_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        index_manager::drop_tag_index(&self.ctx, space, index_name)
    }

    fn rebuild_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        let vertices = reader::scan_vertices(&self.ctx, space)?;
        index_manager::rebuild_tag_index(&self.ctx, space, index_name, &vertices)
    }
}

impl StorageAuthOps for GraphStorage {
    fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError> {
        ops::change_password(&self.ctx, info)
    }

    fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError> {
        ops::create_user(&self.ctx, info)
    }

    fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError> {
        ops::alter_user(&self.ctx, info)
    }

    fn drop_user(&mut self, username: &str) -> Result<bool, StorageError> {
        ops::drop_user(&self.ctx, username)
    }

    fn user_exists(&self, username: &str) -> bool {
        self.ctx.user_storage().user_exists(username)
    }

    fn grant_role(
        &mut self,
        username: &str,
        space_id: u64,
        role: RoleType,
    ) -> Result<bool, StorageError> {
        ops::grant_role(&self.ctx, username, space_id, role)
    }

    fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError> {
        ops::revoke_role(&self.ctx, username, space_id)
    }
}

impl StorageAdmin for GraphStorage {
    fn load_from_disk(&mut self) -> Result<(), StorageError> {
        persistence::load_from_disk(&self.ctx)
    }

    fn save_to_disk(&self) -> Result<(), StorageError> {
        persistence::save_to_disk(&self.ctx)
    }

    fn get_storage_stats(&self) -> StorageStats {
        ops::get_storage_stats(&self.ctx)
    }

    fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError> {
        ops::find_dangling_edges(&self.ctx, space)
    }

    fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError> {
        ops::repair_dangling_edges(&self.ctx, space)
    }

    fn get_db_path(&self) -> &str {
        self.ctx.db_path()
    }
}

impl StoragePersistenceOps for GraphStorage {
    fn flush(&self) -> StorageResult<()> {
        persistence::flush(&self.ctx)
    }

    fn create_checkpoint(&self) -> StorageResult<Option<crate::storage::CheckpointStats>> {
        persistence::create_checkpoint(&self.ctx)
    }

    fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        persistence::verify_snapshot(&self.ctx, snapshot_id)
    }

    fn cleanup_snapshots(&self) -> StorageResult<usize> {
        persistence::cleanup_snapshots(&self.ctx)
    }

    fn snapshot_stats(&self) -> crate::storage::SnapshotStats {
        persistence::snapshot_stats(&self.ctx)
    }

    fn compact(&self, config: &CompactConfig) -> StorageResult<()> {
        persistence::compact_transactional(&self.ctx, config)
    }

    fn save_data(&self) -> StorageResult<()> {
        persistence::save_data(&self.ctx)
    }

    fn save_data_to_dir(&self, dir: &std::path::Path) -> StorageResult<()> {
        persistence::save_data_to_dir(&self.ctx, dir)
    }

    fn auto_flush_if_needed(&self) -> StorageResult<bool> {
        persistence::auto_flush_if_needed(&self.ctx)
    }

    fn auto_checkpoint_if_needed(&self) -> StorageResult<Option<crate::storage::CheckpointStats>> {
        persistence::auto_checkpoint_if_needed(&self.ctx)
    }

    fn should_flush(&self) -> bool {
        persistence::should_flush(&self.ctx)
    }

    fn should_checkpoint(&self) -> bool {
        persistence::should_checkpoint(&self.ctx)
    }
}

impl StorageSchemaContextOps for GraphStorage {
    fn get_schema_manager(&self) -> Option<Arc<SchemaManager>> {
        Some(self.ctx.schema_manager().clone())
    }
}

impl StorageTransactionContextOps for GraphStorage {
    fn get_transaction_context(&self) -> Option<Arc<TransactionContextInfo>> {
        self.ctx.get_transaction_context()
    }

    fn set_transaction_context(&self, context: Option<Arc<TransactionContextInfo>>) {
        self.ctx.set_transaction_context(context);
    }
}

impl StorageSyncContextOps for GraphStorage {
    fn get_sync_manager(&self) -> Option<Arc<crate::sync::SyncManager>> {
        None
    }
}

impl StorageRecoveryOps for GraphStorage {
    fn needs_recovery(&self) -> bool {
        persistence::needs_recovery(&self.ctx)
    }

    fn recover_from_wal(&self) -> StorageResult<crate::transaction::wal::recovery::RecoveryStats> {
        persistence::recover_from_wal(&self.ctx)
    }

    fn recover_from_wal_with_config(
        &self,
        config: crate::transaction::wal::recovery::RecoveryConfig,
    ) -> StorageResult<crate::transaction::wal::recovery::RecoveryStats> {
        persistence::recover_from_wal_with_config(&self.ctx, config)
    }

    fn init_with_recovery(
        &self,
    ) -> StorageResult<Option<crate::transaction::wal::recovery::RecoveryStats>> {
        persistence::initialize_with_recovery(&self.ctx)
    }
}

impl StorageGcOps for GraphStorage {
    fn is_index_gc_running(&self) -> bool {
        self.ctx.is_index_gc_running()
    }

    fn start_index_gc(&self) -> Option<std::thread::JoinHandle<()>> {
        self.ctx.start_index_gc()
    }

    fn stop_index_gc(&self) {
        self.ctx.stop_index_gc();
    }
}

impl crate::storage::client::StorageSnapshotOps for GraphStorage {
    fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>> {
        self.ctx.export_snapshot(ts)
    }

    fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.ctx.get_freeze_stats()
    }

    fn trigger_background_freeze(&self) -> StorageResult<()> {
        self.ctx.trigger_background_freeze()
    }
}

impl crate::transaction::UndoTarget for GraphStorage {
    fn delete_vertex_type(
        &self,
        label: crate::core::types::LabelId,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::delete_vertex_type(&*self.ctx, label)
    }

    fn delete_edge_type(
        &self,
        edge_key: crate::core::types::EdgeKey,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::delete_edge_type(&*self.ctx, edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::delete_vertex(&*self.ctx, vertex, ts)
    }

    fn delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::delete_edge(&*self.ctx, edge_ctx)
    }

    fn undo_update_vertex_property(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        col_id: crate::core::types::ColumnId,
        value: crate::transaction::undo_log::PropertyValue,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::undo_update_vertex_property(
            &*self.ctx, vertex, col_id, value, ts,
        )
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
        crate::core::types::UndoTarget::undo_update_edge_property(
            &*self.ctx, edge_id, oe_offset, ie_offset, col_id, value, ts,
        )
    }

    fn revert_delete_vertex(
        &self,
        vertex: crate::core::types::VertexIdentifier,
        ts: crate::transaction::wal::Timestamp,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_vertex(&*self.ctx, vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: crate::core::types::EdgeDeletionContext,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_edge(&*self.ctx, edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_vertex_properties(
            &*self.ctx, label_name, prop_names,
        )
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_edge_properties(
            &*self.ctx, src_label, dst_label, edge_label, prop_names,
        )
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_vertex_label(&*self.ctx, label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_delete_edge_label(
            &*self.ctx, src_label, dst_label, edge_label,
        )
    }

    fn revert_rename_vertex_properties(
        &self,
        label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_rename_vertex_properties(
            &*self.ctx,
            label,
            current_names,
            original_names,
        )
    }

    fn revert_rename_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> crate::transaction::undo_log::UndoLogResult<()> {
        crate::core::types::UndoTarget::revert_rename_edge_properties(
            &*self.ctx,
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}
