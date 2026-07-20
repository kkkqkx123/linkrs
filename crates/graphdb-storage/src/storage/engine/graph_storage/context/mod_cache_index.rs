use crate::core::metadata::IndexMetadataManager;
use crate::core::types::{LabelId, Timestamp};
use crate::core::{StorageResult, Value};
use crate::storage::edge::ExportedEdgeSnapshot;
use crate::storage::engine::data_store::EdgeTableKey;
use crate::storage::index::index_data_manager::EdgeIdentity;
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
        super::super::index_engine::delete_vertex_indexes_mvcc(
            self,
            space_id,
            vertex_id,
            index_names,
            ts,
        )
    }

    pub(crate) fn update_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_name: &str,
        props: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        super::super::index_engine::update_edge_indexes_mvcc(self, edge, index_name, props, ts)
    }

    pub(crate) fn delete_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        index_names: &[String],
        ts: Timestamp,
    ) -> StorageResult<()> {
        super::super::index_engine::delete_edge_indexes_mvcc(self, edge, index_names, ts)
    }

    pub(crate) fn update_all_edge_indexes_mvcc(
        &self,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
        ranking: i64,
        props: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let edge = EdgeIdentity::new(space_id, src, dst, edge_type, ranking);
        for index in self
            .index_metadata_manager()
            .list_edge_indexes(space_id)?
            .into_iter()
            .filter(|index| index.schema_name == edge_type)
        {
            self.update_edge_indexes_mvcc(&edge, &index.name, props, ts)?;
        }
        Ok(())
    }

    pub(crate) fn delete_all_edge_indexes_mvcc(
        &self,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
        ranking: i64,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let edge = EdgeIdentity::new(space_id, src, dst, edge_type, ranking);
        let index_names: Vec<String> = self
            .index_metadata_manager()
            .list_edge_indexes(space_id)?
            .into_iter()
            .filter(|index| index.schema_name == edge_type)
            .map(|index| index.name)
            .collect();
        if !index_names.is_empty() {
            self.delete_edge_indexes_mvcc(&edge, &index_names, ts)?;
        }
        Ok(())
    }

    pub(crate) fn gc_index_tombstones(&self, ts: Timestamp) -> StorageResult<GcStats> {
        self.persistent.index_data_manager.read().gc_tombstones(ts)
    }

    pub fn export_snapshot(&self, ts: Timestamp) -> StorageResult<Vec<ExportedEdgeSnapshotRecord>> {
        use crate::storage::edge::edge_table::EdgeSnapshotHandle;
        self.persistent
            .data_store
            .with_edge_tables_mut(|edge_tables| {
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
                    let handle = table.snapshot_handle(ts);
                    log::debug!(
                        "Exporting snapshot at ts={} for edge table {}/{}/{}",
                        handle.timestamp(),
                        src_label,
                        dst_label,
                        edge_label
                    );
                    let snapshot = handle.export()?;
                    results.push(ExportedEdgeSnapshotRecord {
                        src_label: *src_label,
                        dst_label: *dst_label,
                        edge_label: *edge_label,
                        snapshot,
                    });
                    handles.push(handle);
                }
                for handle in handles {
                    handle.release();
                }
                Ok(results)
            })
    }
}
