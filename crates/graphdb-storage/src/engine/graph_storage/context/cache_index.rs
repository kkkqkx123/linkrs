use crate::index::traits::IndexGcOps;
use crate::index::types::{EdgeIdentity, GcStats};
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageResult, Value};

use super::GraphStorageContext;

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
        edge: &EdgeIdentity<'_>,
        props: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        for index in self
            .index_metadata_manager()
            .list_edge_indexes(edge.space_id)?
            .into_iter()
            .filter(|index| index.schema_name == edge.edge_type)
        {
            self.update_edge_indexes_mvcc(edge, &index.name, props, ts)?;
        }
        Ok(())
    }

    pub(crate) fn delete_all_edge_indexes_mvcc(
        &self,
        edge: &EdgeIdentity<'_>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let index_names: Vec<String> = self
            .index_metadata_manager()
            .list_edge_indexes(edge.space_id)?
            .into_iter()
            .filter(|index| index.schema_name == edge.edge_type)
            .map(|index| index.name)
            .collect();
        if !index_names.is_empty() {
            self.delete_edge_indexes_mvcc(edge, &index_names, ts)?;
        }
        Ok(())
    }

    pub(crate) fn gc_index_tombstones(&self, ts: Timestamp) -> StorageResult<GcStats> {
        self.persistent.index_data_manager.read().gc_tombstones(ts)
    }
}
