//! Storage Interface Implementation
//!
//! Implements the StorageClient trait for the storage engine.
//! This module acts as an adapter layer between the high-level StorageClient API
//! and the low-level storage engine.

pub mod context;
mod cursor_impl;
mod index_engine;
mod index_manager;
mod ops;
mod persistence;
mod reader;
mod schema_engine;
mod schema_writer;
mod writer;

#[cfg(test)]
mod tests;

pub use context::GraphStorageContext;

use std::path::PathBuf;
use std::sync::Arc;

use crate::core::metadata::{IndexMetadataManager, SchemaManager};
use crate::core::stats::StatsManager;
use crate::core::types::{
    CommitLsn, CompactConfig, EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId,
    PasswordInfo, PropertyDef, SnapshotTimestamp, SpaceInfo, TagInfo, Timestamp, UpdateInfo,
    UserAlterInfo, UserInfo, VertexId,
};
use crate::core::{Edge, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex};
use crate::storage::cursor::{
    EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor,
};
use crate::storage::engine::background_freeze::{BackgroundFreezeManager, FreezeStats};
use crate::storage::engine::graph_storage::context::ExportedEdgeSnapshotRecord;
use crate::storage::engine::PersistenceConfig;
use crate::storage::index::index_data_manager::IndexIdentity;
use crate::storage::index::key_codec::KeyBuilder;
use crate::storage::index::IndexGcConfig;
use crate::storage::{
    StorageAdmin, StorageAuthOps, StorageGcOps, StorageOperationContext,
    StorageOperationContextOps, StoragePersistenceOps, StorageReader, StorageRecoveryOps,
    StorageSchemaContextOps, StorageSchemaOps, StorageStats, StorageSyncContextOps, StorageWriter,
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
    fn commit_auto_if_needed(&self) -> StorageResult<()> {
        let Some(context) = self.ctx.operation_context() else {
            return Ok(());
        };
        if !context.auto_commit || context.read_only {
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
        config: crate::storage::engine::config::PropertyGraphConfig,
    ) -> StorageResult<Self> {
        Ok(Self {
            ctx: Arc::new(GraphStorageContext::new_with_config(config)?),
        })
    }

    /// Create with development configuration (small thresholds, conservative freeze).
    pub fn new_development() -> StorageResult<Self> {
        Self::new_with_config(crate::storage::engine::config::PropertyGraphConfig::development())
    }

    /// Create with production configuration for small systems.
    pub fn new_production_small() -> StorageResult<Self> {
        Self::new_with_config(
            crate::storage::engine::config::PropertyGraphConfig::production_small(),
        )
    }

    /// Create with production configuration for large systems (LSM tiered freeze).
    pub fn new_production_large() -> StorageResult<Self> {
        Self::new_with_config(
            crate::storage::engine::config::PropertyGraphConfig::production_large(),
        )
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
        property_config: crate::storage::engine::config::PropertyGraphConfig,
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
        let new_ctx = (*self.ctx)
            .clone()
            .with_background_freeze(Arc::clone(&manager));
        self.ctx = Arc::new(new_ctx);
        self
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>> {
        self.ctx.export_snapshot(ts)
    }

    pub fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.ctx.get_freeze_stats()
    }

    /// Return current and peak memory usage by storage ownership category.
    pub fn resource_snapshot(&self) -> crate::storage::ResourceSnapshot {
        self.ctx.resource_snapshot()
    }

    /// Return WAL durability positions and sync counters when WAL is enabled.
    pub fn wal_metrics(&self) -> Option<crate::storage::WalMetrics> {
        self.ctx.wal_metrics()
    }

    /// Check whether another active snapshot may be registered.
    pub fn check_snapshot_admission(&self) -> StorageResult<()> {
        self.ctx.check_snapshot_admission()
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        self.ctx.trigger_background_freeze()
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
        let snapshot_timestamp =
            SnapshotTimestamp::new(u64::from(self.ctx.get_read_timestamp().max(1)));
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
            crate::core::types::IndexType::TagIndex => {
                KeyBuilder::build_vertex_index_value_prefix(space_id, index_name, value)?.0
            }
            crate::core::types::IndexType::EdgeIndex => {
                KeyBuilder::build_edge_index_value_prefix(space_id, index_name, value)?.0
            }
        };
        self.split_native_index(space, index_name, boundary)
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

    fn scan_edges_by_type_paginated(
        &self,
        space: &str,
        edge_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        reader::scan_edges_by_type_paginated(&self.ctx, space, edge_type, offset, limit)
    }

    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError> {
        reader::count_vertices_by_tag(&self.ctx, space, tag)
    }

    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
        reader::count_edges_by_type(&self.ctx, space, edge_type)
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

    fn get_edge_index(&self, space: &str, index_name: &str) -> Result<Option<Index>, StorageError> {
        index_manager::get_edge_index(&self.ctx, space, index_name)
    }

    fn list_edge_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError> {
        index_manager::list_edge_indexes(&self.ctx, space)
    }

    fn get_vertex_version_history(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Option<crate::storage::LabelVersionHistory>, StorageError> {
        let tag_info = self.ctx.schema_manager().get_tag(space, tag)?;
        let tag_info = tag_info.ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;

        let history = self.ctx.data_store().with_vertex_tables(|vertex_tables| {
            let table = vertex_tables
                .get(&tag_info.tag_id)
                .ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;
            let version_history = table.version_history_ref();
            let guard = version_history
                .lock()
                .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;
            Ok::<_, StorageError>(guard.clone())
        })?;
        Ok(Some(history))
    }

    fn get_edge_version_history(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Option<crate::storage::LabelVersionHistory>, StorageError> {
        let edge_info = self.ctx.schema_manager().get_edge_type(space, edge_type)?;
        let edge_info =
            edge_info.ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;

        let keys = {
            self.ctx
                .data_store()
                .catalog_read_snapshot()
                .with_edge_label_index(|edge_label_index| {
                    edge_label_index
                        .get(&edge_info.edge_type_id)
                        .cloned()
                        .ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))
                })?
        };

        if keys.is_empty() {
            return Err(StorageError::label_not_found(edge_type.to_string()));
        }

        let key = &keys[0];

        let history = self.ctx.data_store().with_edge_tables(|edge_tables| {
            let table = edge_tables
                .get(key)
                .ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;
            let version_history = table.version_history_ref();
            let guard = version_history
                .lock()
                .map_err(|_| StorageError::db_error("Failed to lock version_history"))?;
            Ok::<_, StorageError>(guard.clone())
        })?;
        Ok(Some(history))
    }

    fn get_vertex_schema_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::storage::PropertyChange>, StorageError> {
        let history = self.get_vertex_version_history(space, tag)?;
        let history = history.ok_or_else(|| StorageError::label_not_found(tag.to_string()))?;

        let mut changes = Vec::new();
        for version in history.get_versions() {
            if version > from_version && version <= to_version {
                if let Some(version_changes) = history.change_log.get_version_changes(version) {
                    changes.extend(version_changes.iter().cloned());
                }
            }
        }

        Ok(changes)
    }

    fn get_edge_schema_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::storage::PropertyChange>, StorageError> {
        let history = self.get_edge_version_history(space, edge_type)?;
        let history =
            history.ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;

        let mut changes = Vec::new();
        for version in history.get_versions() {
            if version > from_version && version <= to_version {
                if let Some(version_changes) = history.change_log.get_version_changes(version) {
                    changes.extend(version_changes.iter().cloned());
                }
            }
        }

        Ok(changes)
    }

    fn detect_vertex_breaking_changes(
        &self,
        space: &str,
        tag: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::storage::PropertyChange>, StorageError> {
        let changes = self.get_vertex_schema_changes(space, tag, from_version, to_version)?;
        Ok(changes.into_iter().filter(|c| c.is_breaking()).collect())
    }

    fn detect_edge_breaking_changes(
        &self,
        space: &str,
        edge_type: &str,
        from_version: u64,
        to_version: u64,
    ) -> Result<Vec<crate::storage::PropertyChange>, StorageError> {
        let changes = self.get_edge_schema_changes(space, edge_type, from_version, to_version)?;
        Ok(changes.into_iter().filter(|c| c.is_breaking()).collect())
    }

    fn create_vertex_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn VertexCursor>, StorageError> {
        let cursor =
            cursor_impl::GraphVertexCursor::new(self.ctx.clone(), space.to_string(), options)?;
        Ok(Box::new(cursor))
    }

    fn create_edge_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn EdgeCursor>, StorageError> {
        let cursor = cursor_impl::GraphEdgeCursor::new(self.ctx.clone(), space, options)?;
        Ok(Box::new(cursor))
    }

    fn create_index_cursor(
        &self,
        plan: &IndexScanPlan,
    ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
        let tag_indexes = self.list_tag_indexes(&plan.space)?;
        let edge_indexes = self.list_edge_indexes(&plan.space)?;
        let index_id = plan.index_id;
        if let Some(index) = tag_indexes.into_iter().find(|index| index.id == index_id) {
            let space_id = self.ctx.schema_manager().get_space_id(&plan.space)?;
            let space_name = plan.space.clone();
            let ctx = self.ctx.clone();
            let stale_checker: Option<crate::storage::index::index_data_manager::StaleChecker> =
                Some(Arc::new(
                    move |entity_ref, _entity_version| match entity_ref {
                        crate::core::wal::EntityRef::Vertex(vid) => {
                            reader::get_vertex(&ctx, &space_name, vid)
                                .ok()
                                .flatten()
                                .is_some()
                        }
                        crate::core::wal::EntityRef::Edge { .. } => true,
                    },
                ));
            let cursor = self
                .ctx
                .index_data_manager()
                .read()
                .open_tag_index_cursor_full(space_id, &index, plan, stale_checker, None)?;
            return Ok(Box::new(cursor));
        }

        if let Some(index) = edge_indexes.into_iter().find(|index| index.id == index_id) {
            let space_id = self.ctx.schema_manager().get_space_id(&plan.space)?;
            let space_name = plan.space.clone();
            let ctx = self.ctx.clone();
            let stale_checker: Option<crate::storage::index::index_data_manager::StaleChecker> =
                Some(Arc::new(
                    move |entity_ref, _entity_version| match entity_ref {
                        crate::core::wal::EntityRef::Vertex(vid) => {
                            reader::get_vertex(&ctx, &space_name, vid)
                                .ok()
                                .flatten()
                                .is_some()
                        }
                        crate::core::wal::EntityRef::Edge { .. } => true,
                    },
                ));
            let cursor = self
                .ctx
                .index_data_manager()
                .read()
                .open_edge_index_cursor_full(space_id, &index, plan, stale_checker, None)?;
            return Ok(Box::new(cursor));
        }

        Err(StorageError::not_found(format!(
            "Index {} not found in space {}",
            plan.index_id, plan.space
        )))
    }
}

impl GraphStorage {
    /// Aggregate all label version histories into a single `SchemaVersionHistory`.
    pub fn aggregate_schema_version_history(
        &self,
        space: &str,
    ) -> Result<crate::storage::schema::version_history::SchemaVersionHistory, StorageError> {
        use crate::storage::schema::version_history::SchemaVersionHistory;
        let mut schema_history = SchemaVersionHistory::new();

        let tag_infos = self.ctx.schema_manager().list_tags(space)?;
        for tag_info in tag_infos {
            if let Some(history) = self.get_vertex_version_history(space, &tag_info.tag_name)? {
                schema_history.add_vertex_history(history);
            }
        }

        let edge_infos = self.ctx.schema_manager().list_edge_types(space)?;
        for edge_info in edge_infos {
            if let Some(history) =
                self.get_edge_version_history(space, &edge_info.edge_type_name)?
            {
                schema_history.add_edge_history(history);
            }
        }

        Ok(schema_history)
    }
}

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

    fn delete_vertex(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::delete_vertex(&self.ctx, space, id)?;
        self.commit_auto_if_needed()
    }

    fn delete_vertex_with_edges(&mut self, space: &str, id: &VertexId) -> Result<(), StorageError> {
        self.ctx.check_write_admission()?;
        writer::delete_vertex_with_edges(&self.ctx, space, id)?;
        self.commit_auto_if_needed()
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

    fn delete_tags(
        &mut self,
        space: &str,
        vertex_id: &VertexId,
        tag_names: &[String],
    ) -> Result<usize, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::delete_tags(&self.ctx, space, vertex_id, tag_names)?;
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

    fn delete_vertex_data(&mut self, space: &str, vertex_id: &str) -> Result<bool, StorageError> {
        self.ctx.check_write_admission()?;
        let result = writer::delete_vertex_data(&self.ctx, space, vertex_id)?;
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
        schema_writer::create_tag_index(&self.ctx, space, index)
    }

    fn drop_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        schema_writer::drop_tag_index(&self.ctx, space, index_name)
    }

    fn rebuild_tag_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        // Keep the exclusive rebuild guard from the table snapshot through
        // WAL catch-up and generation publication. Index writers take the
        // read side in index_engine, so they cannot update the old generation
        // after this snapshot has been taken.
        let rebuild_gate = self.ctx.index_data_manager().read().rebuild_gate();
        let _rebuild_guard = rebuild_gate.write();
        let snapshot_timestamp = self.ctx.get_read_timestamp();
        let start_lsn = match index_manager::current_wal_lsn(&self.ctx) {
            crate::core::types::CommitLsn::ZERO => crate::core::types::CommitLsn::new(1),
            lsn => lsn,
        };
        let snapshot_ctx = self.ctx.with_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: snapshot_timestamp,
            write_timestamp: None,
            read_only: true,
            auto_commit: false,
        });
        let vertices = reader::scan_vertices(&snapshot_ctx, space)?;
        let result = index_manager::rebuild_tag_index(
            &self.ctx,
            space,
            index_name,
            &vertices,
            crate::core::types::SnapshotTimestamp::new(u64::from(snapshot_timestamp)),
            start_lsn,
        );
        if let Some(stats) = self.ctx.stats_manager() {
            if result.is_err() {
                stats.record_generation_rebuild_failure();
            }
        }
        result
    }

    fn create_edge_index(&mut self, space: &str, index: &Index) -> Result<bool, StorageError> {
        schema_writer::create_edge_index(&self.ctx, space, index)
    }

    fn drop_edge_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        schema_writer::drop_edge_index(&self.ctx, space, index_name)
    }

    fn rebuild_edge_index(&mut self, space: &str, index_name: &str) -> Result<bool, StorageError> {
        // Keep the exclusive rebuild guard from the table snapshot through
        // WAL catch-up and generation publication. Index writers take the
        // read side in index_engine, so they cannot update the old generation
        // after this snapshot has been taken.
        let rebuild_gate = self.ctx.index_data_manager().read().rebuild_gate();
        let _rebuild_guard = rebuild_gate.write();
        let snapshot_timestamp = self.ctx.get_read_timestamp();
        let start_lsn = match index_manager::current_wal_lsn(&self.ctx) {
            crate::core::types::CommitLsn::ZERO => crate::core::types::CommitLsn::new(1),
            lsn => lsn,
        };
        let snapshot_ctx = self.ctx.with_operation_context(StorageOperationContext {
            transaction_id: None,
            read_timestamp: snapshot_timestamp,
            write_timestamp: None,
            read_only: true,
            auto_commit: false,
        });
        let edges = reader::scan_all_edges(&snapshot_ctx, space)?;
        let result = index_manager::rebuild_edge_index(
            &self.ctx,
            space,
            index_name,
            &edges,
            crate::core::types::SnapshotTimestamp::new(u64::from(snapshot_timestamp)),
            start_lsn,
        );
        if let Some(stats) = self.ctx.stats_manager() {
            if result.is_err() {
                stats.record_generation_rebuild_failure();
            }
        }
        result
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

    fn persistence_diagnostics(&self) -> Option<crate::storage::PersistenceDiagnostics> {
        persistence::persistence_diagnostics(&self.ctx)
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

    fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>> {
        Some(self.ctx.index_metadata_manager().clone())
    }
}

impl StorageOperationContextOps for GraphStorage {
    fn bind_auto_commit_context(&self) -> Self {
        Self {
            ctx: Arc::new(self.ctx.with_auto_commit_context()),
        }
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        Self {
            ctx: Arc::new(self.ctx.with_operation_context(context)),
        }
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.ctx.operation_context()
    }
}

impl crate::storage::StorageCommitOps for GraphStorage {
    fn commit_staged_writes(
        &self,
        transaction_id: crate::core::types::TransactionId,
        intents: &[crate::core::wal::OutboxIntent],
    ) -> StorageResult<crate::core::types::CommitLsn> {
        self.ctx.commit_staged_writes(transaction_id, intents)
    }

    fn abort_staged_writes(
        &self,
        transaction_id: crate::core::types::TransactionId,
    ) -> StorageResult<()> {
        self.ctx.abort_staged_writes(transaction_id);
        Ok(())
    }

    fn recover_outbox_projection(
        &self,
        sync_manager: &crate::sync::SyncManager,
    ) -> StorageResult<usize> {
        use crate::transaction::wal::{collect_committed_transactions, LocalWalParser, WalParser};
        let Some(paths) = self.ctx.storage_paths() else {
            return Ok(0);
        };
        if !paths.wal_dir().exists() {
            return Ok(0);
        }

        // SyncManager restores the same root-level outbox snapshot before it
        // opens `outbox/outbox.sqlite`. Only the SQLite projection's durable
        // frontier is a valid WAL replay lower bound; a merely discovered
        // snapshot must never cause replay to skip data.
        let snapshot_lsn = sync_manager.outbox_materialized_lsn().map_err(|error| {
            StorageError::db_error(format!(
                "Failed to read outbox materialization frontier: {}",
                error
            ))
        })?;

        // Step 2: Parse committed WAL transactions
        let mut parser = LocalWalParser::new();
        parser
            .open(&paths.wal_dir().to_string_lossy())
            .map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to parse WAL for outbox recovery: {}",
                    error
                ))
            })?;
        let transactions =
            collect_committed_transactions(&parser.parse_all_entries()).map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to validate WAL for outbox recovery: {}",
                    error
                ))
            })?;

        // Step 3: Materialize only intents after the snapshot LSN
        let mut recovered = 0usize;
        for transaction in transactions {
            if transaction.intents.is_empty() {
                continue;
            }
            // Skip transactions already covered by the restored snapshot
            if let Some(snapshot_lsn) = snapshot_lsn {
                if transaction.commit_lsn <= snapshot_lsn {
                    continue;
                }
            }
            sync_manager
                .materialize_committed_transaction(
                    transaction.transaction_id,
                    transaction.commit_lsn,
                    &transaction.intents,
                )
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to recover outbox transaction {}: {}",
                        transaction.transaction_id, error
                    ))
                })?;
            recovered = recovered.saturating_add(transaction.intents.len());
        }

        log::info!(
            "Outbox projection recovery complete: {} intents replayed (snapshot_lsn={:?})",
            recovered,
            snapshot_lsn
        );

        Ok(recovered)
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
