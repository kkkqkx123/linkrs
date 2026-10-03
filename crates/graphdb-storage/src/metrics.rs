use std::sync::Arc;

use crate::cursor::{EdgeCursor, IndexCursor, IndexRow, IndexScanPlan, ScanOptions, VertexCursor};
use crate::macros::{
    forward_methods, forward_timed_read_methods, forward_timed_write_methods, forward_undo_methods,
};
use crate::{
    StorageAdmin, StorageAuthOps, StorageClient, StorageCommitOps, StorageGcOps,
    StorageOperationContext, StorageOperationContextOps, StoragePersistenceOps, StorageReader,
    StorageRecoveryOps, StorageSchemaContextOps, StorageSchemaOps, StorageSnapshotOps,
    StorageStats, StorageSyncContextOps, StorageWriter,
};
use graphdb_core::metadata::{IndexMetadataManager, SchemaManager};
use graphdb_core::types::{
    EdgeTypeInfo, Index, InsertEdgeInfo, InsertVertexInfo, LabelId, PasswordInfo, PropertyDef,
    SpaceInfo, TagInfo, UpdateInfo, UserAlterInfo, UserInfo, VertexId,
};
use graphdb_core::{
    Edge, EdgeDeleteKey, EdgeDirection, RoleType, StorageError, StorageResult, Value, Vertex,
};
use graphdb_sync::SyncManager;

pub struct MetricsStorage<S: StorageClient> {
    inner: S,
    stats: Option<Arc<graphdb_metrics::StatsManager>>,
}

impl<S: StorageClient> MetricsStorage<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, stats: None }
    }

    pub fn with_stats(inner: S, stats: Arc<graphdb_metrics::StatsManager>) -> Self {
        Self {
            inner,
            stats: Some(stats),
        }
    }

    pub fn into_inner(self) -> S {
        self.inner
    }

    fn record_read(&self, start: std::time::Instant) {
        if let Some(stats) = &self.stats {
            stats.record_storage_read(start.elapsed().as_micros() as u64);
        }
    }

    fn record_write(&self, start: std::time::Instant) {
        if let Some(stats) = &self.stats {
            stats.record_storage_write(start.elapsed().as_micros() as u64);
        }
    }

    fn timed_read<T>(
        &self,
        run: impl FnOnce() -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let start = std::time::Instant::now();
        let result = run();
        if result.is_err() {
            if let Some(stats) = &self.stats {
                stats.record_storage_error();
            }
        }
        self.record_read(start);
        result
    }
}

impl<S: StorageClient> crate::stats_reader::ColumnStatsReader for MetricsStorage<S> {
    fn vertex_column_stats(
        &self,
        space: &str,
        tag: &str,
        column: &str,
    ) -> Option<Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        self.inner.vertex_column_stats(space, tag, column)
    }

    fn edge_column_stats(
        &self,
        space: &str,
        edge_type: &str,
        column: &str,
    ) -> Option<Arc<crate::stats_reader::ColumnStatsSnapshot>> {
        self.inner.edge_column_stats(space, edge_type, column)
    }

    fn vertex_table_stats(
        &self,
        space: &str,
        tag: &str,
    ) -> Option<Arc<crate::stats_reader::TableCardinalitySnapshot>> {
        self.inner.vertex_table_stats(space, tag)
    }

    fn edge_table_stats(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Option<Arc<crate::stats_reader::TableCardinalitySnapshot>> {
        self.inner.edge_table_stats(space, edge_type)
    }

    fn stats_epoch(&self) -> u64 {
        self.inner.stats_epoch()
    }
}

impl<S: StorageClient + crate::AutoCommitBatchOps> crate::AutoCommitBatchOps for MetricsStorage<S> {
    fn begin_auto_commit_batch(&self) -> StorageResult<Arc<crate::AutoCommitBatchWindow>> {
        self.inner.begin_auto_commit_batch()
    }

    fn bind_auto_commit_statement(
        &self,
        window: &Arc<crate::AutoCommitBatchWindow>,
    ) -> StorageResult<Self> {
        let inner = self.inner.bind_auto_commit_statement(window)?;
        Ok(Self {
            inner,
            stats: self.stats.clone(),
        })
    }

    fn finalize_auto_commit_batch(
        &self,
        window: &crate::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        self.inner.finalize_auto_commit_batch(window)
    }
}

impl<S: StorageClient + crate::AutoCommitGroupOps> crate::AutoCommitGroupOps for MetricsStorage<S> {
    fn begin_auto_commit_group(&self) -> StorageResult<Arc<crate::AutoCommitBatchWindow>> {
        self.inner.begin_auto_commit_group()
    }

    fn finalize_auto_commit_group(
        &self,
        window: &crate::AutoCommitBatchWindow,
    ) -> StorageResult<()> {
        self.inner.finalize_auto_commit_group(window)
    }
}

impl<S: StorageClient> StorageReader for MetricsStorage<S> {
    forward_timed_read_methods!(inner;
        fn get_vertex(&self, space: &str, tag: &str, id: &VertexId) -> Result<Option<Vertex>, StorageError>;
        fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError>;
    );

    fn get_vertex_projected(
        &self,
        space: &str,
        tag: &str,
        id: &VertexId,
        projection: &[String],
    ) -> Result<Option<Vertex>, StorageError> {
        self.timed_read(|| self.inner.get_vertex_projected(space, tag, id, projection))
    }

    fn scan_vertices(&self, space: &str) -> Result<Vec<Vertex>, StorageError> {
        self.timed_read(|| self.inner.scan_vertices(space))
    }

    fn get_edge(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
    ) -> Result<Option<Edge>, StorageError> {
        self.timed_read(|| self.inner.get_edge(space, src, dst, edge_type, rank))
    }

    fn get_node_edges(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| {
            self.inner
                .get_node_edges(space, node_id, direction, edge_types)
        })
    }

    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| self.inner.scan_edges_by_type(space, edge_type))
    }

    fn scan_all_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| self.inner.scan_all_edges(space))
    }

    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError> {
        self.timed_read(|| self.inner.count_vertices_by_tag(space, tag))
    }

    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
        self.timed_read(|| self.inner.count_edges_by_type(space, edge_type))
    }

    fn scan_vertices_by_tag_paginated(
        &self,
        space: &str,
        tag: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Vertex>, StorageError> {
        self.timed_read(|| {
            self.inner
                .scan_vertices_by_tag_paginated(space, tag, offset, limit)
        })
    }

    fn scan_vertices_by_prop(
        &self,
        space: &str,
        tag: &str,
        prop: &str,
        value: &Value,
    ) -> Result<Vec<Vertex>, StorageError> {
        self.timed_read(|| self.inner.scan_vertices_by_prop(space, tag, prop, value))
    }

    fn get_edge_projected(
        &self,
        space: &str,
        src: &VertexId,
        dst: &VertexId,
        edge_type: &str,
        rank: i64,
        projection: &[String],
    ) -> Result<Option<Edge>, StorageError> {
        self.timed_read(|| {
            self.inner
                .get_edge_projected(space, src, dst, edge_type, rank, projection)
        })
    }

    fn get_node_edges_projected(
        &self,
        space: &str,
        node_id: &VertexId,
        direction: EdgeDirection,
        edge_types: &[String],
        projection: Option<&[String]>,
        limit: Option<usize>,
    ) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| {
            self.inner
                .get_node_edges_projected(space, node_id, direction, edge_types, projection, limit)
        })
    }

    fn get_vertices_batch(
        &self,
        space: &str,
        tag: &str,
        ids: &[VertexId],
    ) -> Result<Vec<Option<Vertex>>, StorageError> {
        self.timed_read(|| self.inner.get_vertices_batch(space, tag, ids))
    }

    fn neighbor_dst_ids_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<Vec<VertexId>>, StorageError> {
        self.timed_read(|| {
            self.inner
                .neighbor_dst_ids_batch(space, src_ids, direction, edge_types)
        })
    }

    fn out_degree_batch(
        &self,
        space: &str,
        src_ids: &[VertexId],
        direction: EdgeDirection,
        edge_types: &[String],
    ) -> Result<Vec<usize>, StorageError> {
        self.timed_read(|| {
            self.inner
                .out_degree_batch(space, src_ids, direction, edge_types)
        })
    }

    fn scan_edges_by_type_paginated(
        &self,
        space: &str,
        edge_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| {
            self.inner
                .scan_edges_by_type_paginated(space, edge_type, offset, limit)
        })
    }

    fn lookup_edges_by_property_range(
        &self,
        space: &str,
        edge_type: &str,
        prop_name: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Result<Vec<Edge>, StorageError> {
        self.timed_read(|| {
            self.inner.lookup_edges_by_property_range(
                space,
                edge_type,
                prop_name,
                lower,
                upper,
                include_lower,
                include_upper,
            )
        })
    }

    fn lookup_index(
        &self,
        space: &str,
        index: &str,
        value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        self.timed_read(|| self.inner.lookup_index(space, index, value))
    }

    fn get_vertex_with_schema(
        &self,
        space: &str,
        tag: &str,
        id: &Value,
    ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> {
        self.timed_read(|| self.inner.get_vertex_with_schema(space, tag, id))
    }

    fn get_edge_with_schema(
        &self,
        space: &str,
        edge_type: &str,
        src: &Value,
        dst: &Value,
    ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        self.timed_read(|| self.inner.get_edge_with_schema(space, edge_type, src, dst))
    }

    fn scan_vertices_with_schema(
        &self,
        space: &str,
        tag: &str,
    ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> {
        self.timed_read(|| self.inner.scan_vertices_with_schema(space, tag))
    }

    fn scan_edges_with_schema(
        &self,
        space: &str,
        edge_type: &str,
    ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        self.timed_read(|| self.inner.scan_edges_with_schema(space, edge_type))
    }

    fn create_vertex_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn VertexCursor>, StorageError> {
        self.timed_read(|| self.inner.create_vertex_cursor(space, options))
    }

    fn create_edge_cursor(
        &self,
        space: &str,
        options: &ScanOptions,
    ) -> Result<Box<dyn EdgeCursor>, StorageError> {
        self.timed_read(|| self.inner.create_edge_cursor(space, options))
    }

    fn create_index_cursor(
        &self,
        plan: &IndexScanPlan,
    ) -> Result<Box<dyn IndexCursor<Row = IndexRow>>, StorageError> {
        self.timed_read(|| self.inner.create_index_cursor(plan))
    }

    forward_methods!(inner;
        fn layout_version(&self) -> u64;
        fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>>;
        fn enable_edge_property_index(&self, space: &str, edge_type: &str, pool_capacity: u64) -> Result<bool, StorageError>;
        fn has_edge_property_index(&self, space: &str, edge_type: &str) -> Result<bool, StorageError>;
        fn disable_edge_property_index(&self, space: &str, edge_type: &str) -> Result<(), StorageError>;
        fn get_space(&self, space: &str) -> Result<Option<SpaceInfo>, StorageError>;
        fn get_space_by_id(&self, space_id: u64) -> Result<Option<SpaceInfo>, StorageError>;
        fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError>;
        fn get_space_id(&self, space: &str) -> Result<u64, StorageError>;
        fn space_exists(&self, space: &str) -> bool;
        fn get_tag(&self, space: &str, tag: &str) -> Result<Option<TagInfo>, StorageError>;
        fn list_tags(&self, space: &str) -> Result<Vec<TagInfo>, StorageError>;
        fn get_edge_type(&self, space: &str, edge_type: &str) -> Result<Option<EdgeTypeInfo>, StorageError>;
        fn list_edge_types(&self, space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError>;
        fn resolve_edge_type_name(&self, space: &str, hash: u32) -> Result<Option<String>, StorageError>;
        fn get_tag_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
        fn list_tag_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;
        fn get_edge_index(&self, space: &str, index: &str) -> Result<Option<Index>, StorageError>;
        fn list_edge_indexes(&self, space: &str) -> Result<Vec<Index>, StorageError>;
        fn get_vertex_version_history(&self, space: &str, tag: &str) -> Result<Option<crate::LabelVersionHistory>, StorageError>;
        fn get_edge_version_history(&self, space: &str, edge_type: &str) -> Result<Option<crate::LabelVersionHistory>, StorageError>;
        fn get_vertex_schema_changes(&self, space: &str, tag: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn get_edge_schema_changes(&self, space: &str, edge_type: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn detect_vertex_breaking_changes(&self, space: &str, tag: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn detect_edge_breaking_changes(&self, space: &str, edge_type: &str, from_version: u64, to_version: u64) -> Result<Vec<crate::PropertyChange>, StorageError>;
        fn list_migration_history(&self, space: &str, label: &str, is_edge: bool) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError>;
        fn get_applied_versions(&self, space: &str, label: &str, is_edge: bool) -> Result<Vec<u64>, StorageError>;
        fn record_migration_history(&self, record: crate::MigrationHistoryRecord) -> Result<(), StorageError>;
        fn list_all_migration_history(&self) -> Result<Vec<crate::MigrationHistoryRecord>, StorageError>;
    );
}

impl<S: StorageClient> StorageWriter for MetricsStorage<S> {
    forward_timed_write_methods!(inner;
        fn insert_vertex(&mut self, space: &str, vertex: Vertex) -> Result<VertexId, StorageError>;
        fn insert_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
        fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError>;
        fn update_vertex_replace(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError>;
        fn delete_vertex_with_edges(&mut self, space: &str, tag: &str, id: &VertexId) -> Result<(), StorageError>;
        fn batch_delete_vertices_with_edges(&mut self, space: &str, tag: &str, ids: &[VertexId]) -> Result<usize, StorageError>;
        fn batch_insert_vertices(&mut self, space: &str, vertices: Vec<Vertex>) -> Result<Vec<VertexId>, StorageError>;
        fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
        fn update_edge_replace(&mut self, space: &str, edge: Edge) -> Result<(), StorageError>;
        fn batch_insert_edges(&mut self, space: &str, edges: Vec<Edge>) -> Result<(), StorageError>;
        fn batch_delete_edges(&mut self, space: &str, deletes: &[EdgeDeleteKey]) -> Result<usize, StorageError>;
        fn insert_vertex_data(&mut self, space: &str, info: &InsertVertexInfo) -> Result<bool, StorageError>;
        fn insert_edge_data(&mut self, space: &str, info: &InsertEdgeInfo) -> Result<bool, StorageError>;
        fn delete_vertex_data(&mut self, space: &str, tag: &str, vertex_id: &str) -> Result<bool, StorageError>;
        fn delete_edge_data(&mut self, space: &str, src: &str, dst: &str, rank: i64) -> Result<bool, StorageError>;
        fn update_data(&mut self, space: &str, space_id: u64, info: &UpdateInfo) -> Result<bool, StorageError>;
        fn delete_vertex(&mut self, space: &str, tag: &str, id: &VertexId) -> Result<(), StorageError>;
        fn delete_edge(&mut self, space: &str, src: &VertexId, dst: &VertexId, edge_type: &str, rank: i64) -> Result<(), StorageError>;
    );
}

impl<S: StorageClient> StorageSchemaOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn create_space(&mut self, space: &mut SpaceInfo) -> Result<bool, StorageError>;
        fn drop_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn clear_space(&mut self, space: &str) -> Result<bool, StorageError>;
        fn alter_space_comment(&mut self, space_id: u64, comment: String) -> Result<bool, StorageError>;
        fn create_tag(&mut self, space: &str, tag: &TagInfo) -> Result<u32, StorageError>;
        fn alter_tag(&mut self, space: &str, tag: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>;
        fn rename_vertex_property(&mut self, label: LabelId, old_name: &str, new_name: &str) -> Result<(), StorageError>;
        fn rename_tag_property(&mut self, space: &str, tag: &str, old_name: &str, new_name: &str) -> Result<bool, StorageError>;
        fn rename_tag(&mut self, space: &str, old_name: &str, new_name: &str) -> Result<bool, StorageError>;
        fn drop_tag(&mut self, space: &str, tag: &str) -> Result<bool, StorageError>;
        fn create_edge_type(&mut self, space: &str, edge: &EdgeTypeInfo) -> Result<u32, StorageError>;
        fn alter_edge_type(&mut self, space: &str, edge_type: &str, additions: Vec<PropertyDef>, deletions: Vec<String>) -> Result<bool, StorageError>;
        fn rename_edge_type(&mut self, space: &str, old_name: &str, new_name: &str) -> Result<bool, StorageError>;
        fn update_edge_endpoints(&mut self, space: &str, edge_type: &str, src_tag: &str, dst_tag: &str) -> Result<bool, StorageError>;
        fn drop_edge_type(&mut self, space: &str, edge_type: &str) -> Result<bool, StorageError>;
        fn create_tag_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
        fn drop_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_tag_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn create_edge_index(&mut self, space: &str, info: &Index) -> Result<bool, StorageError>;
        fn drop_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
        fn rebuild_edge_index(&mut self, space: &str, index: &str) -> Result<bool, StorageError>;
    );
}

impl<S: StorageClient> StorageAuthOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn change_password(&mut self, info: &PasswordInfo) -> Result<bool, StorageError>;
        fn create_user(&mut self, info: &UserInfo) -> Result<bool, StorageError>;
        fn alter_user(&mut self, info: &UserAlterInfo) -> Result<bool, StorageError>;
        fn drop_user(&mut self, username: &str) -> Result<bool, StorageError>;
        fn grant_role(&mut self, username: &str, space_id: u64, role: RoleType) -> Result<bool, StorageError>;
        fn revoke_role(&mut self, username: &str, space_id: u64) -> Result<bool, StorageError>;
    );

    forward_methods!(inner;
        fn user_exists(&self, username: &str) -> bool;
        fn list_users(&self) -> Vec<String>;
    );
}

impl<S: StorageClient> StorageAdmin for MetricsStorage<S> {
    forward_methods!(inner;
        fn load_from_disk(&mut self) -> Result<(), StorageError>;
        fn repair_dangling_edges(&mut self, space: &str) -> Result<usize, StorageError>;
    );

    forward_methods!(inner;
        fn save_to_disk(&self) -> Result<(), StorageError>;
        fn get_storage_stats(&self) -> StorageStats;
        fn get_db_path(&self) -> &str;
        fn find_dangling_edges(&self, space: &str) -> Result<Vec<Edge>, StorageError>;
    );
}

impl<S: StorageClient> StoragePersistenceOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn flush(&self) -> Result<(), StorageError>;
        fn save_data(&self) -> graphdb_core::StorageResult<()>;
        fn save_data_to_dir(&self, dir: &std::path::Path) -> graphdb_core::StorageResult<()>;
        fn create_checkpoint(&self) -> graphdb_core::StorageResult<Option<crate::CheckpointStats>>;
        fn verify_snapshot(&self, snapshot_id: u64) -> graphdb_core::StorageResult<bool>;
        fn cleanup_snapshots(&self) -> graphdb_core::StorageResult<usize>;
        fn snapshot_stats(&self) -> crate::SnapshotStats;
        fn persistence_diagnostics(&self) -> Option<crate::PersistenceDiagnostics>;
        fn compact(&self, config: &graphdb_core::types::CompactConfig) -> graphdb_core::StorageResult<()>;
        fn auto_flush_if_needed(&self) -> graphdb_core::StorageResult<bool>;
        fn auto_checkpoint_if_needed(&self) -> graphdb_core::StorageResult<Option<crate::CheckpointStats>>;
        fn should_flush(&self) -> bool;
        fn should_checkpoint(&self) -> bool;
    );
}

impl<S: StorageClient + StorageSchemaContextOps> StorageSchemaContextOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_schema_manager(&self) -> Option<Arc<SchemaManager>>;
        fn get_index_metadata_manager(&self) -> Option<Arc<dyn IndexMetadataManager>>;
    );
}

impl<S: StorageClient> StorageOperationContextOps for MetricsStorage<S> {
    fn bind_auto_commit_context(&self) -> StorageResult<Self> {
        Ok(Self {
            inner: self.inner.bind_auto_commit_context()?,
            stats: self.stats.clone(),
        })
    }

    fn bind_operation_context(&self, context: StorageOperationContext) -> Self {
        Self {
            inner: self.inner.bind_operation_context(context),
            stats: self.stats.clone(),
        }
    }

    fn bind_read_operation_context(&self) -> StorageResult<Self> {
        Ok(Self {
            inner: self.inner.bind_read_operation_context()?,
            stats: self.stats.clone(),
        })
    }

    fn operation_context(&self) -> Option<Arc<StorageOperationContext>> {
        self.inner.operation_context()
    }

    fn finalize_operation(&self, committed: bool) -> graphdb_core::StorageResult<()> {
        self.inner.finalize_operation(committed)
    }
}

impl<S: StorageClient> StorageCommitOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn commit_staged_writes(&self, transaction_id: graphdb_core::types::TransactionId, intents: &[graphdb_core::wal::OutboxIntent]) -> graphdb_core::StorageResult<graphdb_core::types::CommitLsn>;
        fn abort_staged_writes(&self, transaction_id: graphdb_core::types::TransactionId) -> graphdb_core::StorageResult<()>;
        fn recover_outbox_projection(&self, sync_manager: &graphdb_sync::SyncManager) -> graphdb_core::StorageResult<usize>;
    );
}

impl<S: StorageClient + StorageSyncContextOps> StorageSyncContextOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn get_sync_manager(&self) -> Option<Arc<SyncManager>>;
    );
}

impl<S: StorageClient> StorageRecoveryOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn needs_recovery(&self) -> bool;
        fn recover_from_wal(&self) -> graphdb_core::StorageResult<graphdb_transaction::wal::recovery::RecoveryStats>;
        fn recover_from_wal_with_config(
            &self,
            config: graphdb_transaction::wal::recovery::RecoveryConfig,
        ) -> graphdb_core::StorageResult<graphdb_transaction::wal::recovery::RecoveryStats>;
        fn init_with_recovery(&self) -> graphdb_core::StorageResult<Option<graphdb_transaction::wal::recovery::RecoveryStats>>;
    );
}

impl<S: StorageClient> StorageGcOps for MetricsStorage<S> {
    forward_methods!(inner;
        fn is_index_gc_running(&self) -> bool;
        fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle>;
    );

    forward_methods!(inner;
        fn stop_index_gc(&self);
    );
}

impl<S: StorageClient> std::fmt::Debug for MetricsStorage<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsStorage")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<S: StorageClient> Clone for MetricsStorage<S>
where
    S: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            stats: self.stats.clone(),
        }
    }
}

impl<S: crate::client::StorageClient + StorageSnapshotOps + 'static>
    crate::client::StorageSnapshotOps for MetricsStorage<S>
{
    fn get_freeze_stats(&self) -> Option<crate::engine::background_freeze::FreezeStats> {
        self.inner.get_freeze_stats()
    }

    fn trigger_background_freeze(&self) -> graphdb_core::StorageResult<()> {
        self.inner.trigger_background_freeze()
    }
}

impl<S: graphdb_transaction::UndoTarget + StorageClient> graphdb_transaction::UndoTarget
    for MetricsStorage<S>
{
    forward_undo_methods!(inner;
        fn delete_vertex_type(&self, label: graphdb_core::types::LabelId) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn delete_edge_type(&self, edge_key: graphdb_core::types::EdgeKey) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn delete_vertex(&self, vertex: graphdb_core::types::VertexIdentifier, ts: graphdb_transaction::wal::Timestamp) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn delete_edge(&self, edge_ctx: graphdb_core::types::EdgeDeletionContext) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn restore_edge(&self, edge: graphdb_core::types::EdgeIdentifier, properties: Vec<(String, graphdb_core::Value)>, ts: graphdb_transaction::wal::Timestamp) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn undo_update_edge_property(&self, edge_id: graphdb_core::types::EdgeIdentifier, col_id: graphdb_core::types::ColumnId, value: graphdb_core::Value, ts: graphdb_transaction::wal::Timestamp) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_delete_edge(&self, edge_ctx: graphdb_core::types::EdgeDeletionContext) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_delete_vertex_properties(&self, label_name: &str, prop_names: &[String]) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_delete_edge_properties(&self, src_label: &str, dst_label: &str, edge_label: &str, prop_names: &[String]) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_delete_vertex_label(&self, label_name: &str) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_delete_edge_label(&self, src_label: &str, dst_label: &str, edge_label: &str) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_rename_vertex_properties(&self, label_name: &str, current_names: &[String], original_names: &[String]) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn revert_rename_edge_properties(&self, src_label: &str, dst_label: &str, edge_label: &str, current_names: &[String], original_names: &[String]) -> graphdb_transaction::undo_log::UndoLogResult<()>;
        fn staged_write_mark(&self, txn_id: graphdb_core::types::TransactionId) -> Option<graphdb_core::types::StagedWriteMark>;
        fn rollback_staged_writes(&self, txn_id: graphdb_core::types::TransactionId, mark: graphdb_core::types::StagedWriteMark) -> graphdb_transaction::undo_log::UndoLogResult<()>;
    );
}

#[cfg(test)]
mod tests {
    use crate::{GraphStorage, MetricsStorage, StoragePersistenceOps};

    #[test]
    fn delegates_admin_checkpoint_operations() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let inner = GraphStorage::new_with_path(temp_dir.path().to_path_buf())
            .expect("Failed to create GraphStorage");
        let storage = MetricsStorage::new(inner);

        let checkpoint = storage
            .create_checkpoint()
            .expect("checkpoint should succeed");

        assert!(checkpoint.is_some());
    }
}
