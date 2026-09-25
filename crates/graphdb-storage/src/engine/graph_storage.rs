//! Storage Interface Implementation
//!
//! Implements the StorageClient trait for the storage engine.
//! This module acts as an adapter layer between the high-level StorageClient API
//! and the low-level storage engine.

mod commit_api;
pub mod context;
mod cursor_impl;
mod index_engine;
mod index_manager;
mod ops;
mod persistence;
mod persistence_api;
mod reader;
mod reader_api;
mod schema_api;
mod schema_engine;
mod schema_writer;
mod serial;
mod stats_reader_impl;
mod writer;
mod writer_api;

#[cfg(test)]
mod tests;

pub use context::{AutoCommitBatchWindow, GraphStorageContext, WriteGateStats};
pub use serial::SerialKey;

use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::background_freeze::{BackgroundFreezeManager, FreezeStats};
use crate::engine::PersistenceConfig;
use crate::index::key_codec::KeyBuilder;
use crate::index::types::IndexIdentity;
use crate::index::IndexGcConfig;
use crate::{
    StorageAdmin, StorageAuthOps, StorageGcOps, StorageOperationContext,
    StorageOperationContextOps, StorageRecoveryOps, StorageSchemaContextOps, StorageStats,
    StorageSyncContextOps,
};
use graphdb_core::metadata::{IndexMetadataManager, SchemaManager};
use graphdb_core::types::{
    CommitLsn, PasswordInfo, SnapshotTimestamp, UserAlterInfo, UserInfo, VertexId,
};
use graphdb_core::{Edge, RoleType, StorageError, StorageResult, Value};
use graphdb_metrics::StatsManager;

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

impl crate::stats_reader::ColumnStatsReader for GraphStorage {
    fn vertex_column_stats(
        &self,
        space: &str,
        tag: &str,
        column: &str,
    ) -> Option<std::sync::Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        stats_reader_impl::vertex_column_stats(&self.ctx, space, tag, column)
    }

    fn edge_column_stats(
        &self,
        space: &str,
        edge_type: &str,
        column: &str,
    ) -> Option<std::sync::Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        stats_reader_impl::edge_column_stats(&self.ctx, space, edge_type, column)
    }

    fn vertex_table_stats(
        &self,
        space: &str,
        tag: &str,
    ) -> Option<std::sync::Arc<crate::stats_reader::TableCardinalitySnapshot>> {
        stats_reader_impl::vertex_table_stats(&self.ctx, space, tag)
    }

    fn stats_epoch(&self) -> u64 {
        // The MVCC write timestamp is monotonic and bumps on every write
        // allocation, making it a cheap data-version stamp for the
        // optimizer's statistics cache.
        self.ctx.version_manager().write_timestamp()
    }
}

impl GraphStorage {
    /// Return the MVCC manager used by this storage instance.
    pub fn version_manager(&self) -> Arc<graphdb_transaction::VersionManager> {
        self.ctx.version_manager().clone()
    }

    /// Cumulative auto-commit write-gate admission statistics (see
    /// [`context::WriteGateStats`]).
    pub fn write_gate_stats(&self) -> context::WriteGateStats {
        self.ctx.write_gate_stats()
    }

    /// Outbox pending depth for backpressure-aware clients.
    pub fn outbox_pending(&self) -> u64 {
        self.ctx.outbox_pending()
    }

    pub fn begin_auto_commit_batch(&self) -> StorageResult<Arc<context::AutoCommitBatchWindow>> {
        self.ctx.begin_auto_commit_batch()
    }

    pub fn bind_auto_commit_statement(
        &self,
        window: &Arc<context::AutoCommitBatchWindow>,
    ) -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(window.bind_statement()?),
        })
    }

    pub fn finalize_auto_commit_batch(
        &self,
        window: &context::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        window.finalize()
    }

    pub fn begin_auto_commit_group(&self) -> StorageResult<Arc<context::AutoCommitBatchWindow>> {
        self.ctx.begin_auto_commit_group()
    }

    pub fn finalize_auto_commit_group(
        &self,
        window: &context::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        window.finalize_group()
    }

    fn commit_auto_if_needed(&self) -> StorageResult<()> {
        let Some(context) = self.ctx.operation_context() else {
            return Ok(());
        };
        if !context.auto_commit || context.read_only {
            return Ok(());
        }
        if self.ctx.is_group_bound() {
            return Ok(());
        }
        let transaction_id = context.transaction_id.ok_or_else(|| {
            StorageError::db_error("Auto-commit storage context has no transaction ID".to_string())
        })?;
        self.ctx.commit_staged_writes(transaction_id, &[])?;
        Ok(())
    }

    pub fn new() -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(GraphStorageContext::new()),
        })
    }

    /// Create with a custom property graph configuration.
    pub fn new_with_config(
        config: crate::engine::config::PropertyGraphConfig,
    ) -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(GraphStorageContext::new_with_config(config)?),
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
        Self::open_with_persistence_config(path, config)
    }

    /// Open persistent storage with an explicit property graph configuration.
    pub fn open_with_config(
        path: PathBuf,
        property_config: crate::engine::config::PropertyGraphConfig,
    ) -> StorageResult<Self> {
        let config =
            PersistenceConfig::for_work_dir(&path).with_property_graph_config(property_config);
        Self::open_with_persistence_config(path, config)
    }

    /// Open persistent storage using a fully specified persistence contract.
    pub fn open_with_persistence_config(
        path: PathBuf,
        config: PersistenceConfig,
    ) -> StorageResult<Self> {
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
        sync_policy: Option<graphdb_transaction::wal::SyncPolicy>,
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

    /// number of staged-WAL entries held for in-flight transactions.
    /// Auto-commit statements commit/remove their staged entries on write;
    /// an unbounded value indicates a lifecycle leak.
    pub fn staged_wal_len(&self) -> usize {
        self.ctx.staged_wal_len()
    }

    /// total number of retired index generations awaiting reclamation.
    pub fn retired_generation_count(&self) -> usize {
        self.ctx
            .index_data_manager()
            .read()
            .retired_generation_count()
    }

    pub fn with_vertex_gc(mut self, config: crate::vertex::VertexGcConfig) -> Self {
        let new_ctx = Arc::new((*self.ctx).clone().with_vertex_gc(config));
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
        let new_ctx = (*self.ctx)
            .clone()
            .with_background_freeze(Arc::clone(&manager));
        self.ctx = Arc::new(new_ctx);
        self
    }

    pub fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.ctx.get_freeze_stats()
    }

    /// Return current and peak memory usage by storage ownership category.
    pub fn resource_snapshot(&self) -> crate::ResourceSnapshot {
        self.ctx.resource_snapshot()
    }

    /// Return WAL durability positions and sync counters when WAL is enabled.
    pub fn wal_metrics(&self) -> Option<crate::WalMetrics> {
        self.ctx.wal_metrics()
    }

    /// Check whether another active snapshot may be registered.
    pub fn check_snapshot_admission(&self) -> StorageResult<()> {
        self.ctx.check_snapshot_admission()
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        self.ctx.trigger_background_freeze()
    }

    /// Run the background maintenance pass synchronously: automatic vertex
    /// compaction (ID hole reclamation, governed by
    /// `PropertyGraphConfig::auto_compact`) followed by delta freeze.
    /// Mainly for tests and explicit operator invocation.
    pub fn trigger_background_maintenance(&self) -> StorageResult<()> {
        self.ctx.trigger_background_maintenance()
    }

    /// Remove old published checkpoints while retaining the newest recovery points.
    pub fn cleanup_old_checkpoints(&self, max_checkpoints: usize) -> StorageResult<usize> {
        let persistence = self
            .ctx
            .persistence()
            .as_ref()
            .ok_or_else(|| StorageError::not_supported("Persistence is not enabled"))?;
        persistence.read().cleanup_old_checkpoints(max_checkpoints)
    }

    /// Split one persistent native index at an ordered-key boundary.
    ///
    /// The operation records a real MVCC snapshot and WAL start position,
    /// builds the new shard layout, then reads the committed WAL intents after
    /// the final publish barrier before installing the new generation.
    pub fn split_native_index(
        &self,
        space: &str,
        index_name: &str,
        boundary: Vec<u8>,
    ) -> StorageResult<()> {
        let space_id = self.ctx.schema_manager().get_space_id(space)?;
        let index = self
            .ctx
            .index_metadata_manager()
            .get_tag_index(space_id, index_name)?
            .or(self
                .ctx
                .index_metadata_manager()
                .get_edge_index(space_id, index_name)?)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_name} not found")))?;
        // Hold the rebuild gate from snapshot acquisition through publication.
        // Index writers take its read side before resolving their active
        // generation, so no writer can land in the old generation after this
        // split snapshot.
        let rebuild_gate = self.ctx.index_data_manager().read().rebuild_gate();
        let _rebuild_guard = rebuild_gate.write();
        let snapshot_timestamp = SnapshotTimestamp::new(self.ctx.get_read_timestamp().max(1));
        let start_lsn = {
            let current = index_manager::current_wal_lsn(&self.ctx);
            if current == CommitLsn::ZERO {
                CommitLsn::new(1)
            } else {
                current
            }
        };
        let wal_context = Arc::clone(&self.ctx);
        let wal_index = index.clone();
        let result = self.ctx.index_data_manager().write().split_native_index(
            IndexIdentity {
                space_id,
                index_id: index.id,
            },
            boundary,
            snapshot_timestamp,
            start_lsn,
            {
                let wal_context = Arc::clone(&wal_context);
                move || {
                    let current = index_manager::current_wal_lsn(&wal_context);
                    Ok(if current < start_lsn {
                        start_lsn
                    } else {
                        current
                    })
                }
            },
            move |from_lsn, to_lsn| {
                index_manager::wal_intents_for_index(
                    &wal_context,
                    space_id,
                    &wal_index,
                    from_lsn,
                    to_lsn,
                )
            },
        );
        if let Some(stats) = self.ctx.stats_manager() {
            stats.record_split(result.is_ok());
            if result.is_err() {
                stats.record_fence_failure();
            }
        }
        result
    }

    /// Split a native index at the beginning of one ordered property value.
    pub fn split_native_index_at_value(
        &self,
        space: &str,
        index_name: &str,
        value: &Value,
    ) -> StorageResult<()> {
        let space_id = self.ctx.schema_manager().get_space_id(space)?;
        let index = self
            .ctx
            .index_metadata_manager()
            .get_tag_index(space_id, index_name)?
            .or(self
                .ctx
                .index_metadata_manager()
                .get_edge_index(space_id, index_name)?)
            .ok_or_else(|| StorageError::not_found(format!("Index {index_name} not found")))?;
        let boundary = match index.index_type {
            graphdb_core::types::IndexType::TagIndex => {
                KeyBuilder::build_vertex_index_value_prefix(space_id, index_name, value)?.0
            }
            graphdb_core::types::IndexType::EdgeIndex => {
                KeyBuilder::build_edge_index_value_prefix(space_id, index_name, value)?.0
            }
        };
        self.split_native_index(space, index_name, boundary)
    }
}

impl crate::AutoCommitBatchOps for GraphStorage {
    fn begin_auto_commit_batch(&self) -> StorageResult<Arc<context::AutoCommitBatchWindow>> {
        self.ctx.begin_auto_commit_batch()
    }

    fn bind_auto_commit_statement(
        &self,
        window: &Arc<context::AutoCommitBatchWindow>,
    ) -> StorageResult<Self> {
        GraphStorage::bind_auto_commit_statement(self, window)
    }

    fn finalize_auto_commit_batch(
        &self,
        window: &context::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        GraphStorage::finalize_auto_commit_batch(self, window)
    }

    fn bind_auto_commit_writer(
        &self,
        window: &Arc<context::AutoCommitBatchWindow>,
    ) -> StorageResult<Box<dyn crate::StorageWriter + '_>> {
        let bound = window.bind_statement()?;
        Ok(Box::new(GraphStorage {
            ctx: Arc::new(bound),
        }))
    }
}

impl crate::AutoCommitGroupOps for GraphStorage {
    fn begin_auto_commit_group(&self) -> StorageResult<Arc<context::AutoCommitBatchWindow>> {
        self.ctx.begin_auto_commit_group()
    }

    fn finalize_auto_commit_group(
        &self,
        window: &context::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        window.finalize_group()
    }

    fn rollback_auto_commit_group(
        &self,
        window: &context::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        window.rollback_group()
    }
}

impl StorageAuthOps for GraphStorage {
    fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        ops::change_password(&self.ctx, info)
    }

    fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        ops::create_user(&self.ctx, info)
    }

    fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        ops::alter_user(&self.ctx, info)
    }

    fn drop_user(&mut self, username: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        ops::drop_user(&self.ctx, username)
    }

    fn user_exists(&self, username: &str) -> bool {
        self.ctx.user_storage().user_exists(username)
    }

    fn list_users(&self) -> Vec<String> {
        self.ctx.user_storage().list_users()
    }

    fn grant_role(
        &mut self,
        username: &str,
        space_id: u64,
        role: RoleType,
    ) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        ops::grant_role(&self.ctx, username, space_id, role)
    }

    fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
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

impl StorageSchemaContextOps for GraphStorage {
    fn get_schema_manager(&self) -> Option<Arc<SchemaManager>> {
        Some(self.ctx.schema_manager().clone())
    }

    fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>> {
        Some(self.ctx.index_metadata_manager().clone())
    }
}

impl StorageOperationContextOps for GraphStorage {
    fn bind_auto_commit_context(&self) -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(self.ctx.with_auto_commit_context()?),
        })
    }

    fn bind_read_operation_context(&self) -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(self.ctx.with_read_operation_context()?),
        })
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        Self {
            ctx: Arc::new(self.ctx.with_operation_context(context)),
        }
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.ctx.operation_context()
    }

    fn finalize_operation(&self, committed: bool) -> graphdb_core::StorageResult<()> {
        self.ctx.finalize_operation(committed)
    }
}

impl StorageSyncContextOps for GraphStorage {
    fn get_sync_manager(&self) -> Option<Arc<graphdb_sync::SyncManager>> {
        None
    }
}

impl StorageRecoveryOps for GraphStorage {
    fn needs_recovery(&self) -> bool {
        persistence::needs_recovery(&self.ctx)
    }

    fn recover_from_wal(&self) -> StorageResult<graphdb_transaction::wal::recovery::RecoveryStats> {
        persistence::recover_from_wal(&self.ctx)
    }

    fn recover_from_wal_with_config(
        &self,
        config: graphdb_transaction::wal::recovery::RecoveryConfig,
    ) -> StorageResult<graphdb_transaction::wal::recovery::RecoveryStats> {
        persistence::recover_from_wal_with_config(&self.ctx, config)
    }

    fn init_with_recovery(
        &self,
    ) -> StorageResult<Option<graphdb_transaction::wal::recovery::RecoveryStats>> {
        persistence::initialize_with_recovery(&self.ctx)
    }
}

impl StorageGcOps for GraphStorage {
    fn is_index_gc_running(&self) -> bool {
        self.ctx.is_index_gc_running()
    }

    fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.ctx.start_index_gc()
    }

    fn stop_index_gc(&self) {
        self.ctx.stop_index_gc();
    }
}

/// Direct vertex GC controls (outside StorageGcOps trait).
impl GraphStorage {
    pub fn is_vertex_gc_running(&self) -> bool {
        self.ctx.is_vertex_gc_running()
    }

    pub fn start_vertex_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.ctx.start_vertex_gc()
    }

    pub fn stop_vertex_gc(&self) {
        self.ctx.stop_vertex_gc();
    }

    /// Register a schema-change observer (single shared registry:
    /// one subscription receives both table/space DDL and index DDL).
    pub fn register_schema_callback(
        &self,
        callback: graphdb_core::metadata::SchemaChangeCallback,
    ) -> graphdb_core::event_dispatch::SubscriptionId {
        self.ctx.register_schema_callback(callback)
    }

    /// Remove a previously registered schema-change observer.
    pub fn unregister_schema_callback(
        &self,
        id: graphdb_core::event_dispatch::SubscriptionId,
    ) -> bool {
        self.ctx.unregister_schema_callback(id)
    }

    /// Number of registered schema-change observers (diagnostics).
    pub fn schema_callback_count(&self) -> usize {
        self.ctx.schema_callback_count()
    }

    /// Shared schema-event registry for the central `HookBus`.
    pub fn shared_schema_callbacks(
        &self,
    ) -> Arc<
        graphdb_core::event_dispatch::EventSubscriptions<graphdb_core::metadata::SchemaChangeEvent>,
    > {
        self.ctx.shared_schema_callbacks()
    }

    /// Register a storage-lifecycle observer on the persistence coordinator.
    ///
    /// Returns `None` when no persistence coordinator is attached (e.g.
    /// pure in-memory mode); otherwise returns the subscription id, which
    /// can be passed to `unregister_storage_callback`.
    pub fn register_storage_callback(
        &self,
        callback: crate::engine::persistence_coordinator::StorageEventCallback,
    ) -> Option<graphdb_core::event_dispatch::SubscriptionId> {
        self.ctx
            .persistence()
            .as_ref()
            .map(|persistence| persistence.read().register_storage_callback(callback))
    }

    /// Register a filtered storage-lifecycle observer (only invoked when
    /// `filter` returns true). Returns `None` without a coordinator.
    pub fn register_storage_callback_filtered(
        &self,
        callback: crate::engine::persistence_coordinator::StorageEventCallback,
        filter: graphdb_core::event_dispatch::EventFilter<
            crate::engine::persistence_coordinator::StorageEvent,
        >,
    ) -> Option<graphdb_core::event_dispatch::SubscriptionId> {
        self.ctx.persistence().as_ref().map(|persistence| {
            persistence
                .read()
                .register_storage_callback_filtered(callback, filter)
        })
    }

    /// Remove a previously registered storage-lifecycle observer.
    /// Returns false when unknown or without a coordinator.
    pub fn unregister_storage_callback(
        &self,
        id: graphdb_core::event_dispatch::SubscriptionId,
    ) -> bool {
        if let Some(persistence) = self.ctx.persistence() {
            persistence.read().unregister_storage_callback(id)
        } else {
            false
        }
    }

    /// Number of registered storage-lifecycle observers (diagnostics).
    pub fn storage_callback_count(&self) -> usize {
        if let Some(persistence) = self.ctx.persistence() {
            persistence.read().storage_callback_count()
        } else {
            0
        }
    }

    /// Shared storage-event registry for the central `HookBus`.
    /// Returns `None` without an attached persistence coordinator.
    pub fn shared_storage_callbacks(
        &self,
    ) -> Option<
        Arc<
            graphdb_core::event_dispatch::EventSubscriptions<
                crate::engine::persistence_coordinator::StorageEvent,
            >,
        >,
    > {
        self.ctx
            .persistence()
            .as_ref()
            .map(|persistence| persistence.read().shared_storage_callbacks())
    }

    /// Batch delete vertices by external string IDs.
    pub fn batch_delete_vertices(
        &self,
        label: graphdb_core::types::LabelId,
        external_ids: &[&str],
        ts: graphdb_core::types::Timestamp,
    ) -> graphdb_core::StorageResult<usize> {
        self.ctx.batch_delete_vertices(label, external_ids, ts)
    }

    /// Batch delete vertices by external i64 IDs.
    pub fn batch_delete_vertices_by_i64(
        &self,
        label: graphdb_core::types::LabelId,
        external_ids: &[i64],
        ts: graphdb_core::types::Timestamp,
    ) -> graphdb_core::StorageResult<usize> {
        self.ctx
            .batch_delete_vertices_by_i64(label, external_ids, ts)
    }

    /// Batch-delete multiple vertices together with all their incident edges.
    pub fn batch_delete_vertices_with_edges(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
    ) -> graphdb_core::StorageResult<usize> {
        self.ctx.check_write_admission()?;
        crate::engine::graph_storage::writer::batch_delete_vertices_with_edges(
            &self.ctx, space, tag, ids,
        )
    }
}

impl crate::client::StorageSnapshotOps for GraphStorage {
    fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.ctx.get_freeze_stats()
    }

    fn trigger_background_freeze(&self) -> StorageResult<()> {
        self.ctx.trigger_background_freeze()
    }
}

impl graphdb_transaction::UndoTarget for GraphStorage {
    fn delete_vertex_type(
        &self,
        label: graphdb_core::types::LabelId,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::delete_vertex_type(&*self.ctx, label)
    }

    fn delete_edge_type(
        &self,
        edge_key: graphdb_core::types::EdgeKey,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::delete_edge_type(&*self.ctx, edge_key)
    }

    fn delete_vertex(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::delete_vertex(&*self.ctx, vertex, ts)
    }

    fn delete_edge(
        &self,
        edge_ctx: graphdb_core::types::EdgeDeletionContext,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::delete_edge(&*self.ctx, edge_ctx)
    }

    fn restore_edge(
        &self,
        edge: graphdb_core::types::EdgeIdentifier,
        properties: Vec<(String, graphdb_core::Value)>,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::restore_edge(&*self.ctx, edge, properties, ts)
    }

    fn undo_update_vertex_property(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        col_id: graphdb_core::types::ColumnId,
        value: graphdb_core::Value,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::undo_update_vertex_property(
            &*self.ctx, vertex, col_id, value, ts,
        )
    }

    fn undo_update_edge_property(
        &self,
        edge_id: graphdb_core::types::EdgeIdentifier,
        col_id: graphdb_core::types::ColumnId,
        value: graphdb_core::Value,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::undo_update_edge_property(
            &*self.ctx, edge_id, col_id, value, ts,
        )
    }

    fn revert_delete_vertex(
        &self,
        vertex: graphdb_core::types::VertexIdentifier,
        ts: graphdb_transaction::wal::Timestamp,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_vertex(&*self.ctx, vertex, ts)
    }

    fn revert_delete_edge(
        &self,
        edge_ctx: graphdb_core::types::EdgeDeletionContext,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_edge(&*self.ctx, edge_ctx)
    }

    fn revert_delete_vertex_properties(
        &self,
        label_name: &str,
        prop_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_vertex_properties(
            &*self.ctx, label_name, prop_names,
        )
    }

    fn revert_delete_edge_properties(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
        prop_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_edge_properties(
            &*self.ctx, src_label, dst_label, edge_label, prop_names,
        )
    }

    fn revert_delete_vertex_label(
        &self,
        label_name: &str,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_vertex_label(&*self.ctx, label_name)
    }

    fn revert_delete_edge_label(
        &self,
        src_label: &str,
        dst_label: &str,
        edge_label: &str,
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_delete_edge_label(
            &*self.ctx, src_label, dst_label, edge_label,
        )
    }

    fn revert_rename_vertex_properties(
        &self,
        label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_rename_vertex_properties(
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
    ) -> graphdb_transaction::undo_log::UndoLogResult<()> {
        graphdb_core::types::UndoTarget::revert_rename_edge_properties(
            &*self.ctx,
            src_label,
            dst_label,
            edge_label,
            current_names,
            original_names,
        )
    }
}
