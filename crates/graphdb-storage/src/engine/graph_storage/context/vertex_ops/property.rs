use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::atomic::Ordering;

use super::super::GraphStorageContext;

impl GraphStorageContext {
    pub fn update_vertex_property(
        &self,
        label: LabelId,
        external_id: &str,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
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
                let internal_id = table
                    .get_internal_id(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }

    pub fn update_vertex_property_by_i64(
        &self,
        label: LabelId,
        external_id: i64,
        property_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
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
                let internal_id = table
                    .get_internal_id_by_i64(external_id, ts)
                    .ok_or(StorageError::vertex_not_found())?;
                table.update_property(internal_id, property_name, value, ts)?;
                Ok(internal_id)
            })?;

        self.persistent
            .cache_manager
            .remove_cached_vertex(label, internal_id);
        self.mark_vertex_modified(label);

        Ok(())
    }
}
