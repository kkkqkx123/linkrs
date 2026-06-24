use crate::core::types::{LabelId, Timestamp};
use crate::core::{StorageResult, Value};
use crate::storage::edge::ExportedEdgeSnapshot;
use crate::storage::engine::data_store::EdgeTableKey;
use crate::storage::index::{GcStats, IndexGcOps};

use super::GraphStorageContext;

pub struct ExportedEdgeSnapshotRecord {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub snapshot: ExportedEdgeSnapshot,
}

impl GraphStorageContext {
    pub(crate) fn invalidate_vertex_cache(&self, label: LabelId) {
        self.persistent
            .cache_manager
            .invalidate_vertices_by_label(label);
    }

    pub(crate) fn update_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        props: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        super::super::index_engine::update_vertex_indexes_mvcc(
            self, space_id, vertex_id, index_name, props, ts,
        )
    }

    pub(crate) fn delete_vertex_indexes_mvcc(
        &self,
        space_id: u64,
        vertex_id: &Value,
        index_names: &[String],
        ts: Timestamp,
    ) -> StorageResult<()> {
        super::super::index_engine::delete_vertex_indexes_mvcc(self, space_id, vertex_id, index_names, ts)
    }

    pub(crate) fn gc_index_tombstones(&self, ts: Timestamp) -> StorageResult<GcStats> {
        self.persistent.index_data_manager.read().gc_tombstones(ts)
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>> {
        use crate::storage::edge::edge_table::EdgeSnapshotHandle;
        let mut edge_tables = self.persistent.data_store.edge_tables().write();
        let mut results = Vec::with_capacity(edge_tables.len());
        let mut handles: Vec<EdgeSnapshotHandle<'_>> = Vec::new();
        for (
            EdgeTableKey {
                src_label,
                dst_label,
                edge_label,
            },
            table,
        ) in edge_tables.iter_mut()
        {
            let mut handle = table.snapshot_handle(ts);
            let snapshot = handle.export()?;
            results.push(ExportedEdgeSnapshotRecord {
                src_label: *src_label,
                dst_label: *dst_label,
                edge_label: *edge_label,
                snapshot,
            });
            handles.push(handle);
        }
        Ok(results)
    }

    pub fn release_snapshot(&self, ts: Timestamp) {
        let mut edge_tables = self.persistent.data_store.edge_tables().write();
        for (_, table) in edge_tables.iter_mut() {
            table.unregister_snapshot(ts);
        }
    }
}
