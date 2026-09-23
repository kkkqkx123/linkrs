use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use super::super::GraphStorageContext;

impl GraphStorageContext {
    pub fn insert_vertex(
        &self,
        label: LabelId,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert(external_id, properties, ts)
            })?;

        self.persistent
            .cache_manager
            .cache_vertex_id(label, external_id, internal_id, ts);
        self.mark_vertex_modified(label);
        self.observe_vertex_id_string(label);

        Ok(internal_id)
    }

    pub fn insert_vertex_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        // Single rejection point for negative ids lives in
        // VertexId::try_from_int64; the table layer re-checks as backstop.
        VertexId::try_from_int64(external_id)?;
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let internal_id = self
            .persistent
            .data_store
            .with_vertex_tables_mut(|vertex_tables| {
                let table = vertex_tables.get(&label).ok_or_else(|| {
                    StorageError::label_not_found(format!("vertex label {}", label))
                })?;
                table.insert_by_i64(external_id, properties, ts)
            })?;

        self.persistent.cache_manager.cache_vertex_id(
            label,
            &external_id.to_string(),
            internal_id,
            ts,
        );
        self.mark_vertex_modified(label);
        self.observe_vertex_id_i64(label, external_id);

        Ok(internal_id)
    }
}
